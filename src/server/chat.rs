use crate::chat::{ChatCommandExecutor, ChatPlatform, InboundChatMessage, InboundSender};
use crate::server::AppState;
use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

static LARK_DEDUP_CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn is_duplicate_lark_message(msg_id: &str) -> bool {
    if msg_id.is_empty() {
        return false;
    }
    let mut cache = LARK_DEDUP_CACHE.lock().unwrap();
    let now = std::time::Instant::now();
    cache.retain(|_, seen_at| now.duration_since(*seen_at).as_secs() < 600);
    if cache.contains_key(msg_id) {
        true
    } else {
        cache.insert(msg_id.to_string(), now);
        false
    }
}

/// Answer an inbound command and record the exchange for `/admin/chats`.
///
/// Recording is best-effort and never affects the answer: the calling platform
/// is blocked on the reply, and `record_chat_exchange` already swallows storage
/// errors, so there is no path here that can turn a webhook into a failure.
async fn execute_and_record(
    executor: &ChatCommandExecutor<'_>,
    inbound: &InboundChatMessage,
    db: Option<&crate::web::db::WebDb>,
) -> String {
    let started = std::time::Instant::now();
    let reply = executor.execute(inbound).await;
    if let Some(db) = db {
        db.record_chat_exchange(inbound, &reply, started.elapsed().as_millis() as i64);
    }
    reply
}

// ==============================================================================
// 1. Zulip Inbound Webhook
// ==============================================================================

#[derive(Debug, Deserialize)]
pub struct ZulipWebhookRequest {
    pub token: Option<String>,
    pub data: Option<String>,
    pub message: Option<ZulipMessageDetails>,
}

#[derive(Debug, Deserialize)]
pub struct ZulipMessageDetails {
    pub content: Option<String>,
    pub sender_email: Option<String>,
    pub sender_full_name: Option<String>,
    pub sender_id: Option<i64>,
    pub stream_id: Option<i64>,
    pub display_recipient: Option<serde_json::Value>,
}

pub async fn zulip_chat_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let config = state.config();
    let payload: ZulipWebhookRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to parse Zulip webhook request: {:?}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid Zulip payload" })),
            );
        }
    };

    // Extract token from JSON payload or HTTP headers (Authorization, X-Zulip-Token)
    let received_token = payload
        .token
        .as_deref()
        .or_else(|| {
            headers
                .get("X-Zulip-Token")
                .or_else(|| headers.get("token"))
                .and_then(|v| v.to_str().ok())
        })
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim_start_matches("Bearer ").trim())
        });

    // Mandatory token verification: strictly require SUGIE_ZULIP_OUTGOING_TOKEN.
    // Do NOT fall back to outbound bot_api_key to prevent credential cross-contamination.
    let expected_token = config
        .chat
        .zulip_token
        .as_deref()
        .filter(|t| !t.trim().is_empty());

    let expected_token = match expected_token {
        Some(t) => t,
        None => {
            warn!(
                "Rejected Zulip webhook: SUGIE_ZULIP_OUTGOING_TOKEN is not configured in .env, \
                 so inbound requests cannot be authenticated"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "Zulip token is not configured" })),
            );
        }
    };

    let is_match = received_token.map_or(false, |tok| secret_matches(tok, expected_token));
    if !is_match {
        warn!("Zulip webhook authentication failed: invalid Zulip token");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Unauthorized: invalid Zulip token" })),
        );
    }

    let raw_content = payload
        .data
        .or_else(|| payload.message.as_ref().and_then(|m| m.content.clone()))
        .unwrap_or_default();

    let sender_email = payload
        .message
        .as_ref()
        .and_then(|m| m.sender_email.clone())
        .unwrap_or_else(|| "zulip_user".to_string());

    let sender_name = payload
        .message
        .as_ref()
        .and_then(|m| m.sender_full_name.clone());

    let username = sender_email
        .split('@')
        .next()
        .unwrap_or(&sender_email)
        .to_string();

    info!(
        "Received Zulip chat message from @{} ({}): {:?}",
        username, sender_email, raw_content
    );

    let mut extra_ids = Vec::new();
    if let Some(ref m) = payload.message {
        if let Some(uid) = m.sender_id {
            extra_ids.push(uid.to_string());
        }
    }
    if username != sender_email {
        extra_ids.push(username.clone());
    }

    let channel = payload
        .message
        .as_ref()
        .and_then(|m| m.display_recipient.as_ref())
        .and_then(|v| v.as_str().map(|s| s.to_string()));

    let mut extra_channel_ids = Vec::new();
    if let Some(ref m) = payload.message {
        if let Some(sid) = m.stream_id {
            extra_channel_ids.push(sid.to_string());
        }
    }

    let inbound = InboundChatMessage {
        platform: ChatPlatform::Zulip,
        sender: InboundSender {
            id: sender_email,
            username,
            display_name: sender_name,
            extra_ids,
        },
        raw_content,
        stream_or_channel: channel,
        message_id: None,
        extra_channel_ids,
    };

    let executor = ChatCommandExecutor::new(&config, &state.triage);
    let reply = execute_and_record(&executor, &inbound, state.web_db.as_ref()).await;

    // Zulip automatically renders JSON `{ "content": "..." }` in the stream or direct message
    (StatusCode::OK, Json(json!({ "content": reply })))
}

// ==============================================================================
// 2. Lark (Feishu) Inbound Webhook
// ==============================================================================

#[derive(Debug, Deserialize)]
pub struct LarkCallbackRequest {
    #[serde(default)]
    pub r#type: Option<String>,
    pub challenge: Option<String>,
    pub token: Option<String>,
    pub encrypt: Option<String>,
    pub header: Option<LarkHeader>,
    pub event: Option<LarkEvent>,
}

#[derive(Debug, Deserialize)]
pub struct LarkHeader {
    pub event_type: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LarkEvent {
    #[serde(default)]
    pub r#type: Option<String>,
    pub text: Option<String>,
    pub text_without_at_bot: Option<String>,
    pub open_id: Option<String>,
    pub employee_id: Option<String>,
    pub union_id: Option<String>,
    pub open_chat_id: Option<String>,
    pub open_message_id: Option<String>,
    pub sender: Option<LarkSender>,
    pub message: Option<LarkMessage>,
}

#[derive(Debug, Deserialize)]
pub struct LarkSender {
    pub sender_id: Option<LarkSenderId>,
    pub sender_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LarkSenderId {
    pub open_id: Option<String>,
    pub user_id: Option<String>,
    pub union_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LarkMessage {
    pub message_id: Option<String>,
    pub chat_id: Option<String>,
    pub content: Option<String>,
    pub message_type: Option<String>,
}

pub fn decrypt_lark_payload(encrypt_key: &str, encrypted_base64: &str) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(encrypt_key.as_bytes());
    let key_bytes = hasher.finalize();

    let raw_bytes = BASE64
        .decode(encrypted_base64)
        .map_err(|e| anyhow::anyhow!("Base64 decode failed: {:?}", e))?;
    if raw_bytes.len() < 16 {
        anyhow::bail!("Encrypted payload too short (must be >= 16 bytes for IV)");
    }

    let iv = &raw_bytes[..16];
    let mut ciphertext = raw_bytes[16..].to_vec();

    let dec = Aes256CbcDec::new_from_slices(&key_bytes, iv)
        .map_err(|e| anyhow::anyhow!("Invalid key/iv: {:?}", e))?;
    let decrypted_bytes = dec
        .decrypt_padded::<Pkcs7>(&mut ciphertext)
        .map_err(|e| anyhow::anyhow!("AES decryption failed: {:?}", e))?;

    let decrypted_str = std::str::from_utf8(decrypted_bytes)
        .map_err(|e| anyhow::anyhow!("Decrypted payload is not valid UTF-8: {:?}", e))?;
    Ok(decrypted_str.to_string())
}

pub fn encrypt_lark_payload(encrypt_key: &str, plaintext: &str) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(encrypt_key.as_bytes());
    let key_bytes = hasher.finalize();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut iv_hasher = Sha256::new();
    iv_hasher.update(now.to_le_bytes());
    iv_hasher.update(plaintext.as_bytes());
    let iv_hash = iv_hasher.finalize();
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&iv_hash[..16]);

    let enc = Aes256CbcEnc::new_from_slices(&key_bytes, &iv)
        .map_err(|e| anyhow::anyhow!("Invalid key/iv: {:?}", e))?;

    let pt_bytes = plaintext.as_bytes();
    let mut buf = vec![0u8; pt_bytes.len() + 16];

    let ct = enc
        .encrypt_padded_b2b::<Pkcs7>(pt_bytes, &mut buf)
        .map_err(|e| anyhow::anyhow!("AES encryption failed: {:?}", e))?;

    let mut combined = iv.to_vec();
    combined.extend_from_slice(ct);
    Ok(BASE64.encode(&combined))
}

pub fn encrypt_lark_challenge(encrypt_key: &str, challenge: &str) -> anyhow::Result<String> {
    let plaintext = json!({ "challenge": challenge }).to_string();
    encrypt_lark_payload(encrypt_key, &plaintext)
}

pub async fn lark_chat_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let config = state.config();

    // Check timestamp freshness if Lark timestamp header is present (replay attack defense)
    if let Some(ts_str) = headers
        .get("X-Lark-Request-Timestamp")
        .and_then(|v| v.to_str().ok())
    {
        if let Ok(ts_sec) = ts_str.trim().parse::<i64>() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if (now - ts_sec).abs() > 300 {
                warn!("Rejected Lark callback: timestamp is outside +/- 300s window");
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Lark timestamp out of range" })),
                );
            }
        }
    }
    let raw_payload: LarkCallbackRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to parse Lark callback payload: {:?}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid Lark payload" })),
            );
        }
    };

    let (payload, was_encrypted) = if let Some(ref enc_str) = raw_payload.encrypt {
        let encrypt_key = match config.chat.lark_encrypt_key.as_deref() {
            Some(k) if !k.is_empty() => k,
            _ => {
                warn!("Received encrypted Lark callback, but SUGIE_LARK_ENCRYPT_KEY is not configured in .env");
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Encrypt key not configured" })),
                );
            }
        };

        match decrypt_lark_payload(encrypt_key, enc_str) {
            Ok(decrypted_json) => {
                info!("Successfully decrypted Lark callback payload");
                debug!("Decrypted Lark JSON body: {}", decrypted_json);
                match serde_json::from_str::<LarkCallbackRequest>(&decrypted_json) {
                    Ok(parsed) => (parsed, true),
                    Err(e) => {
                        warn!("Failed to parse decrypted Lark JSON: {:?}", e);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": "Invalid decrypted Lark JSON" })),
                        );
                    }
                }
            }
            Err(e) => {
                warn!("Failed to decrypt Lark payload with Encrypt Key: {:?}", e);
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Decryption failed" })),
                );
            }
        }
    } else {
        (raw_payload, false)
    };

    let expected_token = config
        .chat
        .lark_verification_token
        .as_deref()
        .filter(|t| !t.trim().is_empty());

    // If payload is unencrypted, verification token is mandatory
    if !was_encrypted && expected_token.is_none() {
        warn!(
            "Rejected unencrypted Lark callback: SUGIE_LARK_VERIFICATION_TOKEN is not configured in .env"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Lark verification token is not configured" })),
        );
    }

    // 1. Handle URL Verification Challenge from Lark Developer Console
    if payload.r#type.as_deref() == Some("url_verification") {
        if let Some(ref challenge) = payload.challenge {
            if let Some(expected) = expected_token {
                let matches = payload
                    .token
                    .as_deref()
                    .map_or(false, |tok| secret_matches(tok, expected));
                if !matches {
                    warn!("Lark challenge verification token mismatch");
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": "Invalid Lark verification token" })),
                    );
                }
            }
            info!(
                "Responded to Lark URL verification challenge: {:?}",
                challenge
            );
            return (StatusCode::OK, Json(json!({ "challenge": challenge })));
        }
    }

    let event_type = payload
        .header
        .as_ref()
        .and_then(|h| h.event_type.as_deref())
        .or(payload.event.as_ref().and_then(|e| e.r#type.as_deref()))
        .or(payload.r#type.as_deref())
        .unwrap_or("unknown")
        .to_string();

    info!("Lark callback event received: type='{}'", event_type);

    // 2. Token verification for event callbacks
    if let Some(expected) = expected_token {
        let token_in_payload = payload
            .header
            .as_ref()
            .and_then(|h| h.token.as_deref())
            .or(payload.token.as_deref());
        let matches = token_in_payload.map_or(false, |tok| secret_matches(tok, expected));
        if !matches {
            warn!(
                "Lark event callback token verification failed: got {:?}",
                token_in_payload
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid Lark token" })),
            );
        }
    }

    // 3. Process Message Event (Schema 2.0 `im.message.receive_v1` or Schema 1.0 `message`)
    if let Some(event) = payload.event {
        // Ignore events sent by bots (e.g. self-replies or automated bot messages)
        let is_bot = event
            .sender
            .as_ref()
            .and_then(|s| s.sender_type.as_deref())
            .map(|st| st.eq_ignore_ascii_case("bot"))
            .unwrap_or(false);
        if is_bot {
            debug!("Ignoring Lark message event from bot sender");
            return (StatusCode::OK, Json(json!({ "code": 0, "msg": "ignored bot sender" })));
        }

        let (raw_text, sender_id, extra_ids, chat_id, message_id) = if let Some(msg) = event.message {
            // Schema 2.0
            let text = if let Some(content_str) = msg.content {
                if let Ok(json_obj) = serde_json::from_str::<serde_json::Value>(&content_str) {
                    json_obj
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    content_str
                }
            } else {
                String::new()
            };

            let mut extras = Vec::new();
            let mut main_id = "lark_user".to_string();

            if let Some(s) = event.sender.as_ref() {
                if let Some(sid) = s.sender_id.as_ref() {
                    if let Some(ref oid) = sid.open_id {
                        main_id = oid.clone();
                    }
                    if let Some(ref uid) = sid.user_id {
                        if !uid.trim().is_empty() && uid != &main_id {
                            extras.push(uid.clone());
                        }
                    }
                    if let Some(ref un_id) = sid.union_id {
                        if !un_id.trim().is_empty() && un_id != &main_id {
                            extras.push(un_id.clone());
                        }
                    }
                }
            }

            (text, main_id, extras, msg.chat_id, msg.message_id)
        } else if let Some(text) = event.text_without_at_bot.or(event.text) {
            // Schema 1.0
            let mut extras = Vec::new();
            let main_id = event.open_id.unwrap_or_else(|| "lark_user".to_string());
            if let Some(emp_id) = event.employee_id {
                if !emp_id.trim().is_empty() && emp_id != main_id {
                    extras.push(emp_id);
                }
            }
            if let Some(un_id) = event.union_id {
                if !un_id.trim().is_empty() && un_id != main_id {
                    extras.push(un_id);
                }
            }
            (text, main_id, extras, event.open_chat_id, event.open_message_id)
        } else {
            (String::new(), String::new(), Vec::new(), None, None)
        };

        // Deduplicate: If Lark delivers both Schema 1.0 and 2.0 (or retries), ignore duplicate
        if let Some(ref mid) = message_id {
            if is_duplicate_lark_message(mid) {
                info!("Ignoring duplicate Lark message delivery for message_id={:?}", mid);
                return (StatusCode::OK, Json(json!({ "code": 0, "msg": "duplicate ignored" })));
            }
        }

        if !raw_text.trim().is_empty() {
            info!(
                "Received Lark chat message from @{} (chat_id: {:?}): {:?}",
                sender_id, chat_id, raw_text
            );

            let inbound = InboundChatMessage {
                platform: ChatPlatform::Lark,
                sender: InboundSender {
                    id: sender_id.clone(),
                    username: sender_id.clone(),
                    display_name: None,
                    extra_ids,
                },
                raw_content: raw_text,
                stream_or_channel: chat_id,
                message_id,
                extra_channel_ids: Vec::new(),
            };

            let config_clone = config.clone();
            let triage = state.triage.clone();
            // The console database handle, cloned so the spawned task can record
            // the exchange without capturing the whole `AppState`.
            let chat_db = state.web_db.clone();

            // Spawn execution and dispatch reply back to Lark
            tokio::spawn(async move {
                let executor = ChatCommandExecutor::new(&config_clone, &triage);
                let reply = execute_and_record(&executor, &inbound, chat_db.as_ref()).await;
                info!("Sending Lark response: {}", reply);

                if let Some(lark) = triage.notifier().lark() {
                    let result = if lark.has_bot() {
                        // When Bot credentials are configured:
                        // First preference: reply directly into the message thread
                        if let Some(ref mid) = inbound.message_id {
                            lark.reply_to_message(mid, "🤖 Sugie Bot Response", &reply, None, "blue", &[]).await
                        } else if let Some(ref cid) = inbound.stream_or_channel {
                            lark.send_to_chat(cid, "🤖 Sugie Bot Response", &reply, None, "blue", &[]).await
                        } else {
                            lark.send_notification("🤖 Sugie Bot Response", &reply, None, "blue", &[]).await
                        }
                    } else {
                        // Fallback to webhook mode
                        lark.send_notification("🤖 Sugie Bot Response", &reply, None, "blue", &[]).await
                    };

                    match result {
                        Ok(_) => info!("Successfully dispatched Lark response"),
                        Err(e) => error!("Failed to dispatch Lark reply: {:?}", e),
                    }
                } else {
                    warn!("Lark notifier is not configured; cannot deliver response");
                }
            });

            return (StatusCode::OK, Json(json!({ "code": 0, "msg": "success" })));
        } else {
            info!(
                "Lark event '{}' processed (no executable message text)",
                event_type
            );
        }
    }

    (
        StatusCode::OK,
        Json(json!({ "code": 0, "msg": "event received" })),
    )
}

// ==============================================================================
// 3. Discord Inbound Interactions Webhook
// ==============================================================================

pub async fn discord_chat_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let config = state.config();
    // 1. Verify Ed25519 signature with configured Discord public key
    let pubkey_hex = match config
        .chat
        .discord_public_key
        .as_deref()
        .filter(|k| !k.trim().is_empty())
    {
        Some(k) => k,
        None => {
            warn!(
                "Rejected Discord interaction: SUGIE_DISCORD_PUBLIC_KEY is not configured in .env"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "Discord public key is not configured" })),
            );
        }
    };

    let sig_header = headers
        .get("X-Signature-Ed25519")
        .and_then(|v| v.to_str().ok());
    let time_header = headers
        .get("X-Signature-Timestamp")
        .and_then(|v| v.to_str().ok());

    match (sig_header, time_header) {
        (Some(sig), Some(time)) => {
            if !verify_discord_signature(pubkey_hex, sig, time, &body) {
                warn!("Discord signature verification failed");
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Invalid Discord signature" })),
                );
            }
        }
        _ => {
            warn!("Missing Discord signature headers");
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Missing signature headers" })),
            );
        }
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to parse Discord interaction payload: {:?}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid Discord payload" })),
            );
        }
    };

    let interaction_type = payload.get("type").and_then(|v| v.as_i64()).unwrap_or(0);

    // Discord Type 1: PING handshake (respond with type 1 PONG)
    if interaction_type == 1 {
        info!("Responded to Discord PING interaction");
        return (StatusCode::OK, Json(json!({ "type": 1 })));
    }

    // Discord Type 2 (Application Command) or Type 4 / direct webhook
    let mut command_text = String::new();

    // Check if Discord sent slash command options
    if let Some(data) = payload.get("data") {
        let cmd_name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
        command_text = format!("/{}", cmd_name);

        if let Some(options) = data.get("options").and_then(|v| v.as_array()) {
            for opt in options {
                if let Some(val) = opt.get("value").and_then(|v| v.as_str()) {
                    command_text.push(' ');
                    command_text.push_str(val);
                }
            }
        }
    } else if let Some(content) = payload.get("content").and_then(|v| v.as_str()) {
        command_text = content.to_string();
    }

    let (user_id, username, display_name) = if let Some(member) = payload.get("member") {
        let u = member.get("user");
        let uid = u.and_then(|x| x.get("id")).and_then(|v| v.as_str()).unwrap_or("discord_user");
        let uname = u.and_then(|x| x.get("username")).and_then(|v| v.as_str()).unwrap_or(uid);
        let dname = member.get("nick").and_then(|v| v.as_str()).or_else(|| u.and_then(|x| x.get("global_name")).and_then(|v| v.as_str()));
        (uid.to_string(), uname.to_string(), dname.map(|s| s.to_string()))
    } else if let Some(u) = payload.get("user") {
        let uid = u.get("id").and_then(|v| v.as_str()).unwrap_or("discord_user");
        let uname = u.get("username").and_then(|v| v.as_str()).unwrap_or(uid);
        let dname = u.get("global_name").and_then(|v| v.as_str());
        (uid.to_string(), uname.to_string(), dname.map(|s| s.to_string()))
    } else {
        ("discord_user".to_string(), "discord_user".to_string(), None)
    };

    let channel_id = payload.get("channel_id").and_then(|v| v.as_str()).map(|s| s.to_string());
    let guild_id = payload.get("guild_id").and_then(|v| v.as_str()).map(|s| s.to_string());

    info!(
        "Received Discord interaction from @{} (id: {}): {:?}",
        username, user_id, command_text
    );

    let mut extra_ids = Vec::new();
    if user_id != username {
        extra_ids.push(username.clone());
    }

    let mut extra_channel_ids = Vec::new();
    if let Some(gid) = guild_id {
        extra_channel_ids.push(gid);
    }

    let inbound = InboundChatMessage {
        platform: ChatPlatform::Discord,
        sender: InboundSender {
            id: user_id,
            username,
            display_name,
            extra_ids,
        },
        raw_content: command_text,
        stream_or_channel: channel_id,
        message_id: None,
        extra_channel_ids,
    };

    let executor = ChatCommandExecutor::new(&config, &state.triage);
    let reply = execute_and_record(&executor, &inbound, state.web_db.as_ref()).await;

    // Discord hard-caps message content at 2000 characters. If a response exceeds that,
    // Discord rejects the interaction response and reports "didn't respond in time".
    let safe_content = if reply.chars().count() > 2000 {
        let mut truncated: String = reply.chars().take(1950).collect();
        truncated.push_str("\n... (truncated to fit Discord limit)");
        truncated
    } else {
        reply
    };

    // Discord Type 4: ChannelMessageWithSource
    (
        StatusCode::OK,
        Json(json!({
            "type": 4,
            "data": {
                "content": safe_content
            }
        })),
    )
}

fn verify_discord_signature(
    public_key_hex: &str,
    signature_hex: &str,
    timestamp: &str,
    body: &[u8],
) -> bool {
    // Validate timestamp freshness (within +/- 300 seconds of current system time)
    // to prevent replay attacks of captured interactions.
    let ts_num: i64 = match timestamp.trim().parse() {
        Ok(t) => t,
        Err(_) => {
            warn!("Discord signature verification rejected: invalid timestamp integer '{}'", timestamp);
            return false;
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if (now - ts_num).abs() > 300 {
        warn!(
            "Discord signature timestamp rejected (replay attack prevention): timestamp {} is {}s from current time {}",
            ts_num,
            now - ts_num,
            now
        );
        return false;
    }

    let pk_clean = public_key_hex.trim();
    let pk_bytes = match hex::decode(pk_clean) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        Ok(b) => {
            warn!(
                "Invalid Discord public key length: expected 32 bytes (64 hex chars), got {} bytes",
                b.len()
            );
            return false;
        }
        Err(e) => {
            warn!(
                "Failed to hex-decode SUGIE_DISCORD_PUBLIC_KEY: {:?}. Ensure you copied the 64-character hexadecimal 'PUBLIC KEY' (not Client Secret) from Discord Developer Portal",
                e
            );
            return false;
        }
    };

    let verifying_key = match VerifyingKey::from_bytes(&pk_bytes) {
        Ok(vk) => vk,
        Err(e) => {
            warn!("Failed to parse Discord VerifyingKey: {:?}", e);
            return false;
        }
    };

    let sig_clean = signature_hex.trim();
    let sig_bytes = match hex::decode(sig_clean) {
        Ok(b) if b.len() == 64 => {
            let mut arr = [0u8; 64];
            arr.copy_from_slice(&b);
            arr
        }
        _ => return false,
    };

    let signature = Signature::from_bytes(&sig_bytes);
    let mut message = timestamp.as_bytes().to_vec();
    message.extend_from_slice(body);

    verifying_key.verify(&message, &signature).is_ok()
}

// ==============================================================================
// 4. Generic API / Curl Testing Endpoint
// ==============================================================================

#[derive(Debug, Deserialize)]
pub struct GenericCommandRequest {
    pub command: String,
    pub sender: Option<String>,
}

/// Compare a presented secret with the configured one without leaking the
/// answer through timing. Both sides are hashed first so the loop always runs
/// over the same number of bytes, whatever the caller sent.
fn secret_matches(provided: &str, expected: &str) -> bool {
    let a = Sha256::digest(provided.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub async fn generic_chat_command_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GenericCommandRequest>,
) -> impl IntoResponse {
    let expected = match state.chat_api_token.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => t,
        None => {
            warn!("Rejected /api/chat/command: SUGIE_CHAT_API_TOKEN is not configured in .env");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "Chat API token is not configured (set SUGIE_CHAT_API_TOKEN)" })),
            )
                .into_response();
        }
    };

    let provided = headers
        .get("x-sugie-chat-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !secret_matches(provided, expected) {
        // `{:?}` escapes control characters and the value is truncated, so a
        // caller cannot forge or flood log lines through the sender field.
        let claimed: Option<String> = payload
            .sender
            .as_deref()
            .map(|s| s.chars().take(64).collect());
        warn!("Rejected unauthenticated /api/chat/command call (sender={claimed:?})");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Missing or invalid X-Sugie-Chat-Token"})),
        )
            .into_response();
    }

    let config = state.config();
    let sender_name = payload.sender.unwrap_or_else(|| "api_user".to_string());
    // Prefix sender ID with api: so generic API requests cannot spoof GitHub or other platform identities
    let sender_id = if sender_name.starts_with("api:") {
        sender_name.clone()
    } else {
        format!("api:{}", sender_name)
    };
    let inbound = InboundChatMessage {
        platform: ChatPlatform::Api,
        sender: InboundSender {
            id: sender_id.clone(),
            username: sender_name,
            display_name: None,
            extra_ids: vec![sender_id],
        },
        raw_content: payload.command,
        stream_or_channel: None,
        message_id: None,
        extra_channel_ids: Vec::new(),
    };

    let executor = ChatCommandExecutor::new(&config, &state.triage);
    let reply = execute_and_record(&executor, &inbound, state.web_db.as_ref()).await;

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "response": reply
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_api_secret_comparison() {
        assert!(secret_matches("abc123", "abc123"));
        assert!(!secret_matches("abc124", "abc123"));
        assert!(!secret_matches("abc12", "abc123"));
        assert!(!secret_matches("", "abc123"));
        // A long random token is what the docs recommend; make sure it round-trips.
        let long = "9f2c1d7e4b6a8c0f3e5d7b9a1c2e4f60718293a4b5c6d7e8f90a1b2c3d4e5f60";
        assert!(secret_matches(long, long));
    }

    #[test]
    fn test_lark_encryption_and_decryption_cycle() {
        let encrypt_key = "test_lark_encrypt_key_123456789";
        let original_challenge = "challenge_token_abc_xyz";

        // Test encryption of challenge
        let encrypted_challenge = encrypt_lark_challenge(encrypt_key, original_challenge).unwrap();
        assert!(!encrypted_challenge.is_empty());

        // Test decryption
        let decrypted_json = decrypt_lark_payload(encrypt_key, &encrypted_challenge).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&decrypted_json).unwrap();
        assert_eq!(
            parsed.get("challenge").and_then(|v| v.as_str()),
            Some(original_challenge)
        );
    }
}

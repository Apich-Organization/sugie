use crate::config::{LarkChannelConfig, LarkConfig, LarkWebhookChannel};
use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::Client;
use serde_json::json;
use sha2::Sha256;
use std::sync::Arc;

pub use crate::config::LarkChannelConfig as LarkChannel;

type HmacSha256 = Hmac<Sha256>;

static LARK_OPEN_ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"@(?P<id>ou_[0-9a-zA-Z]+)").unwrap()
});

/// Replaces Lark Open ID mentions with Lark native `<at>` tag and converts global mentions.
pub fn convert_lark_mentions(text: &str) -> String {
    let with_users = LARK_OPEN_ID_RE.replace_all(text, r#"<at id="$id"></at>"#);
    with_users
        .replace("@all", "<at id=all></at>")
        .replace("@everyone", "<at id=all></at>")
}

/// Normalizes standard markdown into Lark's standard message card markdown (`lark_md`):
/// - Converts headings (`#`, `##`, `###`, `####`) to bold text (`**Heading**`), because
///   `lark_md` does not support standard markdown headings and displays raw `#` tags otherwise.
/// - Converts standard markdown list items (`- `, `* `) to bullets (`• `) with indentation preserved,
///   providing consistent, clean rendering across all Lark desktop and mobile apps.
/// - Converts blockquotes (`> `) to visual quote blocks (`▎ `).
/// - Converts Lark Open ID mentions (`@ou_...`) to native `<at id="ou_..."></at>` tags.
/// - Preserves code blocks, inline code, bold, italic, and hyperlinks.
pub fn format_markdown_for_lark(input: &str) -> String {
    let mut out = Vec::new();
    let mut in_code_block = false;

    for line in input.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            // Lark's code highlighter recognizes json, yaml, bash, python, etc. but not toml.
            // Map toml code blocks to yaml for syntax highlighting and reliable rendering.
            let fence = if trimmed.eq_ignore_ascii_case("```toml") {
                "```yaml".to_string()
            } else {
                trimmed.to_string()
            };
            out.push(fence);
            continue;
        }

        if in_code_block {
            out.push(line.to_string());
            continue;
        }

        // Horizontal rules on their own line can stay as is (used for splitting sections into hr tags)
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push("---".to_string());
            continue;
        }

        // Convert markdown headings (#, ##, ###, ####, etc.) to bold text
        if let Some(heading) = trimmed
            .strip_prefix("#### ")
            .or_else(|| trimmed.strip_prefix("### "))
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            let clean = heading.trim();
            if !clean.is_empty() {
                out.push(format!("**{}**", clean));
                continue;
            }
        }

        // Convert standard markdown list items (- item, * item) to bullet points (• item), preserving indentation
        let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
        let after_indent = &line[leading_spaces..];
        let bullet_line = if let Some(item) = after_indent.strip_prefix("- ") {
            format!("{}• {}", " ".repeat(leading_spaces), item)
        } else if let Some(item) = after_indent.strip_prefix("* ") {
            format!("{}• {}", " ".repeat(leading_spaces), item)
        } else {
            line.to_string()
        };

        // Convert markdown blockquotes (> text) to visual quote blocks (▎ text)
        let quote_line = {
            let spaces = bullet_line.chars().take_while(|c| *c == ' ').count();
            let after = &bullet_line[spaces..];
            if let Some(q) = after.strip_prefix("> ") {
                format!("{}▎ {}", " ".repeat(spaces), q)
            } else {
                bullet_line
            }
        };

        // Convert Lark mentions
        let processed = convert_lark_mentions(&quote_line);
        out.push(processed);
    }

    out.join("\n")
}

#[derive(Clone)]
struct LarkTokenCache {
    token: String,
    expires_at: std::time::Instant,
}

#[derive(Clone)]
pub struct LarkNotifier {
    config: LarkConfig,
    http: Client,
    token_cache: Arc<tokio::sync::RwLock<Option<LarkTokenCache>>>,
}

impl LarkNotifier {
    pub fn new(config: LarkConfig) -> Self {
        Self {
            config,
            http: crate::notifications::create_notification_http_client(),
            token_cache: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub fn config(&self) -> &LarkConfig {
        &self.config
    }

    /// Whether the bot has App ID and App Secret configured for Lark Open API.
    pub fn has_bot(&self) -> bool {
        self.config
            .app_id
            .as_ref()
            .map(|id| !id.trim().is_empty())
            .unwrap_or(false)
            && self
                .config
                .app_secret
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
    }

    /// Return all configured webhook channels, falling back to default webhook_url.
    pub fn webhook_channels(&self) -> Vec<LarkWebhookChannel> {
        if !self.config.webhooks.is_empty() {
            return self.config.webhooks.clone();
        }
        let mut list = Vec::new();
        if !self.config.webhook_url.trim().is_empty() {
            list.push(LarkWebhookChannel {
                name: "default".to_string(),
                url: self.config.webhook_url.trim().to_string(),
                secret: self.config.secret.clone(),
            });
        }
        list
    }

    /// Whether an incoming webhook URL is configured.
    pub fn has_webhook(&self) -> bool {
        !self.webhook_channels().is_empty()
    }

    /// Whether Lark can send notifications at all (either as bot or via webhook).
    pub fn is_configured(&self) -> bool {
        self.has_bot() || self.has_webhook()
    }

    /// Registered named channel (group chat) destinations.
    pub fn channels(&self) -> &[LarkChannelConfig] {
        &self.config.channels
    }

    /// Return all candidate bot channels, including default_chat_id if set and not in channels.
    pub fn bot_channels(&self) -> Vec<LarkChannelConfig> {
        let mut list = self.config.channels.clone();
        if let Some(ref default_cid) = self.config.default_chat_id {
            let cid = default_cid.trim();
            if !cid.is_empty() && !list.iter().any(|c| c.chat_id == cid) {
                list.insert(
                    0,
                    LarkChannelConfig {
                        name: "default".to_string(),
                        chat_id: cid.to_string(),
                    },
                );
            }
        }
        list
    }

    /// Resolve target channel name or ID to an actual Lark `chat_id` (`oc_xxx`).
    pub fn resolve_chat_id(&self, target: Option<&str>) -> Option<String> {
        if let Some(t) = target {
            if let Some(ch) = self
                .config
                .channels
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(t) || c.chat_id == t)
            {
                return Some(ch.chat_id.clone());
            }
            if t.starts_with("oc_") {
                return Some(t.to_string());
            }
        }
        self.config
            .default_chat_id
            .clone()
            .or_else(|| self.config.channels.first().map(|c| c.chat_id.clone()))
    }

    /// Fetch or return cached Lark `tenant_access_token` from Lark Open API.
    pub async fn get_tenant_access_token(&self) -> Result<String> {
        let (app_id, app_secret) = match (&self.config.app_id, &self.config.app_secret) {
            (Some(id), Some(secret)) if !id.trim().is_empty() && !secret.trim().is_empty() => {
                (id, secret)
            }
            _ => anyhow::bail!("Lark App ID and App Secret are not configured"),
        };

        // 1. Check existing cached token
        {
            let r = self.token_cache.read().await;
            if let Some(ref cache) = *r {
                if std::time::Instant::now() < cache.expires_at {
                    return Ok(cache.token.clone());
                }
            }
        }

        // 2. Request new token from Lark Open API
        let api_base = if self.config.api_base.trim().is_empty() {
            "https://open.larksuite.com"
        } else {
            self.config.api_base.trim_end_matches('/')
        };
        let token_url = format!("{}/open-apis/auth/v3/tenant_access_token/internal", api_base);

        let body = json!({
            "app_id": app_id,
            "app_secret": app_secret,
        });

        let started = std::time::Instant::now();
        let resp = self
            .http
            .post(&token_url)
            .json(&body)
            .send()
            .await
            .context("Failed to request Lark tenant_access_token")?;
        let duration_ms = started.elapsed().as_millis() as u64;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                &token_url,
                status.as_u16(),
                duration_ms,
                &format!("app_id={app_id} http_error={body_text}"),
            );
            anyhow::bail!("Lark token request HTTP error {}: {}", status, body_text);
        }

        let resp_json: serde_json::Value = serde_json::from_str(&body_text)
            .context("Failed to parse Lark token response")?;

        let code = resp_json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = resp_json
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                &token_url,
                status.as_u16(),
                duration_ms,
                &format!("app_id={app_id} lark_code={code} msg={msg}"),
            );
            anyhow::bail!("Lark token error (code {}): {}", code, msg);
        }

        let token = resp_json
            .get("tenant_access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing tenant_access_token in response"))?
            .to_string();

        let expire_secs = resp_json.get("expire").and_then(|v| v.as_u64()).unwrap_or(7200);
        // Refresh 5 minutes before expiration (or 30s minimum if expire is short)
        let ttl_secs = expire_secs.saturating_sub(300).max(30);
        let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(ttl_secs);

        {
            let mut w = self.token_cache.write().await;
            *w = Some(LarkTokenCache {
                token: token.clone(),
                expires_at,
            });
        }

        Ok(token)
    }

    /// Computes Lark webhook HMAC-SHA256 signature:
    /// string_to_sign = timestamp + "\n" + secret
    /// sign = Base64(HmacSHA256(key=string_to_sign, msg=""))
    fn generate_signature(timestamp: i64, secret: &str) -> Result<String> {
        let string_to_sign = format!("{}\n{}", timestamp, secret);
        let mut mac = HmacSha256::new_from_slice(string_to_sign.as_bytes())
            .context("Failed to initialize HMAC for Lark signature")?;
        mac.update(b"");
        let result = mac.finalize();
        Ok(base64::engine::general_purpose::STANDARD.encode(result.into_bytes()))
    }

    /// Builds a rich interactive card object for Lark/Feishu.
    pub fn build_interactive_card(
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> serde_json::Value {
        let mut effective_title = title.trim().to_string();
        let mut effective_color = color_template.trim().to_string();
        let mut raw_message = message.trim();

        // If the title is generic (e.g. "🤖 Sugie Bot Response" or empty) and message starts with a markdown header:
        // Extract the header as the card's title and remove it from the body to avoid awkward duplicates.
        if (effective_title.is_empty() || effective_title == "🤖 Sugie Bot Response")
            && (raw_message.starts_with('#'))
        {
            if let Some((first_line, rest)) = raw_message.split_once('\n') {
                let first_trimmed = first_line.trim();
                if let Some(h) = first_trimmed
                    .strip_prefix("### ")
                    .or_else(|| first_trimmed.strip_prefix("## "))
                    .or_else(|| first_trimmed.strip_prefix("# "))
                {
                    effective_title = h.trim().to_string();
                    raw_message = rest.trim();
                }
            }
        }

        // Adjust color template if message indicates failure / warning and color was default blue
        if effective_color == "blue" {
            if raw_message.starts_with("⛔") || raw_message.starts_with("❌") {
                effective_color = "red".to_string();
                if effective_title == "🤖 Sugie Bot Response" {
                    effective_title = "⛔ Action Denied".to_string();
                }
            } else if raw_message.starts_with("⚠️") || raw_message.starts_with("❓") {
                effective_color = "orange".to_string();
                if effective_title == "🤖 Sugie Bot Response" {
                    effective_title = "⚠️ Notice".to_string();
                }
            } else if raw_message.starts_with("✅") {
                effective_color = "green".to_string();
            }
        }

        let formatted = format_markdown_for_lark(raw_message);
        let mut elements = Vec::new();

        // Split by horizontal rules `---` to insert native Lark card `hr` dividing lines
        let sections: Vec<&str> = formatted
            .split("\n---\n")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if sections.is_empty() {
            elements.push(json!({
                "tag": "markdown",
                "content": formatted
            }));
        } else {
            for (idx, sec) in sections.iter().enumerate() {
                if idx > 0 {
                    elements.push(json!({ "tag": "hr" }));
                }
                elements.push(json!({
                    "tag": "markdown",
                    "content": sec
                }));
            }
        }

        // Key-value fields
        if !fields.is_empty() {
            let fields_md = fields
                .iter()
                .map(|(k, v)| format!("• **{}**: {}", k, v))
                .collect::<Vec<_>>()
                .join("\n");
            elements.push(json!({
                "tag": "markdown",
                "content": fields_md
            }));
        }

        // Action button for GitHub URL (Schema 2.0 uses button with open_url behavior)
        if let Some(target_url) = url {
            elements.push(json!({
                "tag": "button",
                "text": {
                    "tag": "plain_text",
                    "content": "View on GitHub ↗"
                },
                "type": "primary",
                "behaviors": [
                    {
                        "type": "open_url",
                        "default_url": target_url
                    }
                ]
            }));
        }

        json!({
            "schema": "2.0",
            "config": {
                "wide_screen_mode": true
            },
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": effective_title
                },
                "template": effective_color
            },
            "body": {
                "elements": elements
            }
        })
    }

    /// Send a notification message to a specific chat / group via Lark Bot Open API.
    pub async fn send_to_chat(
        &self,
        chat_id: &str,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let api_base = if self.config.api_base.trim().is_empty() {
            "https://open.larksuite.com"
        } else {
            self.config.api_base.trim_end_matches('/')
        };
        let send_url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", api_base);

        let card = Self::build_interactive_card(title, message, url, color_template, fields);
        let payload = json!({
            "receive_id": chat_id,
            "msg_type": "interactive",
            "content": card.to_string(),
        });

        let started = std::time::Instant::now();
        let resp = self
            .http
            .post(&send_url)
            .bearer_auth(&token)
            .json(&payload)
            .send()
            .await
            .context("Failed to send Lark message to chat")?;
        let duration_ms = started.elapsed().as_millis() as u64;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                &send_url,
                status.as_u16(),
                duration_ms,
                &format!("chat_id={chat_id} title={title} http_error={body_text}"),
            );
            anyhow::bail!("Lark API HTTP error {}: {}", status, body_text);
        }

        let resp_json: serde_json::Value = serde_json::from_str(&body_text)
            .context("Failed to parse Lark message response")?;
        let code = resp_json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = resp_json
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                &send_url,
                status.as_u16(),
                duration_ms,
                &format!("chat_id={chat_id} title={title} lark_code={code} msg={msg}"),
            );
            anyhow::bail!("Lark API rejected message (code {}): {}", code, msg);
        }

        crate::security_log::log_notification_request(
            "lark",
            "POST",
            &send_url,
            status.as_u16(),
            duration_ms,
            &format!("chat_id={chat_id} title={title} template={color_template}"),
        );

        Ok(())
    }

    /// Reply directly to a specific message thread via Lark Bot Open API.
    pub async fn reply_to_message(
        &self,
        message_id: &str,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let api_base = if self.config.api_base.trim().is_empty() {
            "https://open.larksuite.com"
        } else {
            self.config.api_base.trim_end_matches('/')
        };
        let reply_url = format!("{}/open-apis/im/v1/messages/{}/reply", api_base, message_id);

        let card = Self::build_interactive_card(title, message, url, color_template, fields);
        let payload = json!({
            "msg_type": "interactive",
            "content": card.to_string(),
        });

        let started = std::time::Instant::now();
        let resp = self
            .http
            .post(&reply_url)
            .bearer_auth(&token)
            .json(&payload)
            .send()
            .await
            .context("Failed to send Lark message reply")?;
        let duration_ms = started.elapsed().as_millis() as u64;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                &reply_url,
                status.as_u16(),
                duration_ms,
                &format!("reply_to={message_id} title={title} http_error={body_text}"),
            );
            anyhow::bail!("Lark API HTTP error {}: {}", status, body_text);
        }

        let resp_json: serde_json::Value = serde_json::from_str(&body_text)
            .context("Failed to parse Lark reply response")?;
        let code = resp_json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = resp_json
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                &reply_url,
                status.as_u16(),
                duration_ms,
                &format!("reply_to={message_id} title={title} lark_code={code} msg={msg}"),
            );
            anyhow::bail!("Lark API rejected reply (code {}): {}", code, msg);
        }

        crate::security_log::log_notification_request(
            "lark",
            "POST",
            &reply_url,
            status.as_u16(),
            duration_ms,
            &format!("reply_to={message_id} title={title} template={color_template}"),
        );

        Ok(())
    }

    /// Send notification to a specific Lark incoming webhook URL with optional secret.
    pub async fn send_to_webhook(
        &self,
        webhook_url: &str,
        secret: Option<&str>,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        let url_str = webhook_url.trim();
        if url_str.is_empty() {
            anyhow::bail!("Lark webhook URL is not configured");
        }

        let now = Utc::now().timestamp();
        let card = Self::build_interactive_card(title, message, url, color_template, fields);

        let mut payload = json!({
            "msg_type": "interactive",
            "card": card
        });

        if let Some(sec) = secret {
            let s = sec.trim();
            if !s.is_empty() {
                let sign = Self::generate_signature(now, s)?;
                payload["timestamp"] = json!(now.to_string());
                payload["sign"] = json!(sign);
            }
        }

        if !crate::notifications::is_safe_webhook_url(url_str) {
            anyhow::bail!("Blocked unsafe or non-HTTPS Lark webhook destination (SSRF protection)");
        }

        let started = std::time::Instant::now();
        let send_res = self
            .http
            .post(url_str)
            .json(&payload)
            .send()
            .await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let resp = match send_res {
            Ok(r) => r,
            Err(e) => {
                let safe_err = crate::notifications::sanitize_error_str(&e);
                crate::security_log::log_notification_request(
                    "lark",
                    "POST",
                    url_str,
                    0,
                    duration_ms,
                    &format!("title={title} error={safe_err}"),
                );
                return Err(e).context("Failed to send webhook request to Lark");
            }
        };

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            let safe_body = crate::notifications::sanitize_error_str(&body_text);
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                url_str,
                status.as_u16(),
                duration_ms,
                &format!("title={title} http_error={safe_body}"),
            );
            anyhow::bail!("Lark webhook HTTP error {}: {}", status, body_text);
        }

        if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(&body_text) {
            if let Some(code) = json_body.get("code").and_then(|c| c.as_i64()) {
                if code != 0 {
                    let msg = json_body
                        .get("msg")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    crate::security_log::log_notification_request(
                        "lark",
                        "POST",
                        url_str,
                        status.as_u16(),
                        duration_ms,
                        &format!("title={title} lark_code={code} msg={msg}"),
                    );
                    anyhow::bail!("Lark API rejected notification (code {}): {}", code, msg);
                }
            }
        }

        crate::security_log::log_notification_request(
            "lark",
            "POST",
            url_str,
            status.as_u16(),
            duration_ms,
            &format!("title={title} template={color_template}"),
        );

        Ok(())
    }

    /// Send notification via all configured incoming webhook URLs.
    pub async fn send_webhook_notification(
        &self,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        let whs = self.webhook_channels();
        if whs.is_empty() {
            anyhow::bail!("Lark webhook URL is not configured");
        }
        for wh in whs {
            self.send_to_webhook(
                &wh.url,
                wh.secret.as_deref(),
                title,
                message,
                url,
                color_template,
                fields,
            )
            .await?;
        }
        Ok(())
    }

    /// Dispatch notification automatically: prefers Bot channels if configured,
    /// otherwise falls back to incoming webhook URL.
    pub async fn send_notification(
        &self,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        if self.has_bot() {
            if !self.config.channels.is_empty() {
                for ch in &self.config.channels {
                    self.send_to_chat(
                        &ch.chat_id,
                        title,
                        message,
                        url,
                        color_template,
                        fields,
                    )
                    .await?;
                }
                Ok(())
            } else if let Some(ref default_cid) = self.config.default_chat_id {
                self.send_to_chat(
                    default_cid,
                    title,
                    message,
                    url,
                    color_template,
                    fields,
                )
                .await
            } else if self.has_webhook() {
                self.send_webhook_notification(title, message, url, color_template, fields).await
            } else {
                anyhow::bail!("Lark Bot is configured but no channels or default_chat_id specified")
            }
        } else if self.has_webhook() {
            self.send_webhook_notification(title, message, url, color_template, fields).await
        } else {
            anyhow::bail!("Lark is not configured (missing Bot credentials or Webhook URL)")
        }
    }

    /// Send notification to a specific channel name or chat ID.
    pub async fn send_notification_to_channel(
        &self,
        target_channel: Option<&str>,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        if let Some(target) = target_channel {
            // Check if target matches a configured webhook channel
            if let Some(wh) = self.webhook_channels().iter().find(|w| w.name.eq_ignore_ascii_case(target)) {
                return self.send_to_webhook(&wh.url, wh.secret.as_deref(), title, message, url, color_template, fields).await;
            }
        }

        if self.has_bot() {
            let chat_id = self
                .resolve_chat_id(target_channel)
                .ok_or_else(|| anyhow::anyhow!("Could not resolve Lark channel target: {:?}", target_channel))?;
            self.send_to_chat(&chat_id, title, message, url, color_template, fields).await
        } else if self.has_webhook() {
            self.send_webhook_notification(title, message, url, color_template, fields).await
        } else {
            anyhow::bail!("Lark is not configured")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lark_notifier_configuration_states() {
        // 1. Empty config
        let cfg_empty = LarkConfig::default();
        let n_empty = LarkNotifier::new(cfg_empty);
        assert!(!n_empty.has_bot());
        assert!(!n_empty.has_webhook());
        assert!(!n_empty.is_configured());

        // 2. Webhook only
        let cfg_webhook = LarkConfig {
            webhook_url: "https://open.larksuite.com/open-apis/bot/v2/hook/abc".into(),
            ..Default::default()
        };
        let n_webhook = LarkNotifier::new(cfg_webhook);
        assert!(!n_webhook.has_bot());
        assert!(n_webhook.has_webhook());
        assert!(n_webhook.is_configured());

        // 3. Bot only
        let cfg_bot = LarkConfig {
            app_id: Some("cli_12345".into()),
            app_secret: Some("secret_67890".into()),
            default_chat_id: Some("oc_main".into()),
            ..Default::default()
        };
        let n_bot = LarkNotifier::new(cfg_bot);
        assert!(n_bot.has_bot());
        assert!(!n_bot.has_webhook());
        assert!(n_bot.is_configured());
    }

    #[test]
    fn test_lark_channel_resolution() {
        let cfg = LarkConfig {
            app_id: Some("cli_app".into()),
            app_secret: Some("sec_app".into()),
            default_chat_id: Some("oc_default_123".into()),
            channels: vec![
                LarkChannelConfig {
                    name: "devops".into(),
                    chat_id: "oc_devops_456".into(),
                },
                LarkChannelConfig {
                    name: "alerts".into(),
                    chat_id: "oc_alerts_789".into(),
                },
            ],
            ..Default::default()
        };
        let notifier = LarkNotifier::new(cfg);

        // Name match (case-insensitive)
        assert_eq!(
            notifier.resolve_chat_id(Some("devops")),
            Some("oc_devops_456".to_string())
        );
        assert_eq!(
            notifier.resolve_chat_id(Some("DEVOPS")),
            Some("oc_devops_456".to_string())
        );
        assert_eq!(
            notifier.resolve_chat_id(Some("alerts")),
            Some("oc_alerts_789".to_string())
        );

        // Direct chat_id passed
        assert_eq!(
            notifier.resolve_chat_id(Some("oc_custom_999")),
            Some("oc_custom_999".to_string())
        );

        // Fallback to default when None or unknown
        assert_eq!(
            notifier.resolve_chat_id(None),
            Some("oc_default_123".to_string())
        );
        assert_eq!(
            notifier.resolve_chat_id(Some("unknown_channel")),
            Some("oc_default_123".to_string())
        );

        // bot_channels returns default channel first followed by registered channels
        let bot_chans = notifier.bot_channels();
        assert_eq!(bot_chans.len(), 3);
        assert_eq!(bot_chans[0].name, "default");
        assert_eq!(bot_chans[0].chat_id, "oc_default_123");
        assert_eq!(bot_chans[1].name, "devops");
        assert_eq!(bot_chans[2].name, "alerts");
    }

    #[test]
    fn test_lark_interactive_card_structure() {
        let card = LarkNotifier::build_interactive_card(
            "Test Card",
            "This is a test notification message",
            Some("https://github.com/cn-starlabs/sugie"),
            "green",
            &[("Repository", "cn-starlabs/sugie"), ("Status", "Passing")],
        );

        let header = card.get("header").unwrap();
        assert_eq!(
            header.get("template").and_then(|v| v.as_str()),
            Some("green")
        );
        assert_eq!(
            header.get("title").and_then(|t| t.get("content")).and_then(|v| v.as_str()),
            Some("Test Card")
        );

        let elements = card
            .get("body")
            .and_then(|b| b.get("elements"))
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(elements.len(), 3); // Main text, fields, and button
    }

    #[test]
    fn test_lark_signature_generation() {
        let timestamp = 1700000000;
        let secret = "test_lark_secret";
        let sig = LarkNotifier::generate_signature(timestamp, secret).unwrap();
        assert!(!sig.is_empty());
        // Verify deterministic output
        let sig2 = LarkNotifier::generate_signature(timestamp, secret).unwrap();
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_lark_multiple_webhooks_and_fallback() {
        // 1. Single webhook fallback
        let cfg1 = LarkConfig {
            webhook_url: "https://open.larksuite.com/open-apis/bot/v2/hook/wh1".into(),
            secret: Some("sec1".into()),
            ..Default::default()
        };
        let n1 = LarkNotifier::new(cfg1);
        let whs1 = n1.webhook_channels();
        assert_eq!(whs1.len(), 1);
        assert_eq!(whs1[0].name, "default");
        assert_eq!(whs1[0].url, "https://open.larksuite.com/open-apis/bot/v2/hook/wh1");
        assert_eq!(whs1[0].secret.as_deref(), Some("sec1"));

        // 2. Multiple named webhooks
        let cfg2 = LarkConfig {
            webhooks: vec![
                LarkWebhookChannel {
                    name: "devops".into(),
                    url: "https://open.larksuite.com/open-apis/bot/v2/hook/wh-devops".into(),
                    secret: Some("sec-devops".into()),
                },
                LarkWebhookChannel {
                    name: "alerts".into(),
                    url: "https://open.larksuite.com/open-apis/bot/v2/hook/wh-alerts".into(),
                    secret: None,
                },
            ],
            ..Default::default()
        };
        let n2 = LarkNotifier::new(cfg2);
        let whs2 = n2.webhook_channels();
        assert_eq!(whs2.len(), 2);
        assert_eq!(whs2[0].name, "devops");
        assert_eq!(whs2[0].url, "https://open.larksuite.com/open-apis/bot/v2/hook/wh-devops");
        assert_eq!(whs2[0].secret.as_deref(), Some("sec-devops"));
        assert_eq!(whs2[1].name, "alerts");
        assert_eq!(whs2[1].secret, None);
    }

    #[test]
    fn test_lark_markdown_formatting_and_card_enhancements() {
        let input = "### 🤖 Sugie Bot Status\n\n\
- **Status**: 🟢 Running\n\
- **Version**: `v0.1.0`\n  \
- **Sub-item**: Value\n\
> Important notice for @ou_test123 and @all\n\n\
```toml\n\
# This is a comment\n\
admin_only = true\n\
```";

        let formatted = format_markdown_for_lark(input);

        // Heading converted to bold
        assert!(formatted.contains("**🤖 Sugie Bot Status**"));
        assert!(!formatted.contains("### 🤖 Sugie Bot Status"));

        // Bullets normalized to • with indentation
        assert!(formatted.contains("• **Status**: 🟢 Running"));
        assert!(formatted.contains("• **Version**: `v0.1.0`"));
        assert!(formatted.contains("  • **Sub-item**: Value"));

        // Blockquote converted to visual quote block
        assert!(formatted.contains("▎ Important notice for <at id=\"ou_test123\"></at> and <at id=all></at>"));

        // Code block preserves hash comment without converting to bold and maps toml to yaml
        assert!(formatted.contains("# This is a comment"));
        assert!(formatted.contains("```yaml"));

        // Test card title extraction from heading and hr dividers
        let markdown_with_divider = "### 🤖 Bot Digest\n\nFirst part\n\n---\n\nSecond part";
        let card = LarkNotifier::build_interactive_card(
            "🤖 Sugie Bot Response",
            markdown_with_divider,
            None,
            "blue",
            &[],
        );

        let header = card.get("header").unwrap();
        // Dynamic title extracted from heading
        assert_eq!(
            header.get("title").and_then(|t| t.get("content")).and_then(|v| v.as_str()),
            Some("🤖 Bot Digest")
        );

        let elements = card
            .get("body")
            .and_then(|b| b.get("elements"))
            .and_then(|v| v.as_array())
            .unwrap();
        // Two markdown sections and one hr divider between them
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0].get("tag").and_then(|v| v.as_str()), Some("markdown"));
        assert_eq!(elements[1].get("tag").and_then(|v| v.as_str()), Some("hr"));
        assert_eq!(elements[2].get("tag").and_then(|v| v.as_str()), Some("markdown"));
    }
}

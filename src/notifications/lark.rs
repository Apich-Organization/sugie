use crate::config::{LarkChannelConfig, LarkConfig};
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
            http: Client::new(),
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

    /// Whether an incoming webhook URL is configured.
    pub fn has_webhook(&self) -> bool {
        !self.config.webhook_url.trim().is_empty()
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
        let mut elements = Vec::new();

        // Main content
        elements.push(json!({
            "tag": "div",
            "text": {
                "tag": "lark_md",
                "content": message
            }
        }));

        // Key-value fields
        if !fields.is_empty() {
            let fields_md = fields
                .iter()
                .map(|(k, v)| format!("**{}**: {}", k, v))
                .collect::<Vec<_>>()
                .join("\n");
            elements.push(json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": fields_md
                }
            }));
        }

        // Action button for GitHub URL
        if let Some(target_url) = url {
            elements.push(json!({
                "tag": "action",
                "actions": [
                    {
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": "View on GitHub ↗"
                        },
                        "type": "primary",
                        "url": target_url
                    }
                ]
            }));
        }

        json!({
            "config": {
                "wide_screen_mode": true
            },
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": title
                },
                "template": color_template
            },
            "elements": elements
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

    /// Send notification via legacy/custom incoming webhook URL.
    pub async fn send_webhook_notification(
        &self,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        if self.config.webhook_url.trim().is_empty() {
            anyhow::bail!("Lark webhook URL is not configured");
        }

        let now = Utc::now().timestamp();
        let card = Self::build_interactive_card(title, message, url, color_template, fields);

        let mut payload = json!({
            "msg_type": "interactive",
            "card": card
        });

        if let Some(ref secret) = self.config.secret {
            if !secret.trim().is_empty() {
                let sign = Self::generate_signature(now, secret)?;
                payload["timestamp"] = json!(now.to_string());
                payload["sign"] = json!(sign);
            }
        }

        let started = std::time::Instant::now();
        let send_res = self
            .http
            .post(&self.config.webhook_url)
            .json(&payload)
            .send()
            .await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let resp = match send_res {
            Ok(r) => r,
            Err(e) => {
                crate::security_log::log_notification_request(
                    "lark",
                    "POST",
                    &self.config.webhook_url,
                    0,
                    duration_ms,
                    &format!("title={title} error={e}"),
                );
                return Err(e).context("Failed to send webhook request to Lark");
            }
        };

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            crate::security_log::log_notification_request(
                "lark",
                "POST",
                &self.config.webhook_url,
                status.as_u16(),
                duration_ms,
                &format!("title={title} http_error={body_text}"),
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
                        &self.config.webhook_url,
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
            &self.config.webhook_url,
            status.as_u16(),
            duration_ms,
            &format!("title={title} template={color_template}"),
        );

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

        let elements = card.get("elements").and_then(|v| v.as_array()).unwrap();
        assert_eq!(elements.len(), 3); // Main text, fields div, and button action
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
}

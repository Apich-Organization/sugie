use crate::config::LarkConfig;
use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::Client;
use serde_json::json;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct LarkNotifier {
    config: LarkConfig,
    http: Client,
}

impl LarkNotifier {
    pub fn new(config: LarkConfig) -> Self {
        Self {
            config,
            http: Client::new(),
        }
    }

    pub fn config(&self) -> &LarkConfig {
        &self.config
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

    pub async fn send_notification(
        &self,
        title: &str,
        message: &str,
        url: Option<&str>,
        color_template: &str,
        fields: &[(&str, &str)],
    ) -> Result<()> {
        let now = Utc::now().timestamp();

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

        let card = json!({
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": title
                },
                "template": color_template
            },
            "elements": elements
        });

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

        let resp = self
            .http
            .post(&self.config.webhook_url)
            .json(&payload)
            .send()
            .await
            .context("Failed to send webhook request to Lark")?;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            anyhow::bail!("Lark webhook HTTP error {}: {}", status, body_text);
        }

        if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(&body_text) {
            if let Some(code) = json_body.get("code").and_then(|c| c.as_i64()) {
                if code != 0 {
                    let msg = json_body
                        .get("msg")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    anyhow::bail!("Lark API rejected notification (code {}): {}", code, msg);
                }
            }
        }

        Ok(())
    }
}

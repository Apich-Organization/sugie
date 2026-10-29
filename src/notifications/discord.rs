use crate::config::DiscordConfig;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct DiscordEmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscordEmbed {
    pub title: String,
    pub description: String,
    pub url: Option<String>,
    pub color: u32,
    pub fields: Vec<DiscordEmbedField>,
    pub timestamp: String,
    pub footer: Option<DiscordFooter>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscordFooter {
    pub text: String,
}

#[derive(Clone)]
pub struct DiscordNotifier {
    config: DiscordConfig,
    http: Client,
}

impl DiscordNotifier {
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            http: Client::new(),
        }
    }

    pub fn config(&self) -> &DiscordConfig {
        &self.config
    }

    pub async fn send_embed(
        &self,
        title: &str,
        description: &str,
        url: Option<&str>,
        color: u32,
        fields: Vec<DiscordEmbedField>,
        is_report: bool,
    ) -> Result<()> {
        let target_url = if is_report {
            self.config
                .reports_webhook_url
                .as_ref()
                .unwrap_or(&self.config.webhook_url)
        } else {
            &self.config.webhook_url
        };

        let embed = DiscordEmbed {
            title: title.to_string(),
            description: description.to_string(),
            url: url.map(|s| s.to_string()),
            color,
            fields,
            timestamp: Utc::now().to_rfc3339(),
            footer: Some(DiscordFooter {
                text: "sugie bot".to_string(),
            }),
        };

        let mut payload = json!({
            "username": self.config.username,
            "embeds": [embed]
        });

        if let Some(ref avatar) = self.config.avatar_url {
            payload["avatar_url"] = json!(avatar);
        }

        let started = std::time::Instant::now();
        let send_res = self.http.post(target_url).json(&payload).send().await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let resp = match send_res {
            Ok(r) => r,
            Err(e) => {
                crate::security_log::log_notification_request(
                    "discord",
                    "POST",
                    target_url,
                    0,
                    duration_ms,
                    &format!("title={title} error={e}"),
                );
                return Err(e).context("Failed to send webhook to Discord");
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            crate::security_log::log_notification_request(
                "discord",
                "POST",
                target_url,
                status.as_u16(),
                duration_ms,
                &format!("title={title} error={body}"),
            );
            anyhow::bail!("Discord webhook failed with HTTP {}: {}", status, body);
        }

        crate::security_log::log_notification_request(
            "discord",
            "POST",
            target_url,
            status.as_u16(),
            duration_ms,
            &format!("title={title} is_report={is_report}"),
        );

        Ok(())
    }
}

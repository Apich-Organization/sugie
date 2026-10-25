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

    pub async fn send_embed(
        &self,
        title: &str,
        description: &str,
        url: Option<&str>,
        color: u32,
        fields: Vec<DiscordEmbedField>,
    ) -> Result<()> {
        let embed = DiscordEmbed {
            title: title.to_string(),
            description: description.to_string(),
            url: url.map(|s| s.to_string()),
            color,
            fields,
            timestamp: Utc::now().to_rfc3339(),
            footer: Some(DiscordFooter {
                text: "sugie triage bot".to_string(),
            }),
        };

        let mut payload = json!({
            "username": self.config.username,
            "embeds": [embed]
        });

        if let Some(ref avatar) = self.config.avatar_url {
            payload["avatar_url"] = json!(avatar);
        }

        let resp = self
            .http
            .post(&self.config.webhook_url)
            .json(&payload)
            .send()
            .await
            .context("Failed to send webhook to Discord")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Discord webhook failed with HTTP {}: {}", status, body);
        }

        Ok(())
    }
}

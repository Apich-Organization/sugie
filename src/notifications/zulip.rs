use crate::config::ZulipConfig;
use anyhow::{Context, Result};
use reqwest::Client;

#[derive(Clone)]
pub struct ZulipNotifier {
    config: ZulipConfig,
    http: Client,
}

impl ZulipNotifier {
    pub fn new(config: ZulipConfig) -> Self {
        Self {
            config,
            http: Client::new(),
        }
    }

    pub async fn send_message(&self, topic_override: Option<&str>, content: &str) -> Result<()> {
        let topic = topic_override.unwrap_or(&self.config.topic);
        let endpoint = format!("{}/api/v1/messages", self.config.site_url.trim_end_matches('/'));

        let params = [
            ("type", "stream"),
            ("to", &self.config.stream),
            ("topic", topic),
            ("content", content),
        ];

        let resp = self
            .http
            .post(&endpoint)
            .basic_auth(&self.config.bot_email, Some(&self.config.bot_api_key))
            .form(&params)
            .send()
            .await
            .context("Failed to send message to Zulip API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Zulip API request failed with HTTP {}: {}", status, body);
        }

        Ok(())
    }
}

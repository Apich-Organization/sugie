use crate::config::ZulipConfig;
use anyhow::{Context, Result};
use reqwest::Client;

#[derive(Clone)]
pub struct ZulipNotifier {
    config: ZulipConfig,
    http: Client,
}

#[derive(Debug, Clone, Copy)]
pub enum ZulipTopicCategory {
    Triage,
    Reports,
    Alerts,
    Assignments,
    Custom(&'static str),
}

impl ZulipNotifier {
    pub fn new(config: ZulipConfig) -> Self {
        Self {
            config,
            http: Client::new(),
        }
    }

    pub async fn send_message(&self, category: ZulipTopicCategory, content: &str) -> Result<()> {
        let topic = match category {
            ZulipTopicCategory::Triage => &self.config.topic_triage,
            ZulipTopicCategory::Reports => &self.config.topic_reports,
            ZulipTopicCategory::Alerts => &self.config.topic_alerts,
            ZulipTopicCategory::Assignments => &self.config.topic_assigns,
            ZulipTopicCategory::Custom(c) => c,
        };

        let endpoint = format!(
            "{}/api/v1/messages",
            self.config.site_url.trim_end_matches('/')
        );

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

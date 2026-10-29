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

    pub fn config(&self) -> &ZulipConfig {
        &self.config
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

        let started = std::time::Instant::now();
        let send_res = self
            .http
            .post(&endpoint)
            .basic_auth(&self.config.bot_email, Some(&self.config.bot_api_key))
            .form(&params)
            .send()
            .await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let resp = match send_res {
            Ok(r) => r,
            Err(e) => {
                crate::security_log::log_notification_request(
                    "zulip",
                    "POST",
                    &endpoint,
                    0,
                    duration_ms,
                    &format!("stream={} topic={topic} error={e}", self.config.stream),
                );
                return Err(e).context("Failed to send message to Zulip API");
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            crate::security_log::log_notification_request(
                "zulip",
                "POST",
                &endpoint,
                status.as_u16(),
                duration_ms,
                &format!("stream={} topic={topic} error={body}", self.config.stream),
            );
            anyhow::bail!("Zulip API request failed with HTTP {}: {}", status, body);
        }

        crate::security_log::log_notification_request(
            "zulip",
            "POST",
            &endpoint,
            status.as_u16(),
            duration_ms,
            &format!("stream={} topic={topic}", self.config.stream),
        );

        Ok(())
    }
}

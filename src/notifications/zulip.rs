use crate::config::ZulipConfig;
use anyhow::{Context, Result};
use reqwest::Client;

pub use crate::config::ZulipChannelConfig;

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

    /// Return all configured Zulip channel destinations.
    /// If none are explicitly configured, returns the default stream and alerts topic.
    pub fn channels(&self) -> Vec<ZulipChannelConfig> {
        if !self.config.channels.is_empty() {
            self.config.channels.clone()
        } else if !self.config.stream.is_empty() {
            vec![ZulipChannelConfig {
                name: "default".to_string(),
                channel: self.config.stream.clone(),
                topic: self.config.topic_alerts.clone(),
            }]
        } else {
            Vec::new()
        }
    }

    /// Send a message to a specific Zulip channel (stream) and topic.
    pub async fn send_to_channel(&self, channel: &str, topic: &str, content: &str) -> Result<()> {
        let endpoint = format!(
            "{}/api/v1/messages",
            self.config.site_url.trim_end_matches('/')
        );

        let params = [
            ("type", "stream"),
            ("to", channel),
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
                    &format!("channel={channel} topic={topic} error={e}"),
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
                &format!("channel={channel} topic={topic} error={body}"),
            );
            anyhow::bail!("Zulip API request failed with HTTP {}: {}", status, body);
        }

        crate::security_log::log_notification_request(
            "zulip",
            "POST",
            &endpoint,
            status.as_u16(),
            duration_ms,
            &format!("channel={channel} topic={topic}"),
        );

        Ok(())
    }

    pub async fn send_message(&self, category: ZulipTopicCategory, content: &str) -> Result<()> {
        let topic = match category {
            ZulipTopicCategory::Triage => &self.config.topic_triage,
            ZulipTopicCategory::Reports => &self.config.topic_reports,
            ZulipTopicCategory::Alerts => &self.config.topic_alerts,
            ZulipTopicCategory::Assignments => &self.config.topic_assigns,
            ZulipTopicCategory::Custom(c) => c,
        };

        self.send_to_channel(&self.config.stream, topic, content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zulip_channels_fallback() {
        let config = ZulipConfig {
            stream: "general".to_string(),
            topic_alerts: "urgent".to_string(),
            ..Default::default()
        };
        let notifier = ZulipNotifier::new(config);
        let channels = notifier.channels();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "default");
        assert_eq!(channels[0].channel, "general");
        assert_eq!(channels[0].topic, "urgent");
    }

    #[test]
    fn test_zulip_explicit_channels() {
        let config = ZulipConfig {
            channels: vec![
                ZulipChannelConfig {
                    name: "releases".to_string(),
                    channel: "deployments".to_string(),
                    topic: "prod".to_string(),
                },
                ZulipChannelConfig {
                    name: "dev".to_string(),
                    channel: "engineering".to_string(),
                    topic: "ci".to_string(),
                },
            ],
            ..Default::default()
        };
        let notifier = ZulipNotifier::new(config);
        let channels = notifier.channels();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].name, "releases");
        assert_eq!(channels[0].channel, "deployments");
        assert_eq!(channels[0].topic, "prod");
        assert_eq!(channels[1].name, "dev");
        assert_eq!(channels[1].channel, "engineering");
        assert_eq!(channels[1].topic, "ci");
    }

    #[test]
    fn test_zulip_toml_channel_alias() {
        let toml_str = r#"
            site_url = "https://zulip.example.com"
            bot_email = "bot@example.com"
            bot_api_key = "secret"
            channel = "my-channel"

            [[channels]]
            name = "alerts"
            channel = "ops"
            topic = "incidents"

            [[channels]]
            name = "chat"
            stream = "watercooler"
            topic = "general"
        "#;
        let config: ZulipConfig = toml::from_str(toml_str).expect("deserialize zulip config");
        assert_eq!(config.stream, "my-channel");
        assert_eq!(config.channels.len(), 2);
        assert_eq!(config.channels[0].channel, "ops");
        assert_eq!(config.channels[0].topic, "incidents");
        assert_eq!(config.channels[1].channel, "watercooler");
        assert_eq!(config.channels[1].topic, "general");
    }
}

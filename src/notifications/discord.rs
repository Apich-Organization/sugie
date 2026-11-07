use crate::config::DiscordConfig;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use serde::Serialize;
use serde_json::json;

pub use crate::config::{DiscordBotChannel, DiscordWebhookChannel};

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

/// Discord REST base. Bot messages go to `/channels/{id}/messages`; webhooks
/// have their own URL and are posted as-is.
const DISCORD_API: &str = "https://discord.com/api/v10";

/// Where a message is going.
#[derive(Debug, PartialEq, Eq)]
enum DiscordTarget {
    /// Post as the bot to a channel it has access to.
    Channel(String),
    /// Post to a channel webhook.
    Webhook(String),
}

impl DiscordNotifier {
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            http: crate::notifications::create_notification_http_client(),
        }
    }

    pub fn config(&self) -> &DiscordConfig {
        &self.config
    }

    /// Whether this configuration can send anything at all.
    ///
    /// A token without a channel id still counts as configured: the send then
    /// fails with an explicit message instead of the channel going quiet.
    pub fn is_configured(&self) -> bool {
        self.has_webhook() || self.has_bot()
    }

    pub fn has_webhook(&self) -> bool {
        !self.webhook_channels().is_empty()
    }

    pub fn has_bot(&self) -> bool {
        self.config
            .bot_token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    }

    pub fn webhook_channels(&self) -> Vec<DiscordWebhookChannel> {
        if !self.config.webhooks.is_empty() {
            return self.config.webhooks.clone();
        }
        let mut list = Vec::new();
        if !self.config.webhook_url.trim().is_empty() {
            list.push(DiscordWebhookChannel {
                name: "default".to_string(),
                url: self.config.webhook_url.trim().to_string(),
            });
        }
        if let Some(ref r_url) = self.config.reports_webhook_url {
            let r_url = r_url.trim();
            if !r_url.is_empty() && !list.iter().any(|w| w.url == r_url) {
                list.push(DiscordWebhookChannel {
                    name: "reports".to_string(),
                    url: r_url.to_string(),
                });
            }
        }
        list
    }

    pub fn bot_channels(&self) -> Vec<DiscordBotChannel> {
        if !self.config.bot_channels.is_empty() {
            return self.config.bot_channels.clone();
        }
        let mut list = Vec::new();
        if let Some(ref ch) = self.config.channel_id {
            let ch = ch.trim();
            if !ch.is_empty() {
                list.push(DiscordBotChannel {
                    name: "default".to_string(),
                    channel_id: ch.to_string(),
                });
            }
        }
        if let Some(ref r_ch) = self.config.reports_channel_id {
            let r_ch = r_ch.trim();
            if !r_ch.is_empty() && !list.iter().any(|c| c.channel_id == r_ch) {
                list.push(DiscordBotChannel {
                    name: "reports".to_string(),
                    channel_id: r_ch.to_string(),
                });
            }
        }
        list
    }

    /// Pick the destination for this message.
    ///
    /// A bot token takes precedence: one credential covers every channel the bot
    /// has been invited to, instead of one webhook URL per channel. Webhooks stay
    /// supported so existing configurations keep working unchanged.
    fn target(&self, is_report: bool) -> Result<DiscordTarget> {
        let has_token = self
            .config
            .bot_token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty());

        if has_token {
            let channel = if is_report {
                self.config
                    .reports_channel_id
                    .as_deref()
                    .or(self.config.channel_id.as_deref())
                    .or_else(|| self.config.bot_channels.iter().find(|c| c.name == "reports" || c.name == "default").map(|c| c.channel_id.as_str()))
                    .or_else(|| self.config.bot_channels.first().map(|c| c.channel_id.as_str()))
            } else {
                self.config
                    .channel_id
                    .as_deref()
                    .or_else(|| self.config.bot_channels.iter().find(|c| c.name == "default").map(|c| c.channel_id.as_str()))
                    .or_else(|| self.config.bot_channels.first().map(|c| c.channel_id.as_str()))
            };
            return match channel.map(str::trim).filter(|c| !c.is_empty()) {
                Some(id) => Ok(DiscordTarget::Channel(id.to_string())),
                None => anyhow::bail!(
                    "Discord bot token is set but channel_id is missing; \
                     add channel_id (or reports_channel_id) to [notifications.discord]"
                ),
            };
        }

        let url = if is_report {
            self.config
                .reports_webhook_url
                .as_deref()
                .filter(|u| !u.trim().is_empty())
                .or_else(|| self.config.webhooks.iter().find(|w| w.name == "reports" || w.name == "default").map(|w| w.url.as_str()))
                .or_else(|| self.config.webhooks.first().map(|w| w.url.as_str()))
                .unwrap_or(&self.config.webhook_url)
        } else if !self.config.webhook_url.trim().is_empty() {
            &self.config.webhook_url
        } else if let Some(first) = self.config.webhooks.iter().find(|w| w.name == "default").or_else(|| self.config.webhooks.first()) {
            &first.url
        } else {
            ""
        };

        if url.trim().is_empty() {
            anyhow::bail!(
                "Discord is not configured: set SUGIE_DISCORD_BOT_TOKEN with a channel_id, \
                 or SUGIE_DISCORD_WEBHOOK_URL"
            );
        }
        Ok(DiscordTarget::Webhook(url.to_string()))
    }

    fn create_embed(
        &self,
        title: &str,
        description: &str,
        url: Option<&str>,
        color: u32,
        fields: Vec<DiscordEmbedField>,
    ) -> DiscordEmbed {
        DiscordEmbed {
            title: title.to_string(),
            description: description.to_string(),
            url: url.map(|s| s.to_string()),
            color,
            fields,
            timestamp: Utc::now().to_rfc3339(),
            footer: Some(DiscordFooter {
                text: "sugie bot".to_string(),
            }),
        }
    }

    async fn post_json(
        &self,
        target_url: &str,
        payload: &serde_json::Value,
        auth: Option<&str>,
        title: &str,
        is_report: bool,
    ) -> Result<()> {
        if !crate::notifications::is_safe_webhook_url(target_url) {
            anyhow::bail!("Blocked unsafe or non-HTTPS webhook destination (SSRF protection)");
        }

        let mut request = self.http.post(target_url).json(payload);
        if let Some(auth) = auth {
            request = request.header(reqwest::header::AUTHORIZATION, auth);
        }

        let started = std::time::Instant::now();
        let send_res = request.send().await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let resp = match send_res {
            Ok(r) => r,
            Err(e) => {
                let safe_err = crate::notifications::sanitize_error_str(&e);
                crate::security_log::log_notification_request(
                    "discord",
                    "POST",
                    target_url,
                    0,
                    duration_ms,
                    &format!("title={title} error={safe_err}"),
                );
                return Err(e).context("Failed to send webhook to Discord");
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let safe_body = crate::notifications::sanitize_error_str(&body);
            crate::security_log::log_notification_request(
                "discord",
                "POST",
                target_url,
                status.as_u16(),
                duration_ms,
                &format!("title={title} error={safe_body}"),
            );
            let hint = match status.as_u16() {
                401 => " (bot token rejected)",
                403 => " (the bot cannot post in that channel — check it is a member and has Send Messages)",
                404 => " (unknown channel id)",
                _ => "",
            };
            anyhow::bail!("Discord send failed with HTTP {}{hint}: {}", status, body);
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

    pub async fn send_to_webhook(
        &self,
        webhook_url: &str,
        title: &str,
        description: &str,
        url: Option<&str>,
        color: u32,
        fields: Vec<DiscordEmbedField>,
    ) -> Result<()> {
        let embed = self.create_embed(title, description, url, color, fields);
        let mut payload = json!({
            "username": self.config.username,
            "embeds": [embed]
        });
        if let Some(ref avatar) = self.config.avatar_url {
            payload["avatar_url"] = json!(avatar);
        }
        self.post_json(webhook_url, &payload, None, title, false).await
    }

    pub async fn send_to_channel(
        &self,
        channel_id: &str,
        title: &str,
        description: &str,
        url: Option<&str>,
        color: u32,
        fields: Vec<DiscordEmbedField>,
    ) -> Result<()> {
        let token = self
            .config
            .bot_token
            .as_deref()
            .context("Discord bot_token is not configured")?;
        let embed = self.create_embed(title, description, url, color, fields);
        let target_url = format!("{DISCORD_API}/channels/{channel_id}/messages");
        let payload = json!({ "embeds": [embed] });
        let auth = format!("Bot {}", token.trim());
        self.post_json(&target_url, &payload, Some(&auth), title, false).await
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
        match self.target(is_report)? {
            DiscordTarget::Channel(id) => {
                self.send_to_channel(&id, title, description, url, color, fields).await
            }
            DiscordTarget::Webhook(url_str) => {
                self.send_to_webhook(&url_str, title, description, url, color, fields).await
            }
        }
    }

    /// Register/publish Discord slash commands via the REST API globally and for all joined servers.
    pub async fn sync_commands(&self) -> Result<String> {
        let token = self
            .config
            .bot_token
            .as_deref()
            .context("SUGIE_DISCORD_BOT_TOKEN is not configured")?;
        let auth = format!("Bot {}", token.trim());
        let http = &self.http;

        // 1. Get Application ID from Discord
        let app_resp = http
            .get(format!("{DISCORD_API}/oauth2/applications/@me"))
            .header(reqwest::header::AUTHORIZATION, &auth)
            .send()
            .await
            .context("Failed to connect to Discord API")?;

        if !app_resp.status().is_success() {
            let body = app_resp.text().await.unwrap_or_default();
            anyhow::bail!("Discord application lookup failed: {body}");
        }

        let app_info: serde_json::Value = app_resp.json().await?;
        let app_id = app_info["id"]
            .as_str()
            .context("Missing application ID in Discord response")?;

        // 2. Command list payload
        let commands_payload = json!([
            {"name": "help", "description": "Show available bot commands and documentation"},
            {"name": "status", "description": "Show triage bot status and repository sync state"},
            {"name": "repos", "description": "List all configured repositories"},
            {"name": "issues", "description": "List open issues", "options": [{"name": "repo", "description": "Repository name (e.g. StarLabsSI/bincode)", "type": 3, "required": false}]},
            {"name": "prs", "description": "List open pull requests", "options": [{"name": "repo", "description": "Repository name (e.g. StarLabsSI/bincode)", "type": 3, "required": false}]},
            {"name": "metrics", "description": "Display 7-day issue & PR triage metrics", "options": [{"name": "repo", "description": "Repository name", "type": 3, "required": false}]},
            {"name": "stats", "description": "Issue, PR, and commit statistics", "options": [{"name": "repo", "description": "Repository name", "type": 3, "required": false}]},
            {"name": "workload", "description": "Display reviewer active review workloads", "options": [{"name": "repo", "description": "Repository name", "type": 3, "required": false}]},
            {"name": "users", "description": "List eligible GitHub assignees & reviewer pool", "options": [{"name": "repo", "description": "Repository name", "type": 3, "required": false}]},
            {"name": "config", "description": "Show active bot configuration summary"}
        ]);

        // 3. Register global commands
        let global_resp = http
            .put(format!("{DISCORD_API}/applications/{app_id}/commands"))
            .header(reqwest::header::AUTHORIZATION, &auth)
            .json(&commands_payload)
            .send()
            .await
            .context("Failed to publish global Discord commands")?;

        if !global_resp.status().is_success() {
            let body = global_resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to register global Discord commands: {body}");
        }

        // 4. Register guild commands for instant availability on each server
        let mut guild_count = 0usize;
        if let Ok(guilds_resp) = http
            .get(format!("{DISCORD_API}/users/@me/guilds"))
            .header(reqwest::header::AUTHORIZATION, &auth)
            .send()
            .await
        {
            if guilds_resp.status().is_success() {
                if let Ok(guilds) = guilds_resp.json::<Vec<serde_json::Value>>().await {
                    for g in guilds {
                        if let Some(gid) = g["id"].as_str() {
                            let _ = http
                                .put(format!("{DISCORD_API}/applications/{app_id}/guilds/{gid}/commands"))
                                .header(reqwest::header::AUTHORIZATION, &auth)
                                .json(&commands_payload)
                                .send()
                                .await;
                            guild_count += 1;
                        }
                    }
                }
            }
        }

        Ok(format!("Synced 10 commands globally and to {guild_count} server(s)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(webhook: &str, token: Option<&str>, channel: Option<&str>) -> DiscordConfig {
        DiscordConfig {
            webhook_url: webhook.to_string(),
            bot_token: token.map(str::to_string),
            channel_id: channel.map(str::to_string),
            ..Default::default()
        }
    }

    /// A token covers every channel, so it wins over a leftover webhook.
    #[test]
    fn bot_token_takes_precedence_over_webhook() {
        let notifier = DiscordNotifier::new(cfg(
            "https://discord.com/api/webhooks/x/y",
            Some("tok"),
            Some("42"),
        ));
        assert_eq!(
            notifier.target(false).expect("target"),
            DiscordTarget::Channel("42".to_string())
        );
    }

    /// Reports go to their own channel when one is configured, and fall back to
    /// the main channel otherwise.
    #[test]
    fn reports_use_their_own_channel_then_fall_back() {
        let mut config = cfg("", Some("tok"), Some("42"));
        let notifier = DiscordNotifier::new(config.clone());
        assert_eq!(
            notifier.target(true).expect("target"),
            DiscordTarget::Channel("42".to_string())
        );

        config.reports_channel_id = Some("99".to_string());
        let notifier = DiscordNotifier::new(config);
        assert_eq!(
            notifier.target(true).expect("target"),
            DiscordTarget::Channel("99".to_string())
        );
    }

    /// Without a token the webhook is still used, unchanged.
    #[test]
    fn webhook_mode_still_works() {
        let notifier =
            DiscordNotifier::new(cfg("https://discord.com/api/webhooks/x/y", None, None));
        assert_eq!(
            notifier.target(false).expect("target"),
            DiscordTarget::Webhook("https://discord.com/api/webhooks/x/y".to_string())
        );
    }

    /// A token without a channel is a misconfiguration and must say so rather
    /// than silently falling back or posting nowhere.
    #[test]
    fn token_without_channel_is_an_error() {
        let notifier = DiscordNotifier::new(cfg("", Some("tok"), None));
        let err = notifier.target(false).expect_err("must fail");
        assert!(
            err.to_string().contains("channel_id"),
            "unexpected error: {err}"
        );
    }

    /// An empty section is not "configured", so events are not attempted at all.
    #[test]
    fn empty_configuration_is_not_configured() {
        assert!(!DiscordNotifier::new(DiscordConfig::default()).is_configured());
        assert!(DiscordNotifier::new(cfg("", Some("tok"), None)).is_configured());
        assert!(DiscordNotifier::new(cfg("https://x", None, None)).is_configured());
    }

    #[test]
    fn multiple_webhooks_and_bot_channels_registered() {
        let config = DiscordConfig {
            bot_token: Some("my-token".to_string()),
            webhooks: vec![
                DiscordWebhookChannel {
                    name: "alerts".to_string(),
                    url: "https://discord.com/api/webhooks/1/a".to_string(),
                },
                DiscordWebhookChannel {
                    name: "releases".to_string(),
                    url: "https://discord.com/api/webhooks/2/b".to_string(),
                },
            ],
            bot_channels: vec![
                DiscordBotChannel {
                    name: "general".to_string(),
                    channel_id: "1001".to_string(),
                },
                DiscordBotChannel {
                    name: "dev".to_string(),
                    channel_id: "1002".to_string(),
                },
            ],
            ..Default::default()
        };
        let notifier = DiscordNotifier::new(config);
        assert!(notifier.has_webhook());
        assert!(notifier.has_bot());
        assert_eq!(notifier.webhook_channels().len(), 2);
        assert_eq!(notifier.bot_channels().len(), 2);
        assert_eq!(notifier.bot_channels()[0].name, "general");
        assert_eq!(notifier.bot_channels()[0].channel_id, "1001");
        assert_eq!(notifier.webhook_channels()[1].name, "releases");
    }

    #[test]
    fn fallback_to_default_webhook_and_channel() {
        let config = DiscordConfig {
            webhook_url: "https://discord.com/api/webhooks/fallback".to_string(),
            channel_id: Some("8888".to_string()),
            bot_token: Some("bot-token".to_string()),
            ..Default::default()
        };
        let notifier = DiscordNotifier::new(config);
        let webhooks = notifier.webhook_channels();
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0].name, "default");
        assert_eq!(webhooks[0].url, "https://discord.com/api/webhooks/fallback");

        let bot_chans = notifier.bot_channels();
        assert_eq!(bot_chans.len(), 1);
        assert_eq!(bot_chans[0].name, "default");
        assert_eq!(bot_chans[0].channel_id, "8888");
    }
}

pub mod discord;
pub mod zulip;

pub use discord::{DiscordEmbedField, DiscordNotifier};
pub use zulip::ZulipNotifier;

use crate::config::NotificationsConfig;
use tracing::{error, info};

#[derive(Clone)]
pub struct NotificationDispatcher {
    discord: Option<DiscordNotifier>,
    zulip: Option<ZulipNotifier>,
}

#[derive(Debug, Clone, Copy)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl NotificationLevel {
    pub fn discord_color(&self) -> u32 {
        match self {
            NotificationLevel::Info => 0x3498db,    // Blue
            NotificationLevel::Success => 0x2ecc71, // Green
            NotificationLevel::Warning => 0xf39c12, // Orange/Amber
            NotificationLevel::Error => 0xe74c3c,   // Red
        }
    }
}

impl NotificationDispatcher {
    pub fn new(config: &NotificationsConfig) -> Self {
        let discord = config.discord.as_ref().map(|c| DiscordNotifier::new(c.clone()));
        let zulip = config.zulip.as_ref().map(|c| ZulipNotifier::new(c.clone()));
        Self { discord, zulip }
    }

    pub async fn dispatch(
        &self,
        title: &str,
        message: &str,
        url: Option<&str>,
        level: NotificationLevel,
        fields: Vec<DiscordEmbedField>,
    ) {
        if let Some(ref discord) = self.discord {
            if let Err(e) = discord
                .send_embed(title, message, url, level.discord_color(), fields)
                .await
            {
                error!("Failed to dispatch Discord notification: {:?}", e);
            } else {
                info!("Dispatched Discord notification: {}", title);
            }
        }

        if let Some(ref zulip) = self.zulip {
            let zulip_text = if let Some(u) = url {
                format!("### [{}]({})\n\n{}", title, u, message)
            } else {
                format!("### {}\n\n{}", title, message)
            };
            if let Err(e) = zulip.send_message(None, &zulip_text).await {
                error!("Failed to dispatch Zulip notification: {:?}", e);
            } else {
                info!("Dispatched Zulip notification: {}", title);
            }
        }
    }
}

pub mod discord;
pub mod zulip;

pub use discord::{DiscordEmbedField, DiscordNotifier};
pub use zulip::{ZulipNotifier, ZulipTopicCategory};

use crate::config::NotificationsConfig;
use tracing::{error, info};

#[derive(Clone)]
pub struct NotificationDispatcher {
    discord: Option<DiscordNotifier>,
    zulip: Option<ZulipNotifier>,
    config: NotificationsConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SugieEventType {
    WeeklyReport,
    MonthlyReport,
    Assigned,
    Unassigned,
    IssueOpened,
    IssueClosed,
    PrOpened,
    PrMerged,
    LabelChanged,
    StaleWarned,
    StaleClosed,
    CiFailure,
    DuplicateDetected,
    DcoFailed,
    GrowthMilestone,
    CrossRepoMove,
}

impl SugieEventType {
    pub fn is_enabled(&self, cfg: &NotificationsConfig) -> bool {
        match self {
            SugieEventType::WeeklyReport => cfg.events.weekly_report,
            SugieEventType::MonthlyReport => cfg.events.monthly_report,
            SugieEventType::Assigned => cfg.events.assigned,
            SugieEventType::Unassigned => cfg.events.unassigned,
            SugieEventType::IssueOpened => cfg.events.issue_opened,
            SugieEventType::IssueClosed => cfg.events.issue_closed,
            SugieEventType::PrOpened => cfg.events.pr_opened,
            SugieEventType::PrMerged => cfg.events.pr_merged,
            SugieEventType::LabelChanged => cfg.events.label_changed,
            SugieEventType::StaleWarned => cfg.events.stale_warned,
            SugieEventType::StaleClosed => cfg.events.stale_closed,
            SugieEventType::CiFailure => cfg.events.ci_failure,
            SugieEventType::DuplicateDetected => cfg.events.duplicate_detected,
            SugieEventType::DcoFailed => cfg.events.dco_failed,
            SugieEventType::GrowthMilestone => cfg.events.growth_milestone,
            SugieEventType::CrossRepoMove => cfg.events.cross_repo_move,
        }
    }

    pub fn zulip_category(&self) -> ZulipTopicCategory {
        match self {
            SugieEventType::WeeklyReport | SugieEventType::MonthlyReport => {
                ZulipTopicCategory::Reports
            }
            SugieEventType::Assigned | SugieEventType::Unassigned => {
                ZulipTopicCategory::Assignments
            }
            SugieEventType::StaleWarned
            | SugieEventType::StaleClosed
            | SugieEventType::CiFailure
            | SugieEventType::DcoFailed => ZulipTopicCategory::Alerts,
            _ => ZulipTopicCategory::Triage,
        }
    }

    pub fn is_report(&self) -> bool {
        matches!(
            self,
            SugieEventType::WeeklyReport | SugieEventType::MonthlyReport
        )
    }
}

impl NotificationDispatcher {
    pub fn new(config: &NotificationsConfig) -> Self {
        let discord = config
            .discord
            .as_ref()
            .map(|c| DiscordNotifier::new(c.clone()));
        let zulip = config.zulip.as_ref().map(|c| ZulipNotifier::new(c.clone()));
        Self {
            discord,
            zulip,
            config: config.clone(),
        }
    }

    pub async fn dispatch_event(
        &self,
        event_type: SugieEventType,
        title: &str,
        message: &str,
        url: Option<&str>,
        level: NotificationLevel,
        fields: Vec<DiscordEmbedField>,
    ) {
        if !event_type.is_enabled(&self.config) {
            return;
        }

        if let Some(ref discord) = self.discord {
            if let Err(e) = discord
                .send_embed(
                    title,
                    message,
                    url,
                    level.discord_color(),
                    fields,
                    event_type.is_report(),
                )
                .await
            {
                error!("Failed to dispatch Discord notification: {:?}", e);
            } else {
                info!("Dispatched Discord notification for {:?}", event_type);
            }
        }

        if let Some(ref zulip) = self.zulip {
            let zulip_text = if let Some(u) = url {
                format!("### [{}]({})\n\n{}", title, u, message)
            } else {
                format!("### {}\n\n{}", title, message)
            };
            if let Err(e) = zulip
                .send_message(event_type.zulip_category(), &zulip_text)
                .await
            {
                error!("Failed to dispatch Zulip notification: {:?}", e);
            } else {
                info!("Dispatched Zulip notification for {:?}", event_type);
            }
        }
    }
}

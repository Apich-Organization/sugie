pub mod discord;
pub mod lark;
pub mod zulip;

pub use discord::{DiscordEmbedField, DiscordNotifier};
pub use lark::LarkNotifier;
pub use zulip::{ZulipNotifier, ZulipTopicCategory};

use crate::config::NotificationsConfig;
use tracing::{error, info};

#[derive(Clone)]
pub struct NotificationDispatcher {
    discord: Option<DiscordNotifier>,
    zulip: Option<ZulipNotifier>,
    lark: Option<LarkNotifier>,
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

    pub fn lark_template(&self) -> &'static str {
        match self {
            NotificationLevel::Info => "blue",
            NotificationLevel::Success => "green",
            NotificationLevel::Warning => "orange",
            NotificationLevel::Error => "red",
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
    IssueComment,
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
            SugieEventType::IssueComment => cfg.events.issue_comment,
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

pub fn extract_repo_from_context(title: &str, url: Option<&str>) -> Option<String> {
    if let Some(u) = url {
        let trimmed = u.trim();
        if let Some(after) = trimmed
            .strip_prefix("https://github.com/")
            .or_else(|| trimmed.strip_prefix("http://github.com/"))
        {
            let parts: Vec<&str> = after.split('/').collect();
            if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                return Some(format!("{}/{}", parts[0], parts[1]));
            }
        }
    }

    // Try extracting from title if formatted like "New Issue: owner/repo#123" or "... digest for `owner/repo`"
    for word in title.split_whitespace() {
        let clean = word.trim_matches(|c: char| {
            c == '`' || c == '(' || c == ')' || c == ':' || c == '*' || c == '#'
        });
        if let Some((owner, repo_and_rest)) = clean.split_once('/') {
            let repo = repo_and_rest.split('#').next().unwrap_or(repo_and_rest);
            if !owner.is_empty() && !repo.is_empty() && !owner.contains('/') && !repo.contains('/')
            {
                return Some(format!("{}/{}", owner, repo));
            }
        }
    }

    None
}

pub fn is_repo_allowed(
    channel_repos: Option<&Vec<String>>,
    global_repos: Option<&Vec<String>>,
    repo: Option<&str>,
) -> bool {
    let repo = match repo {
        Some(r) => r.trim(),
        None => return true, // Test notifications without repo are always allowed
    };

    // 1. Channel-level filter has priority if configured
    if let Some(list) = channel_repos {
        return list
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(repo));
    }

    // 2. Global-level filter applies if configured
    if let Some(list) = global_repos {
        return list
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(repo));
    }

    // 3. Otherwise allowed by default
    true
}

impl NotificationDispatcher {
    pub fn new(config: &NotificationsConfig) -> Self {
        let discord = config
            .discord
            .as_ref()
            .map(|c| DiscordNotifier::new(c.clone()));
        let zulip = config.zulip.as_ref().map(|c| ZulipNotifier::new(c.clone()));
        let lark = config.lark.as_ref().map(|c| LarkNotifier::new(c.clone()));
        Self {
            discord,
            zulip,
            lark,
            config: config.clone(),
        }
    }

    pub fn discord(&self) -> Option<&DiscordNotifier> {
        self.discord.as_ref()
    }

    pub fn zulip(&self) -> Option<&ZulipNotifier> {
        self.zulip.as_ref()
    }

    pub fn lark(&self) -> Option<&LarkNotifier> {
        self.lark.as_ref()
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

        let event_repo = extract_repo_from_context(title, url);

        if let Some(ref discord) = self.discord {
            if is_repo_allowed(
                discord.config().allowed_repositories.as_ref(),
                self.config.allowed_repositories.as_ref(),
                event_repo.as_deref(),
            ) {
                if let Err(e) = discord
                    .send_embed(
                        title,
                        message,
                        url,
                        level.discord_color(),
                        fields.clone(),
                        event_type.is_report(),
                    )
                    .await
                {
                    error!("Failed to dispatch Discord notification: {:?}", e);
                } else {
                    info!("Dispatched Discord notification for {:?}", event_type);
                }
            } else {
                info!(
                    "Skipping Discord notification for repo '{:?}' (not in allowed list)",
                    event_repo
                );
            }
        }

        if let Some(ref zulip) = self.zulip {
            if is_repo_allowed(
                zulip.config().allowed_repositories.as_ref(),
                self.config.allowed_repositories.as_ref(),
                event_repo.as_deref(),
            ) {
                let mut zulip_text = if let Some(u) = url {
                    format!("### [{}]({})\n\n{}", title, u, message)
                } else {
                    format!("### {}\n\n{}", title, message)
                };

                if !fields.is_empty() {
                    zulip_text.push_str("\n\n");
                    for f in &fields {
                        zulip_text.push_str(&format!("* **{}**: {}\n", f.name, f.value));
                    }
                }

                if let Err(e) = zulip
                    .send_message(event_type.zulip_category(), &zulip_text)
                    .await
                {
                    error!("Failed to dispatch Zulip notification: {:?}", e);
                } else {
                    info!("Dispatched Zulip notification for {:?}", event_type);
                }
            } else {
                info!(
                    "Skipping Zulip notification for repo '{:?}' (not in allowed list)",
                    event_repo
                );
            }
        }

        if let Some(ref lark) = self.lark {
            if is_repo_allowed(
                lark.config().allowed_repositories.as_ref(),
                self.config.allowed_repositories.as_ref(),
                event_repo.as_deref(),
            ) {
                let lark_fields: Vec<(&str, &str)> = fields
                    .iter()
                    .map(|f| (f.name.as_str(), f.value.as_str()))
                    .collect();
                if let Err(e) = lark
                    .send_notification(title, message, url, level.lark_template(), &lark_fields)
                    .await
                {
                    error!("Failed to dispatch Lark notification: {:?}", e);
                } else {
                    info!("Dispatched Lark notification for {:?}", event_type);
                }
            } else {
                info!(
                    "Skipping Lark notification for repo '{:?}' (not in allowed list)",
                    event_repo
                );
            }
        }
    }
}

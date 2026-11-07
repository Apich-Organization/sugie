pub mod discord;
pub mod lark;
pub mod zulip;

pub use discord::{DiscordBotChannel, DiscordEmbedField, DiscordNotifier, DiscordWebhookChannel};
pub use lark::{LarkChannel, LarkNotifier};
pub use crate::config::{LarkChannelConfig, LarkWebhookChannel};
pub use zulip::{ZulipChannelConfig, ZulipNotifier, ZulipTopicCategory};

use crate::config::NotificationsConfig;
use std::sync::{Arc, RwLock};
use tracing::{error, info};

pub fn create_notification_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub fn is_safe_webhook_url(raw_url: &str) -> bool {
    let parsed = match url::Url::parse(raw_url) {
        Ok(u) => u,
        Err(_) => return false,
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let host = match parsed.host() {
        Some(h) => h,
        None => return false,
    };
    match host {
        url::Host::Ipv4(ip) => {
            if ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
            {
                return false;
            }
        }
        url::Host::Ipv6(ip) => {
            if ip.is_loopback() || ip.is_unspecified() {
                return false;
            }
        }
        url::Host::Domain(d) => {
            let lower = d.to_ascii_lowercase();
            if lower == "localhost"
                || lower.ends_with(".localhost")
                || lower.ends_with(".local")
                || lower == "metadata.google.internal"
                || lower == "instance-data"
            {
                return false;
            }
        }
    }
    true
}

pub fn sanitize_error_str(err: &dyn std::fmt::Display) -> String {
    let s = err.to_string();
    if let Some(idx) = s.find("for url (") {
        format!("{} (URL redacted)", &s[..idx])
    } else {
        s
    }
}

struct DispatcherInner {
    discord: Option<DiscordNotifier>,
    zulip: Option<ZulipNotifier>,
    lark: Option<LarkNotifier>,
    config: NotificationsConfig,
}

#[derive(Clone)]
pub struct NotificationDispatcher {
    inner: Arc<RwLock<DispatcherInner>>,
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
            .map(|c| DiscordNotifier::new(c.clone()))
            // A `[notifications.discord]` block with neither a webhook nor a bot
            // token would otherwise accept every event and fail on each one.
            .filter(DiscordNotifier::is_configured);
        let zulip = config.zulip.as_ref().map(|c| ZulipNotifier::new(c.clone()));
        let lark = config.lark.as_ref().map(|c| LarkNotifier::new(c.clone()));
        Self {
            inner: Arc::new(RwLock::new(DispatcherInner {
                discord,
                zulip,
                lark,
                config: config.clone(),
            })),
        }
    }

    /// Hot-reload notifiers with new configuration
    pub fn reload(&self, config: &NotificationsConfig) {
        let discord = config
            .discord
            .as_ref()
            .map(|c| DiscordNotifier::new(c.clone()))
            .filter(DiscordNotifier::is_configured);
        let zulip = config.zulip.as_ref().map(|c| ZulipNotifier::new(c.clone()));
        let lark = config.lark.as_ref().map(|c| LarkNotifier::new(c.clone()));
        if let Ok(mut inner) = self.inner.write() {
            inner.discord = discord;
            inner.zulip = zulip;
            inner.lark = lark;
            inner.config = config.clone();
            info!("NotificationDispatcher reloaded new destination channels and secrets");
        }
    }

    pub fn discord(&self) -> Option<DiscordNotifier> {
        self.inner.read().ok().and_then(|i| i.discord.clone())
    }

    pub fn zulip(&self) -> Option<ZulipNotifier> {
        self.inner.read().ok().and_then(|i| i.zulip.clone())
    }

    pub fn lark(&self) -> Option<LarkNotifier> {
        self.inner.read().ok().and_then(|i| i.lark.clone())
    }

    pub fn config(&self) -> NotificationsConfig {
        self.inner
            .read()
            .map(|i| i.config.clone())
            .unwrap_or_default()
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
        let (discord, zulip, lark, config) = match self.inner.read() {
            Ok(inner) => (
                inner.discord.clone(),
                inner.zulip.clone(),
                inner.lark.clone(),
                inner.config.clone(),
            ),
            Err(_) => return,
        };

        if !event_type.is_enabled(&config) {
            return;
        }

        let event_repo = extract_repo_from_context(title, url);

        if let Some(ref discord) = discord {
            if is_repo_allowed(
                discord.config().allowed_repositories.as_ref(),
                config.allowed_repositories.as_ref(),
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

        if let Some(ref zulip) = zulip {
            if is_repo_allowed(
                zulip.config().allowed_repositories.as_ref(),
                config.allowed_repositories.as_ref(),
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

        if let Some(ref lark) = lark {
            if is_repo_allowed(
                lark.config().allowed_repositories.as_ref(),
                config.allowed_repositories.as_ref(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_safe_webhook_url() {
        // Safe HTTPS URLs
        assert!(is_safe_webhook_url("https://discord.com/api/webhooks/123/token"));
        assert!(is_safe_webhook_url("https://open.larksuite.com/open-apis/bot/v2/hook/abc"));
        assert!(is_safe_webhook_url("https://zulip.example.com/api/v1/messages"));

        // Insecure scheme
        assert!(!is_safe_webhook_url("http://discord.com/api/webhooks/123/token"));
        assert!(!is_safe_webhook_url("ftp://example.com"));

        // Loopback and private / link-local addresses (SSRF)
        assert!(!is_safe_webhook_url("https://127.0.0.1/webhook"));
        assert!(!is_safe_webhook_url("https://localhost/webhook"));
        assert!(!is_safe_webhook_url("https://169.254.169.254/latest/meta-data"));
        assert!(!is_safe_webhook_url("https://10.0.0.1/admin"));
        assert!(!is_safe_webhook_url("https://192.168.1.1/internal"));
        assert!(!is_safe_webhook_url("https://172.16.0.1/secret"));
        assert!(!is_safe_webhook_url("https://metadata.google.internal/computeMetadata"));
    }

    #[test]
    fn test_sanitize_error_str() {
        let err_with_url = "error sending request for url (https://discord.com/api/webhooks/123/SECRET_KEY): connection timeout";
        let cleaned = sanitize_error_str(&err_with_url);
        assert!(!cleaned.contains("SECRET_KEY"));
        assert!(cleaned.contains("(URL redacted)"));
    }
}

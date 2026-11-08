use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SugieConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_db_path")]
    pub db_path: String,

    #[serde(default)]
    pub github: GitHubAuthConfig,

    #[serde(default)]
    pub notifications: NotificationsConfig,

    #[serde(default)]
    pub chat: ChatConfig,

    #[serde(default)]
    pub default_rules: RepoConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatConfig {
    pub zulip_token: Option<String>,
    pub lark_verification_token: Option<String>,
    pub lark_encrypt_key: Option<String>,
    pub discord_public_key: Option<String>,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_db_path() -> String {
    "sugie.db".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitHubAuthConfig {
    #[serde(default)]
    pub app_id: u64,
    pub private_key_pem: Option<String>,
    pub private_key_path: Option<String>,
    #[serde(default)]
    pub webhook_secret: String,
}

impl GitHubAuthConfig {
    pub fn get_private_key(&self) -> Result<String, anyhow::Error> {
        if let Some(ref pem) = self.private_key_pem {
            if !pem.trim().is_empty() {
                return Ok(pem.clone());
            }
        }
        if let Some(ref path) = self.private_key_path {
            let content = std::fs::read_to_string(path)?;
            return Ok(content);
        }
        anyhow::bail!("Neither private_key_pem nor private_key_path provided");
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationsConfig {
    pub discord: Option<DiscordConfig>,
    pub zulip: Option<ZulipConfig>,
    pub lark: Option<LarkConfig>,
    pub allowed_repositories: Option<Vec<String>>,
    #[serde(default)]
    pub events: NotificationEventsConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscordWebhookChannel {
    pub name: String,
    #[serde(alias = "webhook_url")]
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscordBotChannel {
    pub name: String,
    #[serde(alias = "id")]
    pub channel_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Notification delivery mode: "auto", "webhook", "bot", or "both".
    /// - "webhook": send only via incoming webhook URL to default group
    /// - "bot": send only via bot API to Discord channel(s)
    /// - "both": send to both webhook URL and bot channel(s)
    /// - "auto": prefers bot channel if configured, otherwise falls back to webhook URL
    #[serde(default = "default_notify_mode")]
    pub mode: String,
    /// Channel webhook. Kept for existing installs; superseded by `bot_token`.
    #[serde(default)]
    pub webhook_url: String,
    pub reports_webhook_url: Option<String>,
    /// Bot token, normally supplied as `SUGIE_DISCORD_BOT_TOKEN` in `.env`.
    ///
    /// One token covers every channel the bot has been invited to, so there is
    /// no need for a separate webhook URL per channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
    /// Channel id for ordinary notifications when posting as a bot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    /// Channel id for scheduled reports; falls back to `channel_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports_channel_id: Option<String>,
    /// Display name, webhook mode only — a bot always posts under its own name.
    #[serde(default = "default_discord_username")]
    pub username: String,
    pub avatar_url: Option<String>,
    pub allowed_repositories: Option<Vec<String>>,
    /// Multiple named webhook channels.
    #[serde(default, alias = "webhook_channels")]
    pub webhooks: Vec<DiscordWebhookChannel>,
    /// Multiple named bot channels.
    #[serde(default, alias = "channels")]
    pub bot_channels: Vec<DiscordBotChannel>,
}

fn default_discord_username() -> String {
    "sugie-bot".to_string()
}

pub fn default_notify_mode() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZulipChannelConfig {
    pub name: String,
    #[serde(alias = "stream")]
    pub channel: String,
    #[serde(default = "default_zulip_channel_topic")]
    pub topic: String,
}

fn default_zulip_channel_topic() -> String {
    "announcements".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZulipConfig {
    #[serde(default)]
    pub site_url: String,
    #[serde(default)]
    pub bot_email: String,
    #[serde(default)]
    pub bot_api_key: String,
    #[serde(default = "default_zulip_stream", alias = "channel")]
    pub stream: String,
    #[serde(default = "default_zulip_topic_triage")]
    pub topic_triage: String,
    #[serde(default = "default_zulip_topic_reports")]
    pub topic_reports: String,
    #[serde(default = "default_zulip_topic_alerts")]
    pub topic_alerts: String,
    #[serde(default = "default_zulip_topic_assigns")]
    pub topic_assigns: String,
    pub allowed_repositories: Option<Vec<String>>,
    /// Multiple named channel + topic destinations.
    #[serde(default)]
    pub channels: Vec<ZulipChannelConfig>,
}

fn default_zulip_stream() -> String {
    "github-triage".to_string()
}

fn default_zulip_topic_triage() -> String {
    "triage".to_string()
}

fn default_zulip_topic_reports() -> String {
    "reports".to_string()
}

fn default_zulip_topic_alerts() -> String {
    "alerts".to_string()
}

fn default_zulip_topic_assigns() -> String {
    "assignments".to_string()
}

fn default_lark_api_base() -> String {
    "https://open.larksuite.com".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LarkChannelConfig {
    pub name: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LarkWebhookChannel {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LarkConfig {
    /// Notification delivery mode: "auto", "webhook", "bot", or "both".
    /// - "webhook": send only via incoming webhook URL to default group
    /// - "bot": send only via Lark Open API to group chat(s)
    /// - "both": send to both webhook URL and bot chat(s)
    /// - "auto": prefers bot channels if configured, otherwise falls back to webhook URL
    #[serde(default = "default_notify_mode")]
    pub mode: String,
    /// Custom Robot incoming webhook URL (single group chat)
    #[serde(default)]
    pub webhook_url: String,
    /// Webhook signature verification secret
    pub secret: Option<String>,
    /// Lark App ID (e.g. cli_a1b2c3d4e5...), normally from SUGIE_LARK_APP_ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Lark App Secret, normally from SUGIE_LARK_APP_SECRET
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_secret: Option<String>,
    /// Default group chat ID (e.g. oc_xxx) to send notifications to when no channel is specified
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_chat_id: Option<String>,
    /// Base URL for Lark Open API (defaults to https://open.larksuite.com, or https://open.feishu.cn for Feishu)
    #[serde(default = "default_lark_api_base")]
    pub api_base: String,
    pub allowed_repositories: Option<Vec<String>>,
    /// Multiple named channel (group chat) destinations for Lark App Bot
    #[serde(default)]
    pub channels: Vec<LarkChannelConfig>,
    /// Multiple named custom robot webhook destinations for different groups
    #[serde(default)]
    pub webhooks: Vec<LarkWebhookChannel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEventsConfig {
    #[serde(default = "default_true")]
    pub weekly_report: bool,
    #[serde(default = "default_true")]
    pub monthly_report: bool,
    #[serde(default = "default_true")]
    pub assigned: bool,
    #[serde(default = "default_true")]
    pub unassigned: bool,
    #[serde(default = "default_true")]
    pub issue_opened: bool,
    #[serde(default = "default_true")]
    pub issue_closed: bool,
    #[serde(default = "default_true")]
    pub pr_opened: bool,
    #[serde(default = "default_true")]
    pub pr_merged: bool,
    #[serde(default = "default_true")]
    pub label_changed: bool,
    #[serde(default = "default_true")]
    pub stale_warned: bool,
    #[serde(default = "default_true")]
    pub stale_closed: bool,
    #[serde(default = "default_true")]
    pub ci_failure: bool,
    #[serde(default = "default_true")]
    pub duplicate_detected: bool,
    #[serde(default = "default_true")]
    pub dco_failed: bool,
    #[serde(default = "default_true")]
    pub growth_milestone: bool,
    #[serde(default = "default_true")]
    pub cross_repo_move: bool,
    #[serde(default = "default_true")]
    pub issue_comment: bool,
}

impl Default for NotificationEventsConfig {
    fn default() -> Self {
        Self {
            weekly_report: true,
            monthly_report: true,
            assigned: true,
            unassigned: true,
            issue_opened: true,
            issue_closed: true,
            issue_comment: true,
            pr_opened: true,
            pr_merged: true,
            label_changed: true,
            stale_warned: true,
            stale_closed: true,
            ci_failure: true,
            duplicate_detected: true,
            dco_failed: true,
            growth_milestone: true,
            cross_repo_move: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoConfig {
    #[serde(default)]
    pub labels: LabelsConfig,
    #[serde(default)]
    pub duplicates: DuplicateConfig,
    #[serde(default)]
    pub codeowners: CodeownersConfig,
    #[serde(default)]
    pub stale: StaleConfig,
    #[serde(default)]
    pub first_time: FirstTimeConfig,
    #[serde(default)]
    pub projects: ProjectsConfig,
    #[serde(default)]
    pub dco: DcoConfig,
    #[serde(default)]
    pub ci_summary: CiSummaryConfig,
    #[serde(default)]
    pub reviewers: ReviewersConfig,
    #[serde(default)]
    pub growth: GrowthConfig,
    #[serde(default)]
    pub reports: ReportsConfig,
    #[serde(default)]
    pub commands: CommandsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub path_rules: Vec<PathLabelRule>,
    #[serde(default)]
    pub keyword_rules: Vec<KeywordLabelRule>,
    #[serde(default)]
    pub template_rules: Vec<TemplateLabelRule>,
    #[serde(default)]
    pub size_rules: SizeLabelConfig,
    #[serde(default)]
    pub draft_rules: DraftLabelConfig,
    #[serde(default)]
    pub conventional_commits: ConventionalCommitsConfig,
}

impl Default for LabelsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path_rules: Vec::new(),
            keyword_rules: Vec::new(),
            template_rules: Vec::new(),
            size_rules: SizeLabelConfig::default(),
            draft_rules: DraftLabelConfig::default(),
            conventional_commits: ConventionalCommitsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizeLabelConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default = "default_size_xs")]
    pub xs_max: u64,
    #[serde(default = "default_size_s")]
    pub s_max: u64,
    #[serde(default = "default_size_m")]
    pub m_max: u64,
    #[serde(default = "default_size_l")]
    pub l_max: u64,
    #[serde(default = "default_size_labels")]
    pub labels: Vec<String>, // [XS, S, M, L, XL]
}

impl Default for SizeLabelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            xs_max: 10,
            s_max: 50,
            m_max: 250,
            l_max: 800,
            labels: default_size_labels(),
        }
    }
}

fn default_size_xs() -> u64 {
    10
}
fn default_size_s() -> u64 {
    50
}
fn default_size_m() -> u64 {
    250
}
fn default_size_l() -> u64 {
    800
}
fn default_size_labels() -> Vec<String> {
    vec![
        "size/XS".to_string(),
        "size/S".to_string(),
        "size/M".to_string(),
        "size/L".to_string(),
        "size/XL".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftLabelConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_draft_label")]
    pub draft_label: String,
    #[serde(default = "default_true")]
    pub remove_on_ready: bool,
}

impl Default for DraftLabelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            draft_label: "work-in-progress".to_string(),
            remove_on_ready: true,
        }
    }
}

fn default_draft_label() -> String {
    "work-in-progress".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConventionalCommitsConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default = "default_conventional_mappings")]
    pub mappings: HashMap<String, String>,
}

impl Default for ConventionalCommitsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mappings: default_conventional_mappings(),
        }
    }
}

fn default_conventional_mappings() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("feat".to_string(), "enhancement".to_string());
    m.insert("fix".to_string(), "bug".to_string());
    m.insert("docs".to_string(), "documentation".to_string());
    m.insert("perf".to_string(), "performance".to_string());
    m.insert("test".to_string(), "testing".to_string());
    m.insert("ci".to_string(), "ci".to_string());
    m
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathLabelRule {
    pub pattern: String,
    #[serde(default)]
    pub ignore_patterns: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordLabelRule {
    #[serde(default)]
    pub keywords: Vec<String>,
    pub regex: Option<String>,
    #[serde(default = "default_keyword_target")]
    pub target: String, // "title", "body", "both"
    pub labels: Vec<String>,
}

fn default_keyword_target() -> String {
    "both".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateLabelRule {
    pub pattern: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_duplicate_threshold")]
    pub threshold: f64,
    pub auto_close_threshold: Option<f64>,
    #[serde(default = "default_duplicate_label")]
    pub add_label: Option<String>,
    #[serde(default = "default_true")]
    pub search_open_only: bool,
    #[serde(default = "default_max_search_candidates")]
    pub max_search_candidates: u32,
    #[serde(default = "default_duplicate_algorithm")]
    pub algorithm: String, // "combined", "jaccard", "jaro_winkler", "token_cosine"
    pub comment_template: Option<String>,
}

impl Default for DuplicateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.65,
            auto_close_threshold: Some(0.90),
            add_label: Some("possible-duplicate".to_string()),
            search_open_only: true,
            max_search_candidates: 100,
            algorithm: "combined".to_string(),
            comment_template: None,
        }
    }
}

fn default_duplicate_threshold() -> f64 {
    0.65
}

fn default_duplicate_label() -> Option<String> {
    Some("possible-duplicate".to_string())
}

fn default_max_search_candidates() -> u32 {
    100
}

fn default_duplicate_algorithm() -> String {
    "combined".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeownersConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_codeowners_file")]
    pub file_path: String,
    #[serde(default = "default_true")]
    pub auto_assign: bool,
    #[serde(default)]
    pub mention_in_comment: bool,
    #[serde(default)]
    pub rules: Vec<CodeownerRule>,
}

impl Default for CodeownersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            file_path: ".github/CODEOWNERS".to_string(),
            auto_assign: true,
            mention_in_comment: false,
            rules: Vec::new(),
        }
    }
}

fn default_codeowners_file() -> String {
    ".github/CODEOWNERS".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeownerRule {
    pub path_pattern: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub maintainers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_stale_check_interval_hours")]
    pub check_interval_hours: u32,
    #[serde(default = "default_true")]
    pub enable_issues: bool,
    #[serde(default = "default_true")]
    pub enable_prs: bool,
    #[serde(default = "default_stale_warn_days")]
    pub warn_days: u32,
    #[serde(default = "default_stale_close_days")]
    pub close_days: u32,
    pub lock_days: Option<u32>,
    #[serde(default = "default_stale_label")]
    pub stale_label: String,
    pub closed_label: Option<String>,
    #[serde(default = "default_exempt_labels")]
    pub exempt_labels: Vec<String>,
    #[serde(default)]
    pub exempt_milestones: Vec<String>,
    #[serde(default)]
    pub exempt_authors: Vec<String>,
    #[serde(default = "default_stale_warn_message")]
    pub warn_message: String,
    #[serde(default = "default_stale_close_message")]
    pub close_message: String,
}

impl Default for StaleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_hours: 6,
            enable_issues: true,
            enable_prs: true,
            warn_days: 30,
            close_days: 7,
            lock_days: Some(14),
            stale_label: "stale".to_string(),
            closed_label: None,
            exempt_labels: vec!["pinned".to_string(), "security".to_string(), "never-stale".to_string()],
            exempt_milestones: Vec::new(),
            exempt_authors: Vec::new(),
            warn_message: "This item has been marked as stale due to inactivity ({days} days without updates). It will be closed in {close_days} days if no further activity occurs.".to_string(),
            close_message: "This item was automatically closed because it has been inactive for {close_days} days since being marked as stale. Feel free to reopen if you are still actively working on it.".to_string(),
        }
    }
}

fn default_stale_check_interval_hours() -> u32 {
    6
}

fn default_stale_warn_days() -> u32 {
    30
}

fn default_stale_close_days() -> u32 {
    7
}

fn default_stale_label() -> String {
    "stale".to_string()
}

fn default_exempt_labels() -> Vec<String> {
    vec![
        "pinned".to_string(),
        "security".to_string(),
        "never-stale".to_string(),
    ]
}

fn default_stale_warn_message() -> String {
    "This item has been marked as stale due to inactivity ({days} days without updates). It will be closed in {close_days} days if no further activity occurs.".to_string()
}

fn default_stale_close_message() -> String {
    "This item was automatically closed because it has been inactive for {close_days} days since being marked as stale. Feel free to reopen if you are still actively working on it.".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstTimeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_welcome_issue_comment")]
    pub welcome_issue_comment: String,
    #[serde(default = "default_welcome_pr_comment")]
    pub welcome_pr_comment: String,
    #[serde(default = "default_first_time_issue_labels")]
    pub issue_labels: Vec<String>,
    #[serde(default = "default_first_time_pr_labels")]
    pub pr_labels: Vec<String>,
}

impl Default for FirstTimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            welcome_issue_comment: "👋 Welcome @{author}! Thank you for opening your first issue in this repository. Our maintainers will triage it shortly.".to_string(),
            welcome_pr_comment: "🎉 Welcome @{author}! Thank you for your first pull request! Please make sure tests pass and commits follow our guidelines.".to_string(),
            issue_labels: vec!["first-time-contributor".to_string()],
            pr_labels: vec!["first-time-contributor".to_string()],
        }
    }
}

fn default_welcome_issue_comment() -> String {
    "👋 Welcome @{author}! Thank you for opening your first issue in this repository. Our maintainers will triage it shortly.".to_string()
}

fn default_welcome_pr_comment() -> String {
    "🎉 Welcome @{author}! Thank you for your first pull request! Please make sure tests pass and commits follow our guidelines.".to_string()
}

fn default_first_time_issue_labels() -> Vec<String> {
    vec!["first-time-contributor".to_string()]
}

fn default_first_time_pr_labels() -> Vec<String> {
    vec!["first-time-contributor".to_string()]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectsConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    pub issue_project_number: Option<i64>,
    pub pr_project_number: Option<i64>,
    pub project_number: Option<i64>, // Fallback
    pub project_owner: Option<String>,
    pub initial_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcoConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub require_signoff: bool,
    #[serde(default = "default_true")]
    pub set_commit_status: bool,
    #[serde(default = "default_dco_status_context")]
    pub status_context: String,
    #[serde(default = "default_dco_missing_label")]
    pub missing_label: Option<String>,
    #[serde(default = "default_exempt_authors")]
    pub exempt_authors: Vec<String>,
    #[serde(default = "default_dco_comment")]
    pub failure_comment: Option<String>,
}

impl Default for DcoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            require_signoff: true,
            set_commit_status: true,
            status_context: "sugie/dco".to_string(),
            missing_label: Some("dco-missing".to_string()),
            exempt_authors: default_exempt_authors(),
            failure_comment: Some("⚠️ Some commits in this PR are missing a valid `Signed-off-by` trailer. Please sign your commits using `git commit --amend -s` or `git rebase --signoff`.".to_string()),
        }
    }
}

fn default_dco_status_context() -> String {
    "sugie/dco".to_string()
}

fn default_exempt_authors() -> Vec<String> {
    vec!["dependabot[bot]".to_string(), "renovate[bot]".to_string()]
}

fn default_dco_missing_label() -> Option<String> {
    Some("dco-missing".to_string())
}

fn default_dco_comment() -> Option<String> {
    Some("⚠️ Some commits in this PR are missing a valid `Signed-off-by` trailer. Please sign your commits using `git commit --amend -s` or `git rebase --signoff`.".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiSummaryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub comment_on_failure: bool,
    #[serde(default = "default_true")]
    pub sticky: bool,
    #[serde(default = "default_true")]
    pub notify_on_failure: bool,
}

impl Default for CiSummaryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            comment_on_failure: true,
            sticky: true,
            notify_on_failure: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewersConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default)]
    pub pool: Vec<String>,
    #[serde(default = "default_reviewers_per_pr")]
    pub reviewers_per_pr: usize,
    #[serde(default = "default_max_active_reviews")]
    pub max_active_reviews: u32,
    #[serde(default = "default_review_load_balancing")]
    pub load_balancing: String, // "least_busy" or "round_robin"
    #[serde(default)]
    pub out_of_office: Vec<String>,
    #[serde(default)]
    pub component_pools: HashMap<String, Vec<String>>,
}

fn default_reviewers_per_pr() -> usize {
    1
}

fn default_max_active_reviews() -> u32 {
    5
}

fn default_review_load_balancing() -> String {
    "least_busy".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthMilestone {
    pub merged_prs: u32,
    pub message: String,
    #[serde(default = "default_true")]
    pub notify_channels: bool,
    pub recommend_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_growth_thresholds")]
    pub thresholds: Vec<GrowthMilestone>,
}

impl Default for GrowthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            thresholds: default_growth_thresholds(),
        }
    }
}

fn default_growth_thresholds() -> Vec<GrowthMilestone> {
    vec![
        GrowthMilestone {
            merged_prs: 3,
            message: "🎉 Contributor @{author} has merged 3 PRs! Consider inviting them to the Triage team.".to_string(),
            notify_channels: true,
            recommend_role: Some("triage".to_string()),
        },
        GrowthMilestone {
            merged_prs: 10,
            message: "🚀 Outstanding contributor @{author} with 10 merged PRs! Consider inviting them to the Committer/Maintainer team.".to_string(),
            notify_channels: true,
            recommend_role: Some("committer".to_string()),
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportsConfig {
    #[serde(default = "default_true")]
    pub weekly_enabled: bool,
    #[serde(default = "default_true")]
    pub monthly_enabled: bool,
    #[serde(default = "default_weekly_interval_days")]
    pub weekly_interval_days: u32,
    #[serde(default = "default_monthly_interval_days")]
    pub monthly_interval_days: u32,
    #[serde(default = "default_false")]
    pub publish_as_issue: bool,
    #[serde(default = "default_true")]
    pub sync_to_discord: bool,
    #[serde(default = "default_true")]
    pub sync_to_zulip: bool,
    #[serde(default = "default_report_labels")]
    pub issue_labels: Vec<String>,
    #[serde(default = "default_top_contributors_limit")]
    pub top_contributors_limit: usize,
    pub custom_header: Option<String>,
    pub custom_footer: Option<String>,
}

impl Default for ReportsConfig {
    fn default() -> Self {
        Self {
            weekly_enabled: true,
            monthly_enabled: true,
            weekly_interval_days: 7,
            monthly_interval_days: 30,
            publish_as_issue: false,
            sync_to_discord: true,
            sync_to_zulip: true,
            issue_labels: vec!["report".to_string(), "metrics".to_string()],
            top_contributors_limit: 10,
            custom_header: None,
            custom_footer: None,
        }
    }
}

fn default_weekly_interval_days() -> u32 {
    7
}
fn default_monthly_interval_days() -> u32 {
    30
}
fn default_top_contributors_limit() -> usize {
    10
}
fn default_report_labels() -> Vec<String> {
    vec!["report".to_string(), "metrics".to_string()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandsConfig {
    #[serde(default = "default_command_prefix")]
    pub prefix: String,
    #[serde(default)]
    pub allowed_roles: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    #[serde(default)]
    pub super_users: Vec<String>,
    #[serde(default)]
    pub restricted_commands: Vec<String>,
    /// If true, all bot commands (including read-only queries) require super user / admin permissions
    #[serde(default)]
    pub admin_only: bool,
    /// Platform-specific authorized super user IDs (e.g. open_id/user_id in Lark)
    #[serde(default)]
    pub lark_super_users: Vec<String>,
    /// Platform-specific authorized super user IDs or usernames in Discord
    #[serde(default)]
    pub discord_super_users: Vec<String>,
    /// Platform-specific authorized super user emails or usernames in Zulip
    #[serde(default)]
    pub zulip_super_users: Vec<String>,
    /// Whitelisted channel / group IDs for Lark (e.g. chat_id "oc_..."). If empty, all chats are allowed.
    #[serde(default)]
    pub lark_allowed_chats: Vec<String>,
    /// Whitelisted channel or guild IDs for Discord. If empty, all channels are allowed.
    #[serde(default)]
    pub discord_allowed_channels: Vec<String>,
    /// Whitelisted stream names or IDs for Zulip. If empty, all streams are allowed.
    #[serde(default)]
    pub zulip_allowed_channels: Vec<String>,
}

impl Default for CommandsConfig {
    fn default() -> Self {
        let mut aliases = HashMap::new();
        aliases.insert("l".to_string(), "label".to_string());
        aliases.insert("a".to_string(), "assign".to_string());
        aliases.insert("c".to_string(), "close".to_string());

        Self {
            prefix: "/".to_string(),
            allowed_roles: HashMap::new(),
            aliases,
            super_users: Vec::new(),
            restricted_commands: Vec::new(),
            admin_only: false,
            lark_super_users: Vec::new(),
            discord_super_users: Vec::new(),
            zulip_super_users: Vec::new(),
            lark_allowed_chats: Vec::new(),
            discord_allowed_channels: Vec::new(),
            zulip_allowed_channels: Vec::new(),
        }
    }
}

fn default_command_prefix() -> String {
    "/".to_string()
}

/// Read an environment variable, treating a blank value as unset so an empty
/// line in `.env` cannot override a working TOML setting.
fn env_trimmed(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn parse_repo_list(val: &str) -> Option<Vec<String>> {
    let trimmed = val.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return None;
    }
    let repos: Vec<String> = trimmed
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if repos.is_empty() {
        None
    } else {
        Some(repos)
    }
}

pub fn load_dotenv() {
    let candidates = [Path::new(".env"), Path::new("../.env")];
    for path in &candidates {
        if path.exists() {
            let content = match std::fs::read_to_string(path) {
                Ok(content) => content,
                Err(e) => {
                    // Skipping an unreadable `.env` in silence makes the failure
                    // look like "SUGIE_GITHUB_APP_ID not set", which sends people
                    // hunting through the config instead of at `ls -l .env`.
                    eprintln!(
                        "warning: {} exists but could not be read ({e}); \
                         starting without it. Check its owner and mode.",
                        path.display()
                    );
                    continue;
                }
            };
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let mut value = value.trim();
                    // Validate that key is non-empty, contains no null bytes, and matches valid env var identifiers.
                    // Lines like `=========================` split on `=` have an empty key `""`,
                    // which causes std::env::set_var to panic with "Invalid argument (os error 22)".
                    if key.is_empty()
                        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        || !key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                    {
                        continue;
                    }
                    // A quoted value keeps its inner text; the length check is
                    // part of the same condition so the slice below is always
                    // in range.
                    if ((value.starts_with('"') && value.ends_with('"'))
                        || (value.starts_with('\'') && value.ends_with('\'')))
                        && value.len() >= 2
                    {
                        value = &value[1..value.len() - 1];
                    }
                    if std::env::var(key).is_err() {
                        std::env::set_var(key, value);
                    }
                }
            }
            break;
        }
    }
}

fn load_discord_env_channels(
    webhooks: &mut Vec<DiscordWebhookChannel>,
    bot_channels: &mut Vec<DiscordBotChannel>,
) {
    // 1. Scan individual SUGIE_DISCORD_WEBHOOK_<NAME> and SUGIE_DISCORD_BOT_CHANNEL_<NAME> env vars
    for (k, v) in std::env::vars() {
        let v = v.trim().to_string();
        if v.is_empty() {
            continue;
        }
        if let Some(rest) = k.strip_prefix("SUGIE_DISCORD_WEBHOOK_") {
            if rest == "URL" || rest == "REPORTS_URL" {
                continue;
            }
            let name = rest.to_lowercase().replace('_', "-");
            if !webhooks.iter().any(|w| w.name == name) {
                webhooks.push(DiscordWebhookChannel { name, url: v });
            }
        } else if let Some(rest) = k.strip_prefix("SUGIE_DISCORD_BOT_CHANNEL_") {
            let name = rest.to_lowercase().replace('_', "-");
            if !bot_channels.iter().any(|c| c.name == name) {
                bot_channels.push(DiscordBotChannel { name, channel_id: v });
            }
        }
    }

    // 2. Scan comma-separated SUGIE_DISCORD_WEBHOOKS (name=url,name2=url2 or name:url)
    if let Ok(whs) = std::env::var("SUGIE_DISCORD_WEBHOOKS") {
        for pair in whs.split(',') {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parts = if let Some(p) = trimmed.split_once('=') {
                Some(p)
            } else {
                trimmed
                    .split_once("https://")
                    .map(|p| (p.0.trim_end_matches(':'), p.1))
            };
            if let Some((name, rest)) = parts {
                let name = name.trim().to_lowercase().replace('_', "-");
                let url = if rest.starts_with("https://") || rest.starts_with("http://") {
                    rest.trim().to_string()
                } else {
                    format!("https://{}", rest.trim())
                };
                if !name.is_empty() && !webhooks.iter().any(|w| w.name == name) {
                    webhooks.push(DiscordWebhookChannel { name, url });
                }
            }
        }
    }

    // 3. Scan comma-separated SUGIE_DISCORD_BOT_CHANNELS (name=id,name2=id2 or name:id)
    if let Ok(chans) = std::env::var("SUGIE_DISCORD_BOT_CHANNELS") {
        for pair in chans.split(',') {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parts = trimmed.split_once('=').or_else(|| trimmed.split_once(':'));
            if let Some((name, id)) = parts {
                let name = name.trim().to_lowercase().replace('_', "-");
                let channel_id = id.trim().to_string();
                if !name.is_empty() && !channel_id.is_empty() && !bot_channels.iter().any(|c| c.name == name) {
                    bot_channels.push(DiscordBotChannel { name, channel_id });
                }
            }
        }
    }
}

fn load_zulip_env_channels(channels: &mut Vec<ZulipChannelConfig>) {
    // 1. Scan individual SUGIE_ZULIP_CHANNEL_<NAME>="channel:topic" env vars
    for (k, v) in std::env::vars() {
        let v = v.trim().to_string();
        if v.is_empty() {
            continue;
        }
        if let Some(rest) = k.strip_prefix("SUGIE_ZULIP_CHANNEL_") {
            let name = rest.to_lowercase().replace('_', "-");
            let (channel, topic) = match v.split_once(':') {
                Some((ch, tp)) => (ch.trim().to_string(), tp.trim().to_string()),
                None => (v, default_zulip_channel_topic()),
            };
            if !channel.is_empty() && !channels.iter().any(|c| c.name == name) {
                channels.push(ZulipChannelConfig { name, channel, topic });
            }
        }
    }

    // 2. Scan comma-separated SUGIE_ZULIP_CHANNELS ("name=channel:topic,name2=channel2:topic2")
    if let Ok(chans) = std::env::var("SUGIE_ZULIP_CHANNELS") {
        for pair in chans.split(',') {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((name, rest)) = trimmed.split_once('=') {
                let name = name.trim().to_lowercase().replace('_', "-");
                let (channel, topic) = match rest.split_once(':') {
                    Some((ch, tp)) => (ch.trim().to_string(), tp.trim().to_string()),
                    None => (rest.trim().to_string(), default_zulip_channel_topic()),
                };
                if !name.is_empty() && !channel.is_empty() && !channels.iter().any(|c| c.name == name) {
                    channels.push(ZulipChannelConfig { name, channel, topic });
                }
            }
        }
    }
}

fn load_lark_env_channels(
    webhooks: &mut Vec<LarkWebhookChannel>,
    channels: &mut Vec<LarkChannelConfig>,
) {
    // 1. Scan individual SUGIE_LARK_WEBHOOK_<NAME>, SUGIE_LARK_SECRET_<NAME>, and SUGIE_LARK_CHANNEL_<NAME>
    let vars: std::collections::HashMap<String, String> = std::env::vars().collect();
    for (k, v) in &vars {
        let v = v.trim().to_string();
        if v.is_empty() {
            continue;
        }
        if let Some(rest) = k.strip_prefix("SUGIE_LARK_WEBHOOK_") {
            if rest == "URL" {
                continue;
            }
            let name = rest.to_lowercase().replace('_', "-");
            let secret_key = format!("SUGIE_LARK_SECRET_{rest}");
            let secret = vars.get(&secret_key).cloned().filter(|s| !s.trim().is_empty());
            if !webhooks.iter().any(|w| w.name == name) {
                webhooks.push(LarkWebhookChannel { name, url: v, secret });
            }
        } else if let Some(rest) = k.strip_prefix("SUGIE_LARK_CHANNEL_") {
            let name = rest.to_lowercase().replace('_', "-");
            if !channels.iter().any(|c| c.name == name) {
                channels.push(LarkChannelConfig { name, chat_id: v });
            }
        }
    }

    // 2. Scan comma-separated SUGIE_LARK_WEBHOOKS (name=url, name=url#secret, or name:url)
    if let Ok(whs) = std::env::var("SUGIE_LARK_WEBHOOKS") {
        for pair in whs.split(',') {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parts = if let Some(p) = trimmed.split_once('=') {
                Some(p)
            } else {
                trimmed
                    .split_once("https://")
                    .map(|p| (p.0.trim_end_matches(':'), p.1))
            };
            if let Some((name, rest)) = parts {
                let name = name.trim().to_lowercase().replace('_', "-");
                let (url_part, secret) = if let Some((u, s)) = rest.split_once('#') {
                    (u.trim(), Some(s.trim().to_string()).filter(|s| !s.is_empty()))
                } else {
                    (rest.trim(), None)
                };
                let url = if url_part.starts_with("https://") || url_part.starts_with("http://") {
                    url_part.to_string()
                } else {
                    format!("https://{}", url_part)
                };
                if !name.is_empty() && !webhooks.iter().any(|w| w.name == name) {
                    webhooks.push(LarkWebhookChannel { name, url, secret });
                }
            }
        }
    }

    // 3. Scan comma-separated SUGIE_LARK_CHANNELS (name=oc_xxx or name:oc_xxx)
    if let Ok(chs) = std::env::var("SUGIE_LARK_CHANNELS") {
        for pair in chs.split(',') {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (name, chat_id) = if let Some((n, c)) = trimmed.split_once('=') {
                (n.trim(), c.trim())
            } else if let Some((n, c)) = trimmed.split_once(':') {
                (n.trim(), c.trim())
            } else {
                continue;
            };
            let name = name.to_lowercase().replace('_', "-");
            if !name.is_empty() && !chat_id.is_empty() && !channels.iter().any(|c| c.name == name) {
                channels.push(LarkChannelConfig {
                    name,
                    chat_id: chat_id.to_string(),
                });
            }
        }
    }
}

impl SugieConfig {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, anyhow::Error> {
        let content = std::fs::read_to_string(path)?;
        let config: SugieConfig = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn load_from_env_or_file<P: AsRef<Path>>(path: P) -> Result<Self, anyhow::Error> {
        load_dotenv();

        let mut config = if path.as_ref().exists() {
            Self::load_from_file(path)?
        } else {
            SugieConfig {
                host: default_host(),
                port: default_port(),
                db_path: default_db_path(),
                github: GitHubAuthConfig::default(),
                notifications: NotificationsConfig::default(),
                chat: ChatConfig::default(),
                default_rules: RepoConfig::default(),
            }
        };

        // Environment overrides
        if let Ok(host) = std::env::var("SUGIE_HOST") {
            if !host.trim().is_empty() {
                config.host = host;
            }
        }
        if let Ok(port) = std::env::var("SUGIE_PORT") {
            if let Ok(p) = port.parse() {
                config.port = p;
            }
        }
        if let Ok(db) = std::env::var("SUGIE_DB_PATH") {
            if !db.trim().is_empty() {
                config.db_path = db;
            }
        }
        if let Ok(app_id_str) = std::env::var("SUGIE_GITHUB_APP_ID") {
            if let Ok(id) = app_id_str.parse() {
                config.github.app_id = id;
            }
        }
        if let Ok(secret) = std::env::var("SUGIE_GITHUB_WEBHOOK_SECRET") {
            if !secret.trim().is_empty() {
                config.github.webhook_secret = secret;
            }
        }
        if let Ok(pk_pem) = std::env::var("SUGIE_GITHUB_PRIVATE_KEY_PEM") {
            if !pk_pem.trim().is_empty() {
                config.github.private_key_pem = Some(pk_pem);
            }
        }
        if let Ok(pk_path) = std::env::var("SUGIE_GITHUB_PRIVATE_KEY_PATH") {
            if !pk_path.trim().is_empty() {
                config.github.private_key_path = Some(pk_path);
            }
        }

        // Global notification repository filter
        if let Ok(repos) = std::env::var("SUGIE_NOTIFICATION_REPOSITORIES")
            .or_else(|_| std::env::var("SUGIE_NOTIFICATION_REPOS"))
        {
            if !repos.trim().is_empty() {
                config.notifications.allowed_repositories = parse_repo_list(&repos);
            }
        }

        // Discord notification settings (merge env secrets/overrides into TOML config)
        if let Some(ref mut discord) = config.notifications.discord {
            if let Ok(mode) = std::env::var("SUGIE_DISCORD_NOTIFY_MODE")
                .or_else(|_| std::env::var("SUGIE_DISCORD_MODE"))
            {
                if !mode.trim().is_empty() {
                    discord.mode = mode.trim().to_lowercase();
                }
            }
            if let Ok(url) = std::env::var("SUGIE_DISCORD_WEBHOOK_URL") {
                if !url.trim().is_empty() {
                    discord.webhook_url = url;
                }
            }
            if let Ok(reports_url) = std::env::var("SUGIE_DISCORD_REPORTS_WEBHOOK_URL") {
                if !reports_url.trim().is_empty() {
                    discord.reports_webhook_url = Some(reports_url);
                }
            }
            if let Ok(username) = std::env::var("SUGIE_DISCORD_USERNAME") {
                if !username.trim().is_empty() {
                    discord.username = username;
                }
            }
            if let Ok(avatar) = std::env::var("SUGIE_DISCORD_AVATAR_URL") {
                if !avatar.trim().is_empty() {
                    discord.avatar_url = Some(avatar);
                }
            }
            if let Some(token) = env_trimmed("SUGIE_DISCORD_BOT_TOKEN") {
                discord.bot_token = Some(token);
            }
            if let Some(channel) = env_trimmed("SUGIE_DISCORD_CHANNEL_ID") {
                discord.channel_id = Some(channel);
            }
            if let Some(channel) = env_trimmed("SUGIE_DISCORD_REPORTS_CHANNEL_ID") {
                discord.reports_channel_id = Some(channel);
            }
            if let Ok(repos) = std::env::var("SUGIE_DISCORD_REPOSITORIES")
                .or_else(|_| std::env::var("SUGIE_DISCORD_REPOS"))
            {
                discord.allowed_repositories = parse_repo_list(&repos);
            }
            load_discord_env_channels(&mut discord.webhooks, &mut discord.bot_channels);
        } else {
            // No [notifications.discord] section: build one from the environment
            // when either a webhook or a bot token is configured.
            let webhook_url = env_trimmed("SUGIE_DISCORD_WEBHOOK_URL").unwrap_or_default();
            let bot_token = env_trimmed("SUGIE_DISCORD_BOT_TOKEN");
            let mut webhooks = Vec::new();
            let mut bot_channels = Vec::new();
            load_discord_env_channels(&mut webhooks, &mut bot_channels);

            if !webhook_url.is_empty() || bot_token.is_some() || !webhooks.is_empty() || !bot_channels.is_empty() {
                let allowed_repositories = std::env::var("SUGIE_DISCORD_REPOSITORIES")
                    .or_else(|_| std::env::var("SUGIE_DISCORD_REPOS"))
                    .ok()
                    .and_then(|r| parse_repo_list(&r));
                let mode = std::env::var("SUGIE_DISCORD_NOTIFY_MODE")
                    .or_else(|_| std::env::var("SUGIE_DISCORD_MODE"))
                    .unwrap_or_else(|_| default_notify_mode());
                config.notifications.discord = Some(DiscordConfig {
                    mode,
                    webhook_url,
                    reports_webhook_url: env_trimmed("SUGIE_DISCORD_REPORTS_WEBHOOK_URL"),
                    bot_token,
                    channel_id: env_trimmed("SUGIE_DISCORD_CHANNEL_ID"),
                    reports_channel_id: env_trimmed("SUGIE_DISCORD_REPORTS_CHANNEL_ID"),
                    username: std::env::var("SUGIE_DISCORD_USERNAME")
                        .unwrap_or_else(|_| default_discord_username()),
                    avatar_url: env_trimmed("SUGIE_DISCORD_AVATAR_URL"),
                    allowed_repositories,
                    webhooks,
                    bot_channels,
                });
            }
        }

        // Zulip notification settings (merge env secrets/overrides into TOML config)
        if let Some(ref mut zulip) = config.notifications.zulip {
            if let Ok(site) = std::env::var("SUGIE_ZULIP_SITE_URL") {
                if !site.trim().is_empty() {
                    zulip.site_url = site;
                }
            }
            if let Ok(email) = std::env::var("SUGIE_ZULIP_BOT_EMAIL") {
                if !email.trim().is_empty() {
                    zulip.bot_email = email;
                }
            }
            if let Ok(key) = std::env::var("SUGIE_ZULIP_BOT_API_KEY") {
                if !key.trim().is_empty() {
                    zulip.bot_api_key = key;
                }
            }
            if let Ok(stream) = std::env::var("SUGIE_ZULIP_STREAM")
                .or_else(|_| std::env::var("SUGIE_ZULIP_CHANNEL"))
            {
                if !stream.trim().is_empty() {
                    zulip.stream = stream;
                }
            }
            if let Ok(topic) = std::env::var("SUGIE_ZULIP_TOPIC_TRIAGE") {
                if !topic.trim().is_empty() {
                    zulip.topic_triage = topic;
                }
            }
            if let Ok(topic) = std::env::var("SUGIE_ZULIP_TOPIC_REPORTS") {
                if !topic.trim().is_empty() {
                    zulip.topic_reports = topic;
                }
            }
            if let Ok(topic) = std::env::var("SUGIE_ZULIP_TOPIC_ALERTS") {
                if !topic.trim().is_empty() {
                    zulip.topic_alerts = topic;
                }
            }
            if let Ok(topic) = std::env::var("SUGIE_ZULIP_TOPIC_ASSIGNS") {
                if !topic.trim().is_empty() {
                    zulip.topic_assigns = topic;
                }
            }
            if let Ok(repos) = std::env::var("SUGIE_ZULIP_REPOSITORIES")
                .or_else(|_| std::env::var("SUGIE_ZULIP_REPOS"))
            {
                zulip.allowed_repositories = parse_repo_list(&repos);
            }
            load_zulip_env_channels(&mut zulip.channels);
        } else if let (Ok(site), Ok(email), Ok(key)) = (
            std::env::var("SUGIE_ZULIP_SITE_URL"),
            std::env::var("SUGIE_ZULIP_BOT_EMAIL"),
            std::env::var("SUGIE_ZULIP_BOT_API_KEY"),
        ) {
            if !site.trim().is_empty() && !email.trim().is_empty() && !key.trim().is_empty() {
                let stream = std::env::var("SUGIE_ZULIP_STREAM")
                    .or_else(|_| std::env::var("SUGIE_ZULIP_CHANNEL"))
                    .unwrap_or_else(|_| default_zulip_stream());
                let allowed_repositories = std::env::var("SUGIE_ZULIP_REPOSITORIES")
                    .or_else(|_| std::env::var("SUGIE_ZULIP_REPOS"))
                    .ok()
                    .and_then(|r| parse_repo_list(&r));
                let mut channels = Vec::new();
                load_zulip_env_channels(&mut channels);
                config.notifications.zulip = Some(ZulipConfig {
                    site_url: site,
                    bot_email: email,
                    bot_api_key: key,
                    stream,
                    topic_triage: std::env::var("SUGIE_ZULIP_TOPIC_TRIAGE")
                        .unwrap_or_else(|_| default_zulip_topic_triage()),
                    topic_reports: std::env::var("SUGIE_ZULIP_TOPIC_REPORTS")
                        .unwrap_or_else(|_| default_zulip_topic_reports()),
                    topic_alerts: std::env::var("SUGIE_ZULIP_TOPIC_ALERTS")
                        .unwrap_or_else(|_| default_zulip_topic_alerts()),
                    topic_assigns: std::env::var("SUGIE_ZULIP_TOPIC_ASSIGNS")
                        .unwrap_or_else(|_| default_zulip_topic_assigns()),
                    allowed_repositories,
                    channels,
                });
            }
        }

        // Lark notification settings (merge env secrets/overrides into TOML config)
        if let Some(ref mut lark) = config.notifications.lark {
            if let Ok(mode) = std::env::var("SUGIE_LARK_NOTIFY_MODE")
                .or_else(|_| std::env::var("SUGIE_LARK_MODE"))
            {
                if !mode.trim().is_empty() {
                    lark.mode = mode.trim().to_lowercase();
                }
            }
            if let Ok(url) = std::env::var("SUGIE_LARK_WEBHOOK_URL") {
                if !url.trim().is_empty() {
                    lark.webhook_url = url;
                }
            }
            if let Ok(secret) = std::env::var("SUGIE_LARK_SignatureVerification")
                .or_else(|_| std::env::var("SUGIE_LARK_SIGNATURE_VERIFICATION"))
                .or_else(|_| std::env::var("SUGIE_LARK_SECRET"))
            {
                if !secret.trim().is_empty() {
                    lark.secret = Some(secret);
                }
            }
            if let Ok(app_id) = std::env::var("SUGIE_LARK_APP_ID") {
                if !app_id.trim().is_empty() {
                    lark.app_id = Some(app_id);
                }
            }
            if let Ok(app_secret) = std::env::var("SUGIE_LARK_APP_SECRET") {
                if !app_secret.trim().is_empty() {
                    lark.app_secret = Some(app_secret);
                }
            }
            if let Ok(chat_id) = std::env::var("SUGIE_LARK_DEFAULT_CHAT_ID")
                .or_else(|_| std::env::var("SUGIE_LARK_CHAT_ID"))
            {
                if !chat_id.trim().is_empty() {
                    lark.default_chat_id = Some(chat_id);
                }
            }
            if let Ok(api_base) = std::env::var("SUGIE_LARK_API_BASE") {
                if !api_base.trim().is_empty() {
                    lark.api_base = api_base;
                }
            }
            if let Ok(repos) = std::env::var("SUGIE_LARK_REPOSITORIES")
                .or_else(|_| std::env::var("SUGIE_LARK_REPOS"))
            {
                lark.allowed_repositories = parse_repo_list(&repos);
            }
            load_lark_env_channels(&mut lark.webhooks, &mut lark.channels);
        } else {
            let app_id = std::env::var("SUGIE_LARK_APP_ID")
                .ok()
                .filter(|s| !s.trim().is_empty());
            let app_secret = std::env::var("SUGIE_LARK_APP_SECRET")
                .ok()
                .filter(|s| !s.trim().is_empty());
            let lark_url = std::env::var("SUGIE_LARK_WEBHOOK_URL")
                .ok()
                .filter(|s| !s.trim().is_empty());

            let mut webhooks = Vec::new();
            let mut channels = Vec::new();
            load_lark_env_channels(&mut webhooks, &mut channels);

            if app_id.is_some() || lark_url.is_some() || !webhooks.is_empty() || !channels.is_empty() {
                let secret = std::env::var("SUGIE_LARK_SignatureVerification")
                    .or_else(|_| std::env::var("SUGIE_LARK_SIGNATURE_VERIFICATION"))
                    .or_else(|_| std::env::var("SUGIE_LARK_SECRET"))
                    .ok()
                    .filter(|s| !s.trim().is_empty());
                let default_chat_id = std::env::var("SUGIE_LARK_DEFAULT_CHAT_ID")
                    .or_else(|_| std::env::var("SUGIE_LARK_CHAT_ID"))
                    .ok()
                    .filter(|s| !s.trim().is_empty());
                let api_base = std::env::var("SUGIE_LARK_API_BASE")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(default_lark_api_base);
                let allowed_repositories = std::env::var("SUGIE_LARK_REPOSITORIES")
                    .or_else(|_| std::env::var("SUGIE_LARK_REPOS"))
                    .ok()
                    .and_then(|r| parse_repo_list(&r));
                let mode = std::env::var("SUGIE_LARK_NOTIFY_MODE")
                    .or_else(|_| std::env::var("SUGIE_LARK_MODE"))
                    .unwrap_or_else(|_| default_notify_mode());
                config.notifications.lark = Some(LarkConfig {
                    mode,
                    webhook_url: lark_url.unwrap_or_default(),
                    secret,
                    app_id,
                    app_secret,
                    default_chat_id,
                    api_base,
                    allowed_repositories,
                    channels,
                    webhooks,
                });
            }
        }

        // Super users & restricted commands env overrides
        if let Ok(users) =
            std::env::var("SUGIE_SUPER_USERS").or_else(|_| std::env::var("SUGIE_SUPERUSERS"))
        {
            let list: Vec<String> = users
                .split(',')
                .map(|s| s.trim().trim_start_matches('@').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.super_users = list;
            }
        }

        if let Ok(cmds) = std::env::var("SUGIE_RESTRICTED_COMMANDS") {
            let list: Vec<String> = cmds
                .split(',')
                .map(|s| s.trim().trim_start_matches('/').to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.restricted_commands = list;
            }
        }

        if let Ok(val) = std::env::var("SUGIE_COMMANDS_ADMIN_ONLY") {
            let v = val.trim().to_lowercase();
            if v == "1" || v == "true" || v == "yes" {
                config.default_rules.commands.admin_only = true;
            } else if v == "0" || v == "false" || v == "no" {
                config.default_rules.commands.admin_only = false;
            }
        }

        if let Ok(users) = std::env::var("SUGIE_LARK_SUPER_USERS") {
            let list: Vec<String> = users
                .split(',')
                .map(|s| s.trim().trim_start_matches('@').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.lark_super_users = list;
            }
        }

        if let Ok(users) = std::env::var("SUGIE_DISCORD_SUPER_USERS") {
            let list: Vec<String> = users
                .split(',')
                .map(|s| s.trim().trim_start_matches('@').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.discord_super_users = list;
            }
        }

        if let Ok(users) = std::env::var("SUGIE_ZULIP_SUPER_USERS") {
            let list: Vec<String> = users
                .split(',')
                .map(|s| s.trim().trim_start_matches('@').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.zulip_super_users = list;
            }
        }

        // Whitelisted channels / groups for chat platforms
        if let Ok(chats) = std::env::var("SUGIE_LARK_ALLOWED_CHATS")
            .or_else(|_| std::env::var("SUGIE_LARK_ALLOWED_CHANNELS"))
        {
            let list: Vec<String> = chats
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.lark_allowed_chats = list;
            }
        }

        if let Ok(channels) = std::env::var("SUGIE_DISCORD_ALLOWED_CHANNELS") {
            let list: Vec<String> = channels
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.discord_allowed_channels = list;
            }
        }

        if let Ok(channels) = std::env::var("SUGIE_ZULIP_ALLOWED_CHANNELS")
            .or_else(|_| std::env::var("SUGIE_ZULIP_ALLOWED_STREAMS"))
        {
            let list: Vec<String> = channels
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                config.default_rules.commands.zulip_allowed_channels = list;
            }
        }

        // Inbound chat webhook verification tokens
        if let Ok(token) = std::env::var("SUGIE_ZULIP_OUTGOING_TOKEN") {
            if !token.trim().is_empty() {
                config.chat.zulip_token = Some(token);
            }
        }
        if let Ok(token) = std::env::var("SUGIE_LARK_VERIFICATION_TOKEN") {
            if !token.trim().is_empty() {
                config.chat.lark_verification_token = Some(token);
            }
        }
        if let Ok(key) = std::env::var("SUGIE_LARK_ENCRYPT_KEY")
            .or_else(|_| std::env::var("SUGIE_LARK_Encrypt_Key"))
            .or_else(|_| std::env::var("SUGIE_LARK_ENCRYPTKEY"))
        {
            if !key.trim().is_empty() {
                config.chat.lark_encrypt_key = Some(key);
            }
        }
        if let Ok(key) = std::env::var("SUGIE_DISCORD_PUBLIC_KEY") {
            if !key.trim().is_empty() {
                config.chat.discord_public_key = Some(key);
            }
        }

        if config.github.app_id == 0 {
            anyhow::bail!("SUGIE_GITHUB_APP_ID not set. Please provide sugie.toml or set SUGIE_GITHUB_APP_ID in .env or environment.");
        }

        Ok(config)
    }
}

impl RepoConfig {
    pub fn merge_with_override(&self, repo_override: Option<RepoConfig>) -> RepoConfig {
        match repo_override {
            None => self.clone(),
            Some(mut overridden) => {
                if overridden.labels.path_rules.is_empty() {
                    overridden.labels.path_rules = self.labels.path_rules.clone();
                }
                if overridden.labels.keyword_rules.is_empty() {
                    overridden.labels.keyword_rules = self.labels.keyword_rules.clone();
                }
                if overridden.reviewers.pool.is_empty() {
                    overridden.reviewers.pool = self.reviewers.pool.clone();
                }
                if overridden.commands.super_users.is_empty() {
                    overridden.commands.super_users = self.commands.super_users.clone();
                }
                if overridden.commands.restricted_commands.is_empty() {
                    overridden.commands.restricted_commands =
                        self.commands.restricted_commands.clone();
                }
                overridden
            }
        }
    }
}

pub fn interpolate(template: &str, vars: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (key, val) in vars {
        let placeholder = format!("{{{}}}", key);
        result = result.replace(&placeholder, val);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_and_env_merge() {
        let toml_str = r#"
        host = "127.0.0.1"
        port = 9000
        db_path = "custom.db"

        [notifications]
        allowed_repositories = ["MyOrg/RepoA", "MyOrg/RepoB"]

        [notifications.zulip]
        site_url = "https://zulip.example.com"
        bot_email = "bot@zulip.example.com"
        stream = "engineering"

        [default_rules.commands]
        super_users = ["alice"]
        restricted_commands = ["close", "reopen"]
        "#;

        let mut config: SugieConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9000);
        assert_eq!(config.db_path, "custom.db");
        assert_eq!(
            config.notifications.allowed_repositories,
            Some(vec!["MyOrg/RepoA".to_string(), "MyOrg/RepoB".to_string()])
        );
        assert_eq!(
            config.notifications.zulip.as_ref().unwrap().stream,
            "engineering"
        );
        assert_eq!(config.default_rules.commands.super_users, vec!["alice"]);

        // Simulate injecting secrets from .env
        config.github.app_id = 99999;
        config.github.webhook_secret = "my-secret-key".to_string();
        config.notifications.zulip.as_mut().unwrap().bot_api_key = "secret-zulip-token".to_string();

        assert_eq!(config.github.app_id, 99999);
        assert_eq!(config.github.webhook_secret, "my-secret-key");
        assert_eq!(
            config.notifications.zulip.unwrap().bot_api_key,
            "secret-zulip-token"
        );
    }
}

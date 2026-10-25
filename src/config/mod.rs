use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SugieConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_db_path")]
    pub db_path: String,

    pub github: GitHubAuthConfig,

    #[serde(default)]
    pub notifications: NotificationsConfig,

    #[serde(default)]
    pub default_rules: RepoConfig,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAuthConfig {
    pub app_id: u64,
    pub private_key_pem: Option<String>,
    pub private_key_path: Option<String>,
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
    #[serde(default)]
    pub events: NotificationEventsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    pub webhook_url: String,
    pub reports_webhook_url: Option<String>,
    #[serde(default = "default_discord_username")]
    pub username: String,
    pub avatar_url: Option<String>,
}

fn default_discord_username() -> String {
    "sugie-bot".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZulipConfig {
    pub site_url: String,
    pub bot_email: String,
    pub bot_api_key: String,
    #[serde(default = "default_zulip_stream")]
    pub stream: String,
    #[serde(default = "default_zulip_topic_triage")]
    pub topic_triage: String,
    #[serde(default = "default_zulip_topic_reports")]
    pub topic_reports: String,
    #[serde(default = "default_zulip_topic_alerts")]
    pub topic_alerts: String,
    #[serde(default = "default_zulip_topic_assigns")]
    pub topic_assigns: String,
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
        }
    }
}

fn default_command_prefix() -> String {
    "/".to_string()
}

pub fn load_dotenv() {
    let candidates = [
        Path::new(".env"),
        Path::new("../.env"),
    ];
    for path in &candidates {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some((key, value)) = line.split_once('=') {
                        let key = key.trim();
                        let mut value = value.trim();
                        if (value.starts_with('"') && value.ends_with('"'))
                            || (value.starts_with('\'') && value.ends_with('\''))
                        {
                            if value.len() >= 2 {
                                value = &value[1..value.len() - 1];
                            }
                        }
                        if std::env::var(key).is_err() {
                            std::env::set_var(key, value);
                        }
                    }
                }
            }
            break;
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
            let app_id: u64 = std::env::var("SUGIE_GITHUB_APP_ID")
                .map_err(|_| anyhow::anyhow!("SUGIE_GITHUB_APP_ID not set. Please provide sugie.toml or set SUGIE_GITHUB_APP_ID in .env or environment."))?
                .parse()?;
            let private_key_pem = std::env::var("SUGIE_GITHUB_PRIVATE_KEY_PEM").ok();
            let private_key_path = std::env::var("SUGIE_GITHUB_PRIVATE_KEY_PATH").ok();
            let webhook_secret = std::env::var("SUGIE_GITHUB_WEBHOOK_SECRET")
                .unwrap_or_else(|_| "secret".to_string());

            SugieConfig {
                host: std::env::var("SUGIE_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
                port: std::env::var("SUGIE_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(8080),
                db_path: std::env::var("SUGIE_DB_PATH").unwrap_or_else(|_| "data/sugie.db".to_string()),
                github: GitHubAuthConfig {
                    app_id,
                    private_key_pem,
                    private_key_path,
                    webhook_secret,
                },
                notifications: NotificationsConfig::default(),
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

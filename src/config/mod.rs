use serde::{Deserialize, Serialize};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    pub webhook_url: String,
    #[serde(default = "default_discord_username")]
    pub username: String,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub notify_on_stale: bool,
    #[serde(default = "default_true")]
    pub notify_on_growth: bool,
    #[serde(default = "default_true")]
    pub notify_on_report: bool,
    #[serde(default)]
    pub notify_on_ci_failure: bool,
}

fn default_discord_username() -> String {
    "sugie-bot".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZulipConfig {
    pub site_url: String,
    pub bot_email: String,
    pub bot_api_key: String,
    #[serde(default = "default_zulip_stream")]
    pub stream: String,
    #[serde(default = "default_zulip_topic")]
    pub topic: String,
    #[serde(default)]
    pub notify_on_stale: bool,
    #[serde(default = "default_true")]
    pub notify_on_growth: bool,
    #[serde(default = "default_true")]
    pub notify_on_report: bool,
    #[serde(default)]
    pub notify_on_ci_failure: bool,
}

fn default_zulip_stream() -> String {
    "github-triage".to_string()
}

fn default_zulip_topic() -> String {
    "sugie-alerts".to_string()
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LabelsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub path_rules: Vec<PathLabelRule>,
    #[serde(default)]
    pub keyword_rules: Vec<KeywordLabelRule>,
    #[serde(default)]
    pub template_rules: Vec<TemplateLabelRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathLabelRule {
    pub pattern: String,
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
    #[serde(default = "default_duplicate_label")]
    pub add_label: Option<String>,
    pub comment_template: Option<String>,
}

impl Default for DuplicateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.65,
            add_label: Some("possible-duplicate".to_string()),
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
    #[serde(default = "default_stale_warn_days")]
    pub warn_days: u32,
    #[serde(default = "default_stale_close_days")]
    pub close_days: u32,
    pub lock_days: Option<u32>,
    #[serde(default = "default_stale_label")]
    pub stale_label: String,
    #[serde(default = "default_exempt_labels")]
    pub exempt_labels: Vec<String>,
    #[serde(default = "default_stale_warn_message")]
    pub warn_message: String,
    #[serde(default = "default_stale_close_message")]
    pub close_message: String,
}

impl Default for StaleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warn_days: 30,
            close_days: 7,
            lock_days: Some(14),
            stale_label: "stale".to_string(),
            exempt_labels: vec!["pinned".to_string(), "security".to_string(), "never-stale".to_string()],
            warn_message: "This issue/PR has been marked as stale due to inactivity. It will be closed in 7 days if no further activity occurs. Thank you for your contributions!".to_string(),
            close_message: "This issue/PR was closed because it has been inactive for 7 days since being marked as stale. Feel free to reopen or open a new issue if needed.".to_string(),
        }
    }
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
    vec!["pinned".to_string(), "security".to_string(), "never-stale".to_string()]
}

fn default_stale_warn_message() -> String {
    "This issue/PR has been marked as stale due to inactivity. It will be closed in 7 days if no further activity occurs. Thank you for your contributions!".to_string()
}

fn default_stale_close_message() -> String {
    "This issue/PR was closed because it has been inactive for 7 days since being marked as stale. Feel free to reopen or open a new issue if needed.".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstTimeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_welcome_comment")]
    pub welcome_comment: String,
    #[serde(default = "default_first_time_labels")]
    pub labels: Vec<String>,
}

impl Default for FirstTimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            welcome_comment: "👋 Thank you for opening your first contribution! A maintainer will review it shortly.".to_string(),
            labels: vec!["first-time-contributor".to_string()],
        }
    }
}

fn default_welcome_comment() -> String {
    "👋 Thank you for opening your first contribution! A maintainer will review it shortly.".to_string()
}

fn default_first_time_labels() -> Vec<String> {
    vec!["first-time-contributor".to_string()]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectsConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    pub project_number: Option<i64>,
    pub project_owner: Option<String>,
    pub initial_status: Option<String>,
}

fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcoConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub require_signoff: bool,
    #[serde(default = "default_true")]
    pub set_commit_status: bool,
    #[serde(default = "default_dco_missing_label")]
    pub missing_label: Option<String>,
    #[serde(default = "default_dco_comment")]
    pub failure_comment: Option<String>,
}

impl Default for DcoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            require_signoff: true,
            set_commit_status: true,
            missing_label: Some("dco-missing".to_string()),
            failure_comment: Some("⚠️ Some commits are missing a valid `Signed-off-by` line. Please sign your commits using `git commit --amend -s` or `git rebase --signoff`.".to_string()),
        }
    }
}

fn default_dco_missing_label() -> Option<String> {
    Some("dco-missing".to_string())
}

fn default_dco_comment() -> Option<String> {
    Some("⚠️ Some commits are missing a valid `Signed-off-by` line. Please sign your commits using `git commit --amend -s` or `git rebase --signoff`.".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiSummaryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub comment_on_failure: bool,
    #[serde(default = "default_true")]
    pub sticky: bool,
}

impl Default for CiSummaryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            comment_on_failure: true,
            sticky: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewersConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default)]
    pub pool: Vec<String>,
    #[serde(default = "default_max_active_reviews")]
    pub max_active_reviews: u32,
    #[serde(default = "default_review_load_balancing")]
    pub load_balancing: String, // "least_busy" or "round_robin"
    #[serde(default)]
    pub out_of_office: Vec<String>,
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
            message: "🎉 Contributor has merged 3 PRs! Consider inviting them to the Triage team.".to_string(),
            notify_channels: true,
            recommend_role: Some("triage".to_string()),
        },
        GrowthMilestone {
            merged_prs: 10,
            message: "🚀 Outstanding contributor! 10 merged PRs! Consider inviting them to the Maintainer/Committer team.".to_string(),
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
    #[serde(default = "default_false")]
    pub publish_as_issue: bool,
    #[serde(default = "default_report_labels")]
    pub issue_labels: Vec<String>,
}

impl Default for ReportsConfig {
    fn default() -> Self {
        Self {
            weekly_enabled: true,
            monthly_enabled: true,
            publish_as_issue: false,
            issue_labels: vec!["report".to_string(), "metrics".to_string()],
        }
    }
}

fn default_report_labels() -> Vec<String> {
    vec!["report".to_string(), "metrics".to_string()]
}

impl SugieConfig {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, anyhow::Error> {
        let content = std::fs::read_to_string(path)?;
        let config: SugieConfig = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn load_from_env_or_file<P: AsRef<Path>>(path: P) -> Result<Self, anyhow::Error> {
        if path.as_ref().exists() {
            Self::load_from_file(path)
        } else {
            // Attempt to build from environment variables
            let app_id: u64 = std::env::var("SUGIE_GITHUB_APP_ID")
                .map_err(|_| anyhow::anyhow!("SUGIE_GITHUB_APP_ID not set"))?
                .parse()?;
            let private_key_pem = std::env::var("SUGIE_GITHUB_PRIVATE_KEY_PEM").ok();
            let private_key_path = std::env::var("SUGIE_GITHUB_PRIVATE_KEY_PATH").ok();
            let webhook_secret = std::env::var("SUGIE_GITHUB_WEBHOOK_SECRET")
                .unwrap_or_else(|_| "secret".to_string());

            Ok(SugieConfig {
                host: std::env::var("SUGIE_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
                port: std::env::var("SUGIE_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(8080),
                db_path: std::env::var("SUGIE_DB_PATH").unwrap_or_else(|_| "sugie.db".to_string()),
                github: GitHubAuthConfig {
                    app_id,
                    private_key_pem,
                    private_key_path,
                    webhook_secret,
                },
                notifications: NotificationsConfig::default(),
                default_rules: RepoConfig::default(),
            })
        }
    }
}

impl RepoConfig {
    pub fn merge_with_override(&self, repo_override: Option<RepoConfig>) -> RepoConfig {
        match repo_override {
            None => self.clone(),
            Some(mut overridden) => {
                // If overridden has empty rules, keep defaults if appropriate
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

use crate::config::SugieConfig;
use crate::github::client::GitHubClient;
use crate::github::models::User;
use crate::metrics::MetricsReporter;
use crate::triage::commands::{CommandExecutor, CommandParser, SlashCommand};
use crate::triage::projects::ProjectsService;
use crate::triage::reviewers::ReviewerLoadBalancer;
use crate::triage::TriageService;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatPlatform {
    Zulip,
    Lark,
    Discord,
    Api,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundSender {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundChatMessage {
    pub platform: ChatPlatform,
    pub sender: InboundSender,
    pub raw_content: String,
    pub stream_or_channel: Option<String>,
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatCommand {
    Help,
    Status,
    Repos,
    Workload { repo: Option<String> },
    Users { repo: Option<String> },
    Metrics { repo: Option<String> },
    Issues { repo: Option<String> },
    Prs { repo: Option<String> },
    Stats { repo: Option<String> },
    Config,
    Stale { repo: Option<String> },
    Comment {
        repo: String,
        number: u64,
        body: String,
    },
    IssueAction {
        repo: String,
        number: u64,
        command: SlashCommand,
    },
    Unknown(String),
}

pub struct ChatCommandParser;

impl ChatCommandParser {
    pub fn parse(raw_text: &str, known_repos: &[String]) -> ChatCommand {
        let cleaned = Self::strip_mentions(raw_text);
        let trimmed = cleaned.trim();

        if trimmed.is_empty() {
            return ChatCommand::Help;
        }

        let mut parts: Vec<&str> = trimmed.split_whitespace().collect();
        // Remove leading bot mentions (e.g., "@SugieBot status" -> "status")
        while let Some(first) = parts.first() {
            if first.starts_with('@') || first.starts_with("<@") {
                parts.remove(0);
            } else {
                break;
            }
        }

        if parts.is_empty() {
            return ChatCommand::Help;
        }

        let first_lower = parts[0].to_lowercase();
        let cmd_key = first_lower.trim_start_matches('/');

        match cmd_key {
            "help" => ChatCommand::Help,
            "status" => ChatCommand::Status,
            "repos" | "repositories" => ChatCommand::Repos,
            "workload" | "workloads" => {
                let repo = parts.get(1).map(|r| Self::resolve_repo(r, known_repos));
                ChatCommand::Workload { repo }
            }
            "users" | "assignees" | "collaborators" | "team" => {
                let repo = parts.get(1).map(|r| Self::resolve_repo(r, known_repos));
                ChatCommand::Users { repo }
            }
            "metrics" | "report" => {
                let repo = parts.get(1).map(|r| Self::resolve_repo(r, known_repos));
                ChatCommand::Metrics { repo }
            }
            "config" => ChatCommand::Config,
            "stats" => {
                let repo = parts.get(1).map(|r| Self::resolve_repo(r, known_repos));
                ChatCommand::Stats { repo }
            }
            "stale" => {
                let repo = parts.get(1).map(|r| Self::resolve_repo(r, known_repos));
                ChatCommand::Stale { repo }
            }
            "comment" => {
                // `/comment <repo>#<num> <text>` or `/comment <repo> <num> <text>`
                if parts.len() >= 3 {
                    if let Some((repo, number)) = Self::parse_repo_issue_target(parts[1], known_repos) {
                        return ChatCommand::Comment {
                            repo,
                            number,
                            body: parts[2..].join(" "),
                        };
                    }
                    if parts.len() >= 4 {
                        if let Ok(number) = parts[2].trim_start_matches('#').parse::<u64>() {
                            return ChatCommand::Comment {
                                repo: Self::resolve_repo(parts[1], known_repos),
                                number,
                                body: parts[3..].join(" "),
                            };
                        }
                    }
                }
                ChatCommand::Unknown(trimmed.to_string())
            }
            "issues" | "prs" | "pulls" => {
                let repo = parts.get(1).map(|r| Self::resolve_repo(r, known_repos));
                if cmd_key == "issues" {
                    ChatCommand::Issues { repo }
                } else {
                    ChatCommand::Prs { repo }
                }
            }
            _ => {
                // Pattern 1: `[repo#num] /command [args...]`
                // e.g. "StarLabsSI/ensia#5 /assign @alice" or "ensia#12 /reviewer @alice"
                if let Some((repo, number)) = Self::parse_repo_issue_target(parts[0], known_repos) {
                    let remaining_cmd = parts[1..].join(" ");
                    if let Some(cmd) = Self::parse_single_slash_command(&remaining_cmd) {
                        return ChatCommand::IssueAction {
                            repo,
                            number,
                            command: cmd,
                        };
                    }
                }

                // Pattern 2a: `/reviewer set <repo#num> @a @b` (sub-command before the target)
                if parts.len() >= 3
                    && matches!(parts[1].to_lowercase().as_str(), "set" | "replace" | "change" | "swap")
                {
                    if let Some((repo, number)) = Self::parse_repo_issue_target(parts[2], known_repos) {
                        let mut reconstructed = vec![parts[0], parts[1]];
                        reconstructed.extend_from_slice(&parts[3..]);
                        let cmd_str = reconstructed.join(" ");
                        if let Some(cmd) = Self::parse_single_slash_command(&cmd_str) {
                            return ChatCommand::IssueAction {
                                repo,
                                number,
                                command: cmd,
                            };
                        }
                    }
                }

                // Pattern 2: `/command [repo#num] [args...]`
                // e.g. "/reviewer ensia#12 +@alice -@bob" or "/close StarLabsSI/ensia#5 completed"
                if parts.len() >= 2 {
                    if let Some((repo, number)) = Self::parse_repo_issue_target(parts[1], known_repos) {
                        let mut reconstructed = vec![parts[0]];
                        reconstructed.extend_from_slice(&parts[2..]);
                        let cmd_str = reconstructed.join(" ");
                        if let Some(cmd) = Self::parse_single_slash_command(&cmd_str) {
                            return ChatCommand::IssueAction {
                                repo,
                                number,
                                command: cmd,
                            };
                        }
                    }
                }

                // Pattern 3: `/command <repo> <number> [args...]`
                // e.g. "/retest ensia 12"
                if parts.len() >= 3 {
                    if let Ok(num) = parts[2].parse::<u64>() {
                        let repo = Self::resolve_repo(parts[1], known_repos);
                        let mut reconstructed = vec![parts[0]];
                        reconstructed.extend_from_slice(&parts[3..]);
                        let cmd_str = reconstructed.join(" ");
                        if let Some(cmd) = Self::parse_single_slash_command(&cmd_str) {
                            return ChatCommand::IssueAction {
                                repo,
                                number: num,
                                command: cmd,
                            };
                        }
                    }
                }

                ChatCommand::Unknown(trimmed.to_string())
            }
        }
    }

    pub fn strip_mentions(text: &str) -> String {
        let mut out = String::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            // Zulip mention: @**...**
            if i + 2 < chars.len() && chars[i] == '@' && chars[i + 1] == '*' && chars[i + 2] == '*' {
                i += 3;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '*') {
                    i += 1;
                }
                i += 2; // skip **
                continue;
            }
            // Discord mention: <@...>
            if i + 1 < chars.len() && chars[i] == '<' && chars[i + 1] == '@' {
                i += 2;
                while i < chars.len() && chars[i] != '>' {
                    i += 1;
                }
                i += 1; // skip >
                continue;
            }
            // Lark mention: @_user_1
            if i + 1 < chars.len() && chars[i] == '@' && chars[i + 1] == '_' {
                i += 2;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                continue;
            }

            out.push(chars[i]);
            i += 1;
        }

        out
    }

    fn parse_repo_issue_target(token: &str, known_repos: &[String]) -> Option<(String, u64)> {
        if let Some((r, num_str)) = token.split_once('#') {
            if let Ok(num) = num_str.parse::<u64>() {
                let resolved = Self::resolve_repo(r, known_repos);
                return Some((resolved, num));
            }
        }
        None
    }

    pub fn resolve_repo(input: &str, known_repos: &[String]) -> String {
        let trimmed = input.trim();
        if trimmed.contains('/') {
            return trimmed.to_string();
        }

        for repo in known_repos {
            if let Some((_, name)) = repo.split_once('/') {
                if name.eq_ignore_ascii_case(trimmed) {
                    return repo.clone();
                }
            }
        }

        trimmed.to_string()
    }

    fn parse_single_slash_command(text: &str) -> Option<SlashCommand> {
        let mut config = crate::config::CommandsConfig::default();
        config.prefix = "/".to_string();
        let mut text_to_parse = text.trim().to_string();
        if !text_to_parse.starts_with('/') {
            text_to_parse = format!("/{}", text_to_parse);
        }
        let cmds = CommandParser::parse_comment(&text_to_parse, &config);
        cmds.into_iter().next()
    }
}

pub struct ChatCommandExecutor<'a> {
    config: &'a SugieConfig,
    triage: &'a TriageService,
}

impl<'a> ChatCommandExecutor<'a> {
    pub fn new(config: &'a SugieConfig, triage: &'a TriageService) -> Self {
        Self { config, triage }
    }

    pub async fn execute(&self, message: &InboundChatMessage) -> String {
        let known_repos = self.get_known_repositories();
        let cmd = ChatCommandParser::parse(&message.raw_content, &known_repos);

        match cmd {
            ChatCommand::Help => self.render_help(),
            ChatCommand::Status => self.render_status().await,
            ChatCommand::Repos => self.render_repos(),
            ChatCommand::Workload { repo } => self.render_workload(repo.as_deref()),
            ChatCommand::Users { repo } => self.render_users(repo.as_deref()).await,
            ChatCommand::Metrics { repo } => self.render_metrics(repo.as_deref()).await,
            ChatCommand::Issues { repo } => self.render_open_items(repo.as_deref(), false).await,
            ChatCommand::Prs { repo } => self.render_open_items(repo.as_deref(), true).await,
            ChatCommand::Stats { repo } => self.render_stats(repo.as_deref()).await,
            ChatCommand::Config => self.render_config(),
            ChatCommand::Stale { repo } => {
                self.run_stale(&message.sender, repo.as_deref()).await
            }
            ChatCommand::Comment { repo, number, body } => {
                self.post_comment(&message.sender, &repo, number, &body).await
            }
            ChatCommand::IssueAction {
                repo,
                number,
                command,
            } => {
                self.execute_issue_action(&message.sender, &repo, number, command)
                    .await
            }
            ChatCommand::Unknown(text) => {
                format!(
                    "❓ Unknown command `{}`.\nType `help` or `/help` to see available bot commands.",
                    text
                )
            }
        }
    }

    fn get_known_repositories(&self) -> Vec<String> {
        let mut repos = Vec::new();
        if let Some(ref allowed) = self.config.notifications.allowed_repositories {
            repos.extend(allowed.clone());
        }
        if let Ok(db_repos) = self.triage.storage().list_repositories() {
            for (_, name, _) in db_repos {
                if !repos.contains(&name) {
                    repos.push(name);
                }
            }
        }
        repos
    }

    fn render_help(&self) -> String {
        r#"
### 🤖 Sugie Bot Chat Commands Reference

You can send commands directly to Sugie from this chat:

#### 📊 General & Status Commands
| Command | Example | Description |
| :--- | :--- | :--- |
| `status` | `@bot status` | Shows bot health, version, and monitored repos |
| `repos` | `@bot repos` | Lists all onboarded repositories |
| `workload [repo]` | `@bot workload ensia` | Displays reviewer active review workloads |
| `users [repo]` | `@bot users ensia` | Lists eligible GitHub assignees & reviewer pool |
| `metrics [repo]` | `@bot metrics ensia` | Displays 7-day issue & PR triage digest |
| `stats [repo]` | `@bot stats ensia` | Issue / PR / commit statistics |
| `config` | `@bot config` | Shows the active configuration summary |
| `issues [repo]` | `@bot issues ensia` | Lists open issues (first 10) |
| `prs [repo]` | `@bot prs ensia` | Lists open PRs with their requested reviewers |
| `help` | `@bot help` | Displays this help manual |

#### 🛠️ Repository & Issue/PR Actions
Format: `<repo>#<number> /command` or `/command <repo>#<number>`
| Command | Example | Description |
| :--- | :--- | :--- |
| `/reviewer` | `/reviewer ensia#12 @alice @bob` | Requests reviewers (PRs only) |
| `/reviewer` | `/reviewer ensia#12 +@alice -@bob` | Adds & removes reviewers |
| `/reviewer set` | `/reviewer set ensia#12 @alice @bob` | **Replaces** all requested reviewers with exactly this list |
| `/reassign` | `/reassign ensia#12 @carol` | Alias of `/reviewer set` (change reviewer) |
| `/unreviewer` | `/unreviewer ensia#12 @bob` | Removes requested reviewer |
| `/reviewer` | `/reviewer ensia#12` | Auto-assigns least-busy reviewers |
| `/assign` | `/assign ensia#12 @alice` | Assigns user(s) to issue or PR |
| `/unassign` | `/unassign ensia#12 @alice` | Removes assignment |
| `/label` | `/label ensia#12 +bug -wip` | Adds/removes labels on GitHub |
| `/comment` | `/comment ensia#12 Fixed in v0.2.1` | Posts a comment (super users only) |
| `/stale` | `/stale ensia` | Runs a stale sweep on a repo (super users only) |
| `/unlabel` | `/unlabel ensia#12 wip` | Removes label(s) |
| `/project` | `/project ensia#12 1 Triage` | Adds to a GitHub Project v2 board |
| `/move` | `/move ensia#12 org/other-repo` | Moves issue to another repository |
| `/duplicate` | `/duplicate ensia#12 #5` | Closes as duplicate of #5 |
| `/close` | `/close ensia#12 completed` | Closes issue or PR |
| `/reopen` | `/reopen ensia#12` | Reopens issue or PR |
| `/retest` | `/retest ensia#12` | Retests failed CI checks (PRs only) |
"#
        .trim()
        .to_string()
    }

    async fn render_status(&self) -> String {
        let repos = self
            .triage
            .storage()
            .list_repositories()
            .unwrap_or_default();
        let channels = &self.config.notifications;
        let mut ch_list = Vec::new();
        if channels.discord.is_some() {
            ch_list.push("Discord");
        }
        if channels.zulip.is_some() {
            ch_list.push("Zulip");
        }
        if channels.lark.is_some() {
            ch_list.push("Lark");
        }

        format!(
            "### 🤖 Sugie Bot Status\n\n\
            - **Status**: 🟢 Running\n\
            - **Version**: `v{}`\n\
            - **Monitored Repositories**: {}\n\
            - **Notification Channels**: {}\n\
            - **Super Users**: {}\n\
            - **Restricted Commands**: {}\n",
            env!("CARGO_PKG_VERSION"),
            repos.len(),
            if ch_list.is_empty() {
                "None configured".to_string()
            } else {
                ch_list.join(", ")
            },
            if self.config.default_rules.commands.super_users.is_empty() {
                "(None)".to_string()
            } else {
                self.config
                    .default_rules
                    .commands
                    .super_users
                    .join(", ")
            },
            if self
                .config
                .default_rules
                .commands
                .restricted_commands
                .is_empty()
            {
                "(None)".to_string()
            } else {
                self.config
                    .default_rules
                    .commands
                    .restricted_commands
                    .join(", ")
            }
        )
    }

    fn render_repos(&self) -> String {
        let repos = self
            .triage
            .storage()
            .list_repositories()
            .unwrap_or_default();
        if repos.is_empty() {
            return "📦 No repositories currently onboarded in local storage.".to_string();
        }

        let mut lines = vec![format!("📦 **Onboarded Repositories ({})**:", repos.len())];
        for (id, name, inst_id) in repos {
            lines.push(format!("- `{}` (ID: {}, Installation: {})", name, id, inst_id));
        }
        lines.join("\n")
    }

    fn render_workload(&self, target_repo: Option<&str>) -> String {
        let workloads = match self.triage.storage().list_all_reviewer_workloads(target_repo) {
            Ok(w) => w,
            Err(e) => return format!("⚠️ Error retrieving workloads: {}", e),
        };

        if workloads.is_empty() {
            return if let Some(r) = target_repo {
                format!("👥 No reviewer workloads tracked for `{}`.", r)
            } else {
                "👥 No reviewer workloads currently recorded.".to_string()
            };
        }

        let title = if let Some(r) = target_repo {
            format!("👥 **Reviewer Workloads for `{}`**:", r)
        } else {
            "👥 **Active Reviewer Workloads (Least Busy Ranking)**:".to_string()
        };

        let mut lines = vec![title];
        for (username, repo, active) in workloads {
            lines.push(format!("- **@{}** (`{}`): **{}** active reviews", username, repo, active));
        }
        lines.join("\n")
    }

    fn is_super_user(&self, sender: &InboundSender) -> bool {
        self.config
            .default_rules
            .commands
            .super_users
            .iter()
            .any(|su| {
                let clean = su.trim_start_matches('@');
                clean.eq_ignore_ascii_case(&sender.username) || clean.eq_ignore_ascii_case(&sender.id)
            })
    }

    async fn client_for(&self, repo: &str) -> Result<GitHubClient> {
        match self.triage.get_client_for_repo(repo).await {
            Ok(c) => Ok(c),
            Err(_) => self.find_client_dynamically(repo).await,
        }
    }

    fn default_repo(&self, target: Option<&str>) -> Option<String> {
        target
            .map(|s| s.to_string())
            .or_else(|| self.get_known_repositories().first().cloned())
    }

    async fn render_users(&self, target_repo: Option<&str>) -> String {
        let Some(repo) = self.default_repo(target_repo) else {
            return "⚠️ Please specify repository: `users <owner/repo>`.".to_string();
        };
        let Some((owner, repo_name)) = repo.split_once('/') else {
            return format!("⚠️ Invalid repository format `{}`.", repo);
        };
        let client = match self.client_for(&repo).await {
            Ok(c) => c,
            Err(e) => return format!("❌ Could not access repository `{}`: {}", repo, e),
        };

        let repo_config = self.triage.load_repo_config(&client, owner, repo_name).await;
        let gh_assignees: Vec<String> = client
            .list_assignees(owner, repo_name)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|u| u.login)
            .collect();
        let workloads = self
            .triage
            .storage()
            .list_all_reviewer_workloads(Some(&repo))
            .unwrap_or_default();

        let mut ordered: Vec<String> = Vec::new();
        let mut push_unique = |name: &str| {
            let clean = name.trim().trim_start_matches('@');
            if !clean.is_empty() && !ordered.iter().any(|x| x.eq_ignore_ascii_case(clean)) {
                ordered.push(clean.to_string());
            }
        };

        for u in &repo_config.reviewers.pool {
            push_unique(u);
        }
        for u in &repo_config.commands.super_users {
            push_unique(u);
        }
        for (u, _, _) in &workloads {
            push_unique(u);
        }
        for u in &gh_assignees {
            push_unique(u);
        }

        if ordered.is_empty() {
            return format!("👥 No eligible assignees or reviewers found for `{}`.", repo);
        }

        let mut lines = vec![format!(
            "👥 **Available GitHub Users for `{}` ({})**:",
            repo,
            ordered.len()
        )];
        for login in ordered {
            let mut tags = Vec::new();
            if repo_config
                .reviewers
                .pool
                .iter()
                .any(|p| p.trim_start_matches('@').eq_ignore_ascii_case(&login))
            {
                tags.push("Reviewer Pool".to_string());
            }
            if repo_config
                .commands
                .super_users
                .iter()
                .any(|s| s.trim_start_matches('@').eq_ignore_ascii_case(&login))
            {
                tags.push("Super User".to_string());
            }
            if gh_assignees.iter().any(|g| g.eq_ignore_ascii_case(&login)) {
                tags.push("GitHub Assignee".to_string());
            }
            if let Some((_, _, active)) = workloads
                .iter()
                .find(|(w, _, _)| w.eq_ignore_ascii_case(&login))
            {
                tags.push(format!("{active} active reviews"));
            }
            if tags.is_empty() {
                lines.push(format!("- **@{}**", login));
            } else {
                lines.push(format!("- **@{}** ({})", login, tags.join(" · ")));
            }
        }
        lines.join("\n")
    }

    async fn render_stats(&self, target_repo: Option<&str>) -> String {
        let Some(repo) = self.default_repo(target_repo) else {
            return "⚠️ Please specify repository: `stats <owner/repo>`.".to_string();
        };
        let cache = crate::web::stats::StatsCache::default();
        let s = crate::web::stats::fetch(self.triage, &cache, &repo, true).await;
        if let Some(e) = s.error {
            return format!("⚠️ Could not fetch stats for `{}`: {}", repo, e);
        }
        format!(
            "📈 **Stats for `{}`**\n\n- Open issues: **{}**\n- Open PRs: **{}**\n- Commits (30d): **{}**\n- Merged PRs (30d): **{}**\n- Stars: **{}** · Forks: **{}**\n- Default branch: `{}`",
            s.full_name, s.open_issues, s.open_prs, s.commits_30d, s.merged_30d, s.stars, s.forks, s.default_branch
        )
    }

    fn render_config(&self) -> String {
        let s = crate::web::config_store::ConfigSummary::from_config(self.config);
        format!(
            "⚙️ **Active Configuration**\n\n- Listen: `{}:{}`\n- Notification channels: {}\n- Repository filter: {}\n- Super users: {}\n- Restricted commands: {}\n- Stale sweep: {} · Auto reviewers: {} · Auto labels: {}",
            s.host,
            s.port,
            if s.channels.is_empty() { "None".to_string() } else { s.channels.join(", ") },
            if s.allowed_repositories.is_empty() { "all onboarded repositories".to_string() } else { s.allowed_repositories.join(", ") },
            if s.super_users.is_empty() { "(none)".to_string() } else { s.super_users.join(", ") },
            if s.restricted_commands.is_empty() { "(none)".to_string() } else { format!("/{}", s.restricted_commands.join(", /")) },
            if s.stale_enabled { "on" } else { "off" },
            if s.reviewers_enabled { "on" } else { "off" },
            if s.labels_enabled { "on" } else { "off" },
        )
    }

    async fn run_stale(&self, sender: &InboundSender, target_repo: Option<&str>) -> String {
        if !self.is_super_user(sender) {
            return format!(
                "⛔ Sorry @{}, `/stale` is restricted to authorized super users.",
                sender.username
            );
        }
        let rules = self.config.default_rules.stale.clone();
        if !rules.enabled {
            return "⚠️ Stale lifecycle rules are disabled (`default_rules.stale.enabled = false`).".to_string();
        }
        let Some(repo) = self.default_repo(target_repo) else {
            return "⚠️ Please specify repository: `stale <owner/repo>`.".to_string();
        };
        let Some((owner, repo_name)) = repo.split_once('/') else {
            return format!("⚠️ Invalid repository format `{}`.", repo);
        };
        let client = match self.client_for(&repo).await {
            Ok(c) => c,
            Err(e) => return format!("❌ Could not access repository `{}`: {}", repo, e),
        };
        let mgr = crate::stale::StaleManager::new(
            rules,
            self.triage.storage().clone(),
            crate::notifications::NotificationDispatcher::new(&self.config.notifications),
        );
        match mgr.process_repository(&client, owner, repo_name).await {
            Ok(r) => format!(
                "🧹 Stale sweep for `{}` completed: warned **{}** · closed **{}** · locked **{}**",
                repo, r.warned_count, r.closed_count, r.locked_count
            ),
            Err(e) => format!("⚠️ Stale sweep failed for `{}`: {}", repo, e),
        }
    }

    async fn post_comment(
        &self,
        sender: &InboundSender,
        repo: &str,
        number: u64,
        body: &str,
    ) -> String {
        if !self.is_super_user(sender) {
            return format!(
                "⛔ Sorry @{}, `/comment` is restricted to authorized super users.",
                sender.username
            );
        }
        let Some((owner, repo_name)) = repo.split_once('/') else {
            return format!(
                "⚠️ Invalid repository format `{}`. Please specify `owner/repo`.",
                repo
            );
        };
        let client = match self.client_for(repo).await {
            Ok(c) => c,
            Err(e) => return format!("❌ Could not connect to repository `{}`: {}", repo, e),
        };
        let text = format!("{}\n\n— _posted by @{} via Sugie chat_", body.trim(), sender.username);
        match client.create_comment(owner, repo_name, number, &text).await {
            Ok(c) => match c.html_url {
                Some(url) => format!("✅ Commented on [{}#{}]({}) as requested by @{}.", repo, number, url, sender.username),
                None => format!("✅ Commented on {}#{} as requested by @{}.", repo, number, sender.username),
            },
            Err(e) => format!("⚠️ Failed to comment on {}#{}: {}", repo, number, e),
        }
    }

    async fn render_open_items(&self, target_repo: Option<&str>, prs: bool) -> String {
        let repo = match target_repo {
            Some(r) => r.to_string(),
            None => match self.get_known_repositories().first() {
                Some(first) => first.clone(),
                None => {
                    return format!(
                        "⚠️ Please specify repository: `{} <owner/repo>`.",
                        if prs { "prs" } else { "issues" }
                    )
                }
            },
        };
        let Some((owner, repo_name)) = repo.split_once('/') else {
            return format!("⚠️ Invalid repository format `{}`.", repo);
        };
        let client = match self.triage.get_client_for_repo(&repo).await {
            Ok(c) => c,
            Err(_) => match self.find_client_dynamically(&repo).await {
                Ok(c) => c,
                Err(e) => return format!("❌ Could not access repository `{}`: {}", repo, e),
            },
        };
        const LIMIT: usize = 10;
        if prs {
            match client.list_repo_pulls(owner, repo_name, "open", 100).await {
                Ok(list) => {
                    let mut lines = vec![format!(
                        "🔀 **Open PRs in `{}`** ({} total, showing {}):",
                        repo,
                        list.len(),
                        list.len().min(LIMIT)
                    )];
                    for p in list.iter().take(LIMIT) {
                        let reviewers = if p.requested_reviewers.is_empty() {
                            "no reviewers".to_string()
                        } else {
                            p.requested_reviewers
                                .iter()
                                .map(|u| format!("@{}", u.login))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };
                        lines.push(format!(
                            "- #{} {} — @{}{} · reviewers: {}",
                            p.number,
                            p.title,
                            p.user.login,
                            if p.draft { " (draft)" } else { "" },
                            reviewers
                        ));
                    }
                    lines.join("\n")
                }
                Err(e) => format!("⚠️ Failed to fetch PRs for `{}`: {}", repo, e),
            }
        } else {
            match client.list_repo_issues(owner, repo_name, "open", 100).await {
                Ok(list) => {
                    let issues: Vec<_> = list.into_iter().filter(|i| !i.is_pr()).collect();
                    let mut lines = vec![format!(
                        "🐞 **Open issues in `{}`** ({} total, showing {}):",
                        repo,
                        issues.len(),
                        issues.len().min(LIMIT)
                    )];
                    for i in issues.iter().take(LIMIT) {
                        lines.push(format!("- #{} {} — @{}", i.number, i.title, i.user.login));
                    }
                    lines.join("\n")
                }
                Err(e) => format!("⚠️ Failed to fetch issues for `{}`: {}", repo, e),
            }
        }
    }

    async fn render_metrics(&self, target_repo: Option<&str>) -> String {
        let repo = match target_repo {
            Some(r) => r.to_string(),
            None => {
                let known = self.get_known_repositories();
                if let Some(first) = known.first() {
                    first.clone()
                } else {
                    return "⚠️ Please specify repository: `metrics <owner/repo>`.".to_string();
                }
            }
        };

        let client = match self.triage.get_client_for_repo(&repo).await {
            Ok(c) => c,
            Err(e) => return format!("❌ Could not access repository `{}`: {}", repo, e),
        };

        let (owner, repo_name) = match repo.split_once('/') {
            Some((o, r)) => (o, r),
            None => return format!("⚠️ Invalid repository format `{}`.", repo),
        };

        let reporter = MetricsReporter::new(
            self.config.default_rules.reports.clone(),
            self.triage.notifier().clone(),
        );

        match reporter
            .generate_report(&client, owner, repo_name, 7, "Past 7 Days")
            .await
        {
            Ok(metrics) => reporter.format_markdown_report(&metrics),
            Err(e) => format!("⚠️ Error generating metrics for `{}`: {}", repo, e),
        }
    }

    async fn execute_issue_action(
        &self,
        sender: &InboundSender,
        repo: &str,
        number: u64,
        command: SlashCommand,
    ) -> String {
        let cmd_name = command.name();

        // Check if command is restricted
        let is_restricted = self
            .config
            .default_rules
            .commands
            .restricted_commands
            .iter()
            .any(|r| r.trim_start_matches('/').eq_ignore_ascii_case(cmd_name));

        if is_restricted {
            let is_super = self
                .config
                .default_rules
                .commands
                .super_users
                .iter()
                .any(|su| {
                    let clean = su.trim_start_matches('@');
                    clean.eq_ignore_ascii_case(&sender.username)
                        || clean.eq_ignore_ascii_case(&sender.id)
                });

            if !is_super {
                return format!(
                    "⛔ Sorry @{}, the command `/{}` is restricted to authorized super users.",
                    sender.username, cmd_name
                );
            }
        }

        let (owner, repo_name) = match repo.split_once('/') {
            Some((o, r)) => (o, r),
            None => {
                return format!(
                    "⚠️ Invalid repository format `{}`. Please specify `owner/repo` (e.g. `StarLabsSI/ensia`).",
                    repo
                );
            }
        };

        let client = match self.triage.get_client_for_repo(repo).await {
            Ok(c) => c,
            Err(_) => match self.find_client_dynamically(repo).await {
                Ok(c) => c,
                Err(e) => return format!("❌ Could not connect to repository `{}`: {}", repo, e),
            },
        };

        let issue = match client.get_issue(owner, repo_name, number).await {
            Ok(i) => i,
            Err(e) => {
                return format!("❌ Could not find issue or PR `{}#{}`: {}", repo, number, e);
            }
        };

        let user = User {
            login: sender.username.clone(),
            id: 0,
            r#type: Some(format!("{:?}", sender.id)),
        };

        let repo_config = self.triage.load_repo_config(&client, owner, repo_name).await;
        let projects = ProjectsService::new(repo_config.projects.clone());
        let balancer = ReviewerLoadBalancer::new(
            repo_config.reviewers.clone(),
            self.triage.storage().clone(),
        );
        let executor = CommandExecutor::new(
            &client,
            &projects,
            self.triage.notifier(),
            &repo_config.commands,
            Some(balancer),
        );

        match executor
            .execute(&command, owner, repo_name, &issue, &user)
            .await
        {
            Ok(()) => {
                let link = if issue.is_pr() {
                    format!("https://github.com/{}/{}/pull/{}", owner, repo_name, number)
                } else {
                    format!("https://github.com/{}/{}/issues/{}", owner, repo_name, number)
                };
                format!(
                    "✅ Successfully executed `/{}` on [{}#{}]({}) requested by @{}.",
                    cmd_name, repo, number, link, sender.username
                )
            }
            Err(e) => {
                format!(
                    "⚠️ Error executing `/{}` on {}#{}: {}",
                    cmd_name, repo, number, e
                )
            }
        }
    }

    async fn find_client_dynamically(&self, repo: &str) -> Result<GitHubClient> {
        let auth = self.triage.auth();
        let installs = auth.get_installations().await?;
        for inst in installs {
            if let Some(inst_id) = inst.get("id").and_then(|v| v.as_u64()) {
                if let Ok(repos) = auth.get_installation_repositories(inst_id).await {
                    for r in repos {
                        if let (Some(full_name), Some(repo_id)) = (
                            r.get("full_name").and_then(|v| v.as_str()),
                            r.get("id").and_then(|v| v.as_u64()),
                        ) {
                            if full_name.eq_ignore_ascii_case(repo) {
                                let _ = self.triage.storage().upsert_repository(repo_id, full_name, inst_id);
                                return self.triage.get_client_for_installation(inst_id).await;
                            }
                        }
                    }
                }
            }
        }
        anyhow::bail!("Repository {} not found across active GitHub App installations.", repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_command_parsing() {
        let known = vec!["StarLabsSI/ensia".to_string(), "StarLabsSI/bincode".to_string()];

        // Global commands
        assert_eq!(ChatCommandParser::parse("status", &known), ChatCommand::Status);
        assert_eq!(ChatCommandParser::parse("/status", &known), ChatCommand::Status);
        assert_eq!(ChatCommandParser::parse("@**sugie-bot** status", &known), ChatCommand::Status);
        assert_eq!(ChatCommandParser::parse("<@123456> repos", &known), ChatCommand::Repos);
        assert_eq!(ChatCommandParser::parse("@_user_1 help", &known), ChatCommand::Help);
        assert_eq!(
            ChatCommandParser::parse("workload ensia", &known),
            ChatCommand::Workload { repo: Some("StarLabsSI/ensia".to_string()) }
        );

        // Issue actions (Pattern 1: repo#num command)
        assert_eq!(
            ChatCommandParser::parse("ensia#12 /reviewer @alice", &known),
            ChatCommand::IssueAction {
                repo: "StarLabsSI/ensia".to_string(),
                number: 12,
                command: SlashCommand::Reviewer {
                    add: vec!["alice".to_string()],
                    remove: vec![],
                },
            }
        );

        // Issue actions (Pattern 2: command repo#num)
        assert_eq!(
            ChatCommandParser::parse("/close StarLabsSI/bincode#42 not planned", &known),
            ChatCommand::IssueAction {
                repo: "StarLabsSI/bincode".to_string(),
                number: 42,
                command: SlashCommand::Close(Some("not planned".to_string())),
            }
        );

        // Change reviewers (set/replace)
        assert_eq!(
            ChatCommandParser::parse("/reviewer set ensia#12 @alice @bob", &known),
            ChatCommand::IssueAction {
                repo: "StarLabsSI/ensia".to_string(),
                number: 12,
                command: SlashCommand::ReviewerSet(vec!["alice".to_string(), "bob".to_string()]),
            }
        );
        assert_eq!(
            ChatCommandParser::parse("/reassign ensia#12 @carol", &known),
            ChatCommand::IssueAction {
                repo: "StarLabsSI/ensia".to_string(),
                number: 12,
                command: SlashCommand::ReviewerSet(vec!["carol".to_string()]),
            }
        );
        assert_eq!(
            ChatCommandParser::parse("prs ensia", &known),
            ChatCommand::Prs { repo: Some("StarLabsSI/ensia".to_string()) }
        );

        // Issue actions (Pattern 3: command repo num)
        assert_eq!(
            ChatCommandParser::parse("/retest ensia 7", &known),
            ChatCommand::IssueAction {
                repo: "StarLabsSI/ensia".to_string(),
                number: 7,
                command: SlashCommand::Retest,
            }
        );
    }
}

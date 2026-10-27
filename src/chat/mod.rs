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
    Metrics { repo: Option<String> },
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
            "metrics" | "report" => {
                let repo = parts.get(1).map(|r| Self::resolve_repo(r, known_repos));
                ChatCommand::Metrics { repo }
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
            ChatCommand::Metrics { repo } => self.render_metrics(repo.as_deref()).await,
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
| `metrics [repo]` | `@bot metrics ensia` | Displays 7-day issue & PR triage digest |
| `help` | `@bot help` | Displays this help manual |

#### 🛠️ Repository & Issue/PR Actions
Format: `<repo>#<number> /command` or `/command <repo>#<number>`
| Command | Example | Description |
| :--- | :--- | :--- |
| `/reviewer` | `/reviewer ensia#12 @alice @bob` | Requests reviewers (PRs only) |
| `/reviewer` | `/reviewer ensia#12 +@alice -@bob` | Adds & removes reviewers |
| `/unreviewer` | `/unreviewer ensia#12 @bob` | Removes requested reviewer |
| `/reviewer` | `/reviewer ensia#12` | Auto-assigns least-busy reviewers |
| `/assign` | `/assign ensia#12 @alice` | Assigns user(s) to issue or PR |
| `/unassign` | `/unassign ensia#12 @alice` | Removes assignment |
| `/label` | `/label ensia#12 +bug -wip` | Adds/removes labels on GitHub |
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

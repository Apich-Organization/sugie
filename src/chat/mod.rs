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

/// `/issues` and `/prs` in chat: default rows, maximum rows and the character
/// budget for one message. Discord caps a message at 2000 characters and the
/// other platforms are similar, so a long list is trimmed rather than rejected.
const CHAT_LIST_DEFAULT: usize = 10;
const CHAT_LIST_MAX: usize = 25;
const CHAT_LIST_BUDGET: usize = 1800;
/// Raw API pages one chat list may walk (the issues endpoint interleaves PRs).
const CHAT_LIST_PAGE_SIZE: u32 = 100;
const CHAT_LIST_MAX_RAW_PAGES: u32 = 5;

/// Compose one chat list message from an already-fetched window.
///
/// Pure on purpose: the length budget and the "next page" footer are the parts
/// most likely to break a platform's message limit, and they are worth testing
/// without a GitHub client in the way.
#[allow(clippy::too_many_arguments)]
fn assemble_list_message(
    icon: &str,
    noun: &str,
    repo: &str,
    cmd: &str,
    page: usize,
    count: usize,
    offset: usize,
    lines: &[String],
    total: Option<u64>,
) -> String {
    if lines.is_empty() {
        return match total {
            Some(0) => format!("{icon} No open {noun} in `{repo}`."),
            _ => format!("{icon} No open {noun} on page {page} of `{repo}` ({count} per page)."),
        };
    }

    let first = offset + 1;
    let last = offset + lines.len();
    let mut out = vec![match total {
        Some(n) => {
            format!("{icon} **Open {noun} in `{repo}`** ({n} total, showing {first}–{last}):")
        }
        None => format!("{icon} **Open {noun} in `{repo}`** (showing {first}–{last}):"),
    }];

    // Stay inside the platform limit instead of letting Discord reject the whole
    // message. Titles vary in length, so the budget is measured in characters,
    // not in rows.
    let mut used = out[0].chars().count();
    let mut shown = 0usize;
    for line in lines {
        let cost = line.chars().count() + 1;
        if used + cost > CHAT_LIST_BUDGET {
            break;
        }
        used += cost;
        out.push(line.clone());
        shown += 1;
    }

    if shown < lines.len() {
        out.push(format!(
            "… truncated to fit the message limit. Ask for fewer: `{cmd} {repo} {CHAT_LIST_DEFAULT} {page}`"
        ));
    } else {
        let last_shown = offset + shown;
        let more = match total {
            Some(n) => (last_shown as u64) < n,
            None => true,
        };
        if more {
            out.push(format!("Next page: `{cmd} {repo} {count} {}`", page + 1));
        }
    }
    out.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatPlatform {
    Zulip,
    Lark,
    Discord,
    Api,
}

impl ChatPlatform {
    /// Stable lowercase label, used as the stored value and as the filter value
    /// in the console's chat history.
    pub fn as_str(&self) -> &'static str {
        match self {
            ChatPlatform::Zulip => "zulip",
            ChatPlatform::Lark => "lark",
            ChatPlatform::Discord => "discord",
            ChatPlatform::Api => "api",
        }
    }
}

/// Classify a bot reply for the console's chat history.
///
/// The executor reports problems with a leading marker rather than a `Result`,
/// so the recorded outcome is derived from the text the user actually saw:
/// `❌` is a hard failure, `⚠️`/`❓` is a refusal or misuse, everything else —
/// help text, listings, `✅` confirmations — counts as a successful turn.
pub fn reply_status(reply: &str) -> &'static str {
    let head = reply.trim_start();
    if head.starts_with('❌') {
        "error"
    } else if head.starts_with('⚠') || head.starts_with('❓') {
        "warn"
    } else {
        "ok"
    }
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
    Workload {
        repo: Option<String>,
    },
    Users {
        repo: Option<String>,
    },
    Metrics {
        repo: Option<String>,
    },
    Issues {
        repo: Option<String>,
        /// Rows per message (clamped) and the 1-based page of issues to show.
        count: usize,
        page: usize,
    },
    Prs {
        repo: Option<String>,
        count: usize,
        page: usize,
    },
    Stats {
        repo: Option<String>,
    },
    Config,
    ChatId,
    Stale {
        repo: Option<String>,
    },
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
            "chatid" | "channel" | "channelid" | "where" | "id" => ChatCommand::ChatId,
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
                    if let Some((repo, number)) =
                        Self::parse_repo_issue_target(parts[1], known_repos)
                    {
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
                // `issues [repo] [count] [page]` — the chat surfaces have no
                // pagination UI, so the window is requested in the message.
                let (repo, count, page) = Self::parse_list_options(&parts[1..], known_repos);
                if cmd_key == "issues" {
                    ChatCommand::Issues { repo, count, page }
                } else {
                    ChatCommand::Prs { repo, count, page }
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
                    && matches!(
                        parts[1].to_lowercase().as_str(),
                        "set" | "replace" | "change" | "swap"
                    )
                {
                    if let Some((repo, number)) =
                        Self::parse_repo_issue_target(parts[2], known_repos)
                    {
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
                    if let Some((repo, number)) =
                        Self::parse_repo_issue_target(parts[1], known_repos)
                    {
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
            if i + 2 < chars.len() && chars[i] == '@' && chars[i + 1] == '*' && chars[i + 2] == '*'
            {
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

    /// Parse `[repo] [count] [page]` in any order for `/issues` and `/prs`.
    ///
    /// Chat has no pagination controls, so a number in the message is the only
    /// way to ask for more than the default ten. Numeric tokens fill count, then
    /// page; the first non-numeric token is the repository.
    fn parse_list_options(args: &[&str], known_repos: &[String]) -> (Option<String>, usize, usize) {
        let mut repo = None;
        let mut numbers = Vec::new();
        for arg in args {
            match arg.trim_start_matches('#').parse::<usize>() {
                Ok(n) => numbers.push(n),
                Err(_) => {
                    if repo.is_none() {
                        repo = Some(Self::resolve_repo(arg, known_repos));
                    }
                }
            }
        }
        let count = numbers
            .first()
            .copied()
            .unwrap_or(CHAT_LIST_DEFAULT)
            .clamp(1, CHAT_LIST_MAX);
        let page = numbers.get(1).copied().unwrap_or(1).max(1);
        (repo, count, page)
    }

    fn parse_single_slash_command(text: &str) -> Option<SlashCommand> {
        let config = crate::config::CommandsConfig {
            prefix: "/".to_string(),
            ..Default::default()
        };
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
            ChatCommand::Status => self.render_status(Some(message)).await,
            ChatCommand::ChatId => self.render_chat_id(message),
            ChatCommand::Repos => self.render_repos(),
            ChatCommand::Workload { repo } => self.render_workload(repo.as_deref()),
            ChatCommand::Users { repo } => self.render_users(repo.as_deref()).await,
            ChatCommand::Metrics { repo } => self.render_metrics(repo.as_deref()).await,
            ChatCommand::Issues { repo, count, page } => {
                self.render_open_items(repo.as_deref(), false, count, page)
                    .await
            }
            ChatCommand::Prs { repo, count, page } => {
                self.render_open_items(repo.as_deref(), true, count, page)
                    .await
            }
            ChatCommand::Stats { repo } => self.render_stats(repo.as_deref()).await,
            ChatCommand::Config => self.render_config(),
            ChatCommand::Stale { repo } => self.run_stale(&message.sender, repo.as_deref()).await,
            ChatCommand::Comment { repo, number, body } => {
                self.post_comment(&message.sender, &repo, number, &body)
                    .await
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
        r#"### 🤖 Sugie Bot Commands Reference

**📊 General & Status Commands**
• `/status` — Bot health, version, and monitored repos
• `/repos` — List onboarded repositories
• `/workload [repo]` — Reviewer active review workloads
• `/users [repo]` — Eligible assignees & reviewer pool
• `/metrics [repo]` — 7-day issue & PR triage digest
• `/stats [repo]` — Issue, PR, and commit statistics
• `/config` — Active configuration summary
• `/chatid` — Display current group / channel ID
• `/issues [repo] [count] [page]` — List open issues (e.g. `/issues 10`)
• `/prs [repo] [count] [page]` — List open PRs with reviewers
• `/help` — Displays this command reference

**🛠️ Issue & PR Actions** (`<repo>#<num> /command` or `/command <repo>#<num>`)
• `/assign @user` / `/unassign @user` — Assign / unassign user(s)
• `/label +bug -wip` / `/unlabel wip` — Add or remove labels
• `/reviewer @user` / `/unreviewer @user` — Request / remove PR reviewer
• `/reviewer set @user` — Replace all reviewers
• `/close [reason]` / `/reopen` — Close or reopen issue/PR
• `/project <num> <column>` — Add to GitHub Project v2 board
• `/move <target_repo>` — Move issue to another repository
• `/duplicate #num` — Close as duplicate
• `/retest` — Retest failed CI checks (PRs only)
• `/comment <text>` — Post comment (super users only)
• `/stale` — Run stale sweep (super users only)"#
            .to_string()
    }

    fn render_chat_id(&self, message: &InboundChatMessage) -> String {
        let platform_name = match message.platform {
            ChatPlatform::Lark => "Lark (Feishu)",
            ChatPlatform::Discord => "Discord",
            ChatPlatform::Zulip => "Zulip",
            ChatPlatform::Api => "Generic API",
        };
        let channel = message
            .stream_or_channel
            .as_deref()
            .unwrap_or("(direct message / unassigned)");

        let toml_snippet = match message.platform {
            ChatPlatform::Lark => format!(
                "\n\n💡 To register this channel in `sugie.toml`:\n```toml\n[[notifications.lark.channels]]\nname = \"my-channel\"\nchat_id = \"{}\"\n```",
                channel
            ),
            ChatPlatform::Discord => format!(
                "\n\n💡 To register this channel in `sugie.toml`:\n```toml\n[[notifications.discord.bot_channels]]\nname = \"my-channel\"\nchannel_id = \"{}\"\n```",
                channel
            ),
            ChatPlatform::Zulip => format!(
                "\n\n💡 To register this channel in `sugie.toml`:\n```toml\n[[notifications.zulip.channels]]\nname = \"my-channel\"\nchannel = \"{}\"\ntopic = \"announcements\"\n```",
                channel
            ),
            ChatPlatform::Api => String::new(),
        };

        format!(
            "### 📍 Chat / Channel Information\n\n\
            - **Platform**: {}\n\
            - **Current Chat ID**: `{}`\
            {}",
            platform_name, channel, toml_snippet
        )
    }

    async fn render_status(&self, message: Option<&InboundChatMessage>) -> String {
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

        let current_chat_info = if let Some(m) = message {
            if let Some(ref ch) = m.stream_or_channel {
                format!("- **Current Chat ID**: `{}`\n", ch)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        format!(
            "### 🤖 Sugie Bot Status\n\n\
            - **Status**: 🟢 Running\n\
            - **Version**: `v{}`\n\
            - **Monitored Repositories**: {}\n\
            - **Notification Channels**: {}\n\
            {} - **Super Users**: {}\n\
            - **Restricted Commands**: {}\n",
            env!("CARGO_PKG_VERSION"),
            repos.len(),
            if ch_list.is_empty() {
                "None configured".to_string()
            } else {
                ch_list.join(", ")
            },
            current_chat_info,
            if self.config.default_rules.commands.super_users.is_empty() {
                "(None)".to_string()
            } else {
                self.config.default_rules.commands.super_users.join(", ")
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
            lines.push(format!(
                "- `{}` (ID: {}, Installation: {})",
                name, id, inst_id
            ));
        }
        lines.join("\n")
    }

    fn render_workload(&self, target_repo: Option<&str>) -> String {
        let workloads = match self
            .triage
            .storage()
            .list_all_reviewer_workloads(target_repo)
        {
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
            lines.push(format!(
                "- **@{}** (`{}`): **{}** active reviews",
                username, repo, active
            ));
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
                clean.eq_ignore_ascii_case(&sender.username)
                    || clean.eq_ignore_ascii_case(&sender.id)
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

        let repo_config = self
            .triage
            .load_repo_config(&client, owner, repo_name)
            .await;
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
            return format!(
                "👥 No eligible assignees or reviewers found for `{}`.",
                repo
            );
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
        let text = format!(
            "{}\n\n— _posted by @{} via Sugie chat_",
            body.trim(),
            sender.username
        );
        match client.create_comment(owner, repo_name, number, &text).await {
            Ok(c) => match c.html_url {
                Some(url) => format!(
                    "✅ Commented on [{}#{}]({}) as requested by @{}.",
                    repo, number, url, sender.username
                ),
                None => format!(
                    "✅ Commented on {}#{} as requested by @{}.",
                    repo, number, sender.username
                ),
            },
            Err(e) => format!("⚠️ Failed to comment on {}#{}: {}", repo, number, e),
        }
    }

    /// `/issues` and `/prs` for the chat platforms.
    ///
    /// `count`/`page` describe the window to show. The issues endpoint returns
    /// issues and pull requests interleaved, so a window is filled by walking
    /// raw pages (bounded); the header reports the repository total from the
    /// exact counters, not from the length of the page we happened to fetch.
    async fn render_open_items(
        &self,
        target_repo: Option<&str>,
        prs: bool,
        count: usize,
        page: usize,
    ) -> String {
        let cmd = if prs { "prs" } else { "issues" };
        let repo = match target_repo {
            Some(r) => r.to_string(),
            None => match self.get_known_repositories().first() {
                Some(first) => first.clone(),
                None => return format!("⚠️ Please specify repository: `{cmd} <owner/repo>`."),
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

        let count = count.clamp(1, CHAT_LIST_MAX);
        let page = page.max(1);
        let offset = (page - 1).saturating_mul(count);

        // `lines` holds the rows for the requested window; `total` is the
        // repository-wide count when the API answered the counter query.
        let (icon, noun, lines, total) = if prs {
            let total = client.count_open_pulls(owner, repo_name).await.ok();
            match client
                .list_repo_pulls_page(owner, repo_name, "open", count as u32, page as u32)
                .await
            {
                Ok(list) => {
                    let lines = list
                        .iter()
                        .map(|p| {
                            let reviewers = if p.requested_reviewers.is_empty() {
                                "no reviewers".to_string()
                            } else {
                                p.requested_reviewers
                                    .iter()
                                    .map(|u| format!("@{}", u.login))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            format!(
                                "- #{} {} — @{}{} · reviewers: {}",
                                p.number,
                                p.title,
                                p.user.login,
                                if p.draft { " (draft)" } else { "" },
                                reviewers
                            )
                        })
                        .collect::<Vec<_>>();
                    ("🔀", "PRs", lines, total)
                }
                Err(e) => return format!("⚠️ Failed to fetch PRs for `{}`: {}", repo, e),
            }
        } else {
            let total = match tokio::join!(
                client.count_open_issues_and_prs(owner, repo_name),
                client.count_open_pulls(owner, repo_name)
            ) {
                (Ok(all), Ok(prs)) => Some(all.saturating_sub(prs)),
                _ => None,
            };
            let want = offset + count;
            let mut issues = Vec::new();
            let mut raw_page = 1;
            while issues.len() < want && raw_page <= CHAT_LIST_MAX_RAW_PAGES {
                match client
                    .list_repo_issues_page(owner, repo_name, "open", CHAT_LIST_PAGE_SIZE, raw_page)
                    .await
                {
                    Ok(batch) => {
                        let short_page = batch.len() < CHAT_LIST_PAGE_SIZE as usize;
                        issues.extend(batch.into_iter().filter(|i| !i.is_pr()));
                        if short_page {
                            break;
                        }
                    }
                    Err(e) => return format!("⚠️ Failed to fetch issues for `{}`: {}", repo, e),
                }
                raw_page += 1;
            }
            let start = offset.min(issues.len());
            let end = (offset + count).min(issues.len());
            let lines = issues[start..end]
                .iter()
                .map(|i| format!("- #{} {} — @{}", i.number, i.title, i.user.login))
                .collect::<Vec<_>>();
            ("🐞", "issues", lines, total)
        };

        assemble_list_message(icon, noun, &repo, cmd, page, count, offset, &lines, total)
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

        let repo_config = self
            .triage
            .load_repo_config(&client, owner, repo_name)
            .await;
        let projects = ProjectsService::new(repo_config.projects.clone());
        let balancer =
            ReviewerLoadBalancer::new(repo_config.reviewers.clone(), self.triage.storage().clone());
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
                    format!(
                        "https://github.com/{}/{}/issues/{}",
                        owner, repo_name, number
                    )
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
                                let _ = self
                                    .triage
                                    .storage()
                                    .upsert_repository(repo_id, full_name, inst_id);
                                return self.triage.get_client_for_installation(inst_id).await;
                            }
                        }
                    }
                }
            }
        }
        anyhow::bail!(
            "Repository {} not found across active GitHub App installations.",
            repo
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_command_parsing() {
        let known = vec![
            "StarLabsSI/ensia".to_string(),
            "StarLabsSI/bincode".to_string(),
        ];

        // Global commands
        assert_eq!(
            ChatCommandParser::parse("status", &known),
            ChatCommand::Status
        );
        assert_eq!(
            ChatCommandParser::parse("/status", &known),
            ChatCommand::Status
        );
        assert_eq!(
            ChatCommandParser::parse("@**sugie-bot** status", &known),
            ChatCommand::Status
        );
        assert_eq!(
            ChatCommandParser::parse("<@123456> repos", &known),
            ChatCommand::Repos
        );
        assert_eq!(
            ChatCommandParser::parse("@_user_1 help", &known),
            ChatCommand::Help
        );
        assert_eq!(
            ChatCommandParser::parse("workload ensia", &known),
            ChatCommand::Workload {
                repo: Some("StarLabsSI/ensia".to_string())
            }
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
            ChatCommand::Prs {
                repo: Some("StarLabsSI/ensia".to_string()),
                count: CHAT_LIST_DEFAULT,
                page: 1,
            }
        );
        // Numbers mean count then page, and may come before or after the repo.
        assert_eq!(
            ChatCommandParser::parse("issues 25 2", &known),
            ChatCommand::Issues {
                repo: None,
                count: 25,
                page: 2,
            }
        );
        assert_eq!(
            ChatCommandParser::parse("/prs ensia 30 3", &known),
            ChatCommand::Prs {
                repo: Some("StarLabsSI/ensia".to_string()),
                count: CHAT_LIST_MAX,
                page: 3,
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

    #[test]
    fn chat_list_message_reports_real_total_and_next_page() {
        let lines: Vec<String> = (1..=3)
            .map(|n| format!("#{n} Some title — @alice"))
            .collect();
        let msg = assemble_list_message(
            "🐞",
            "issues",
            "StarLabsSI/ensia",
            "issues",
            1,
            10,
            0,
            &lines,
            Some(240),
        );
        assert!(
            msg.starts_with("🐞 **Open issues in `StarLabsSI/ensia`** (240 total, showing 1–3):")
        );
        assert!(msg.contains("#1 Some title"));
        assert!(msg.ends_with("Next page: `issues StarLabsSI/ensia 10 2`"));
    }

    #[test]
    fn chat_list_message_stops_advertising_pages_that_do_not_exist() {
        let lines = vec!["#1 Only issue — @bob".to_string()];
        let msg = assemble_list_message("🐞", "issues", "o/r", "issues", 1, 10, 0, &lines, Some(1));
        assert!(msg.contains("(1 total, showing 1–1)"));
        assert!(!msg.contains("Next page"));
    }

    #[test]
    fn chat_list_message_is_trimmed_to_the_platform_budget() {
        // 60 long rows would blow past Discord's 2000-character cap.
        let lines: Vec<String> = (1..=60)
            .map(|n| format!("#{n} {}", "x".repeat(80)))
            .collect();
        let msg = assemble_list_message("🔀", "PRs", "o/r", "prs", 1, 60, 0, &lines, Some(500));
        assert!(msg.contains("truncated to fit the message limit"));
        assert!(!msg.contains("Next page"));
        // Budget plus the truncation notice, which is itself bounded.
        assert!(msg.chars().count() <= CHAT_LIST_BUDGET + 160);
    }

    #[test]
    fn chat_list_message_handles_empty_and_out_of_range() {
        let empty: Vec<String> = Vec::new();
        assert_eq!(
            assemble_list_message("🐞", "issues", "o/r", "issues", 1, 10, 0, &empty, Some(0)),
            "🐞 No open issues in `o/r`."
        );
        assert_eq!(
            assemble_list_message("🐞", "issues", "o/r", "issues", 3, 10, 20, &empty, Some(25)),
            "🐞 No open issues on page 3 of `o/r` (10 per page)."
        );
    }
}

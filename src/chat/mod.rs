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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub extra_ids: Vec<String>,
}

impl InboundSender {
    pub fn new(id: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            username: username.into(),
            display_name: None,
            extra_ids: Vec::new(),
        }
    }

    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }

    pub fn with_extra_id(mut self, extra_id: impl Into<String>) -> Self {
        let eid = extra_id.into();
        if !eid.trim().is_empty() && !self.extra_ids.contains(&eid) {
            self.extra_ids.push(eid);
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundChatMessage {
    pub platform: ChatPlatform,
    pub sender: InboundSender,
    pub raw_content: String,
    pub stream_or_channel: Option<String>,
    pub message_id: Option<String>,
    #[serde(default)]
    pub extra_channel_ids: Vec<String>,
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

impl ChatCommand {
    pub fn name(&self) -> &'static str {
        match self {
            ChatCommand::Help => "help",
            ChatCommand::Status => "status",
            ChatCommand::ChatId => "chatid",
            ChatCommand::Repos => "repos",
            ChatCommand::Workload { .. } => "workload",
            ChatCommand::Users { .. } => "users",
            ChatCommand::Metrics { .. } => "metrics",
            ChatCommand::Issues { .. } => "issues",
            ChatCommand::Prs { .. } => "prs",
            ChatCommand::Stats { .. } => "stats",
            ChatCommand::Config => "config",
            ChatCommand::Stale { .. } => "stale",
            ChatCommand::Comment { .. } => "comment",
            ChatCommand::IssueAction { command, .. } => command.name(),
            ChatCommand::Unknown(_) => "unknown",
        }
    }
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
        // 0. Channel / Group Whitelist Authorization Gate
        if !self.is_channel_allowed(message) {
            let ch_display = message
                .stream_or_channel
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("unknown");

            return match message.platform {
                ChatPlatform::Lark => format!(
                    "⛔ This chat group is not authorized to interact with Sugie Bot.\n\n\
                     📍 **Chat ID**: `{ch_display}`\n\n\
                     To authorize this group, an administrator must add this Chat ID to `lark_allowed_chats`."
                ),
                ChatPlatform::Discord => format!(
                    "⛔ This channel is not authorized to interact with Sugie Bot.\n\n\
                     📍 **Channel ID**: `{ch_display}`\n\n\
                     To authorize this channel, an administrator must add this Channel ID to `discord_allowed_channels`."
                ),
                ChatPlatform::Zulip => format!(
                    "⛔ This stream/channel is not authorized to interact with Sugie Bot.\n\n\
                     📍 **Stream / Channel**: `{ch_display}`\n\n\
                     To authorize this stream, an administrator must add it to `zulip_allowed_channels`."
                ),
                ChatPlatform::Api => "⛔ Unauthorized API channel.".to_string(),
            };
        }

        let known_repos = self.get_known_repositories();
        let cmd = ChatCommandParser::parse(&message.raw_content, &known_repos);
        let cmd_name = cmd.name();

        let is_super = self.is_super_user(message.platform, &message.sender);

        // 1. Check if this specific command is restricted in restricted_commands
        let is_restricted = self
            .config
            .default_rules
            .commands
            .restricted_commands
            .iter()
            .any(|r| {
                let clean = r.trim().trim_start_matches('/');
                clean.eq_ignore_ascii_case(cmd_name)
                    || (cmd_name == "chatid"
                        && (clean.eq_ignore_ascii_case("chat_id") || clean.eq_ignore_ascii_case("chat-id")))
            });

        if is_restricted && !is_super {
            return format!(
                "⛔ Sorry @{}, the command `/{}` is restricted to authorized super users.\n\nYour User ID is `{}`.",
                message.sender.username, cmd_name, message.sender.id
            );
        }

        // 2. Check if admin_only mode is enabled
        // In admin_only mode, only super users can execute commands.
        // /help and /chatid are exempt so users can discover their ID, UNLESS
        // they are explicitly placed in restricted_commands.
        if self.config.default_rules.commands.admin_only && !is_super {
            let is_exempt = matches!(cmd, ChatCommand::ChatId | ChatCommand::Help) && !is_restricted;
            if !is_exempt {
                return format!(
                    "⛔ Access Denied: Sugie is configured in admin-only mode (`admin_only = true`).\nOnly authorized administrators can run commands.\n\nYour User ID is `{}` (@{}). Type `/chatid` to see how to request access.",
                    message.sender.id, message.sender.username
                );
            }
        }

        match cmd {
            ChatCommand::Help => self.render_help(),
            ChatCommand::Status => self.render_status(Some(message)).await,
            ChatCommand::ChatId => self.render_chat_id(message),
            ChatCommand::Repos => self.render_repos(is_super),
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
            ChatCommand::Stale { repo } => self.run_stale(message, repo.as_deref()).await,
            ChatCommand::Comment { repo, number, body } => {
                self.post_comment(message, &repo, number, &body).await
            }
            ChatCommand::IssueAction {
                repo,
                number,
                command,
            } => {
                self.execute_issue_action(message, &repo, number, command).await
            }
            ChatCommand::Unknown(text) => {
                format!(
                    "❓ Unknown command `{}`.\nType `help` or `/help` to see available bot commands.",
                    text
                )
            }
        }
    }

    pub fn is_repo_allowed(&self, repo: &str) -> bool {
        if let Some(ref allowed) = self.config.notifications.allowed_repositories {
            if !allowed.is_empty() {
                return allowed.iter().any(|r| r.eq_ignore_ascii_case(repo));
            }
        }
        true
    }

    fn get_known_repositories(&self) -> Vec<String> {
        let mut repos = Vec::new();
        if let Some(ref allowed) = self.config.notifications.allowed_repositories {
            if !allowed.is_empty() {
                return allowed.clone();
            }
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

        let is_admin = self.is_super_user(message.platform, &message.sender);
        let admin_badge = if is_admin {
            "🟢 Authorized Super User / Admin"
        } else {
            "🔴 Standard User (Not authorized for restricted commands)"
        };

        let mut out = format!(
            "### 📍 Chat & User Identification\n\n\
            - **Platform**: {}\n\
            - **Current Chat ID**: `{}`\n\
            - **Your User ID**: `{}`\n\
            - **Your Username**: `@{}`\n\
            - **Admin Permission**: {}\n",
            platform_name, channel, message.sender.id, message.sender.username, admin_badge
        );

        if !message.sender.extra_ids.is_empty() {
            out.push_str(&format!(
                "- **Alternate IDs**: `{}`\n",
                message.sender.extra_ids.join("`, `")
            ));
        }

        if !is_admin {
            let config_field = match message.platform {
                ChatPlatform::Lark => "lark_super_users",
                ChatPlatform::Discord => "discord_super_users",
                ChatPlatform::Zulip => "zulip_super_users",
                ChatPlatform::Api => "super_users",
            };
            out.push_str(&format!(
                "\n💡 **To grant this user admin permissions**, add to `sugie.toml`:\n```toml\n[default_rules.commands]\n{} = [\"{}\"]\n```\n",
                config_field, message.sender.id
            ));
        }

        let toml_snippet = match message.platform {
            ChatPlatform::Lark => format!(
                "\n💡 **To register this channel in `sugie.toml`**:\n```toml\n[[notifications.lark.channels]]\nname = \"my-channel\"\nchat_id = \"{}\"\n```",
                channel
            ),
            ChatPlatform::Discord => format!(
                "\n💡 **To register this channel in `sugie.toml`**:\n```toml\n[[notifications.discord.bot_channels]]\nname = \"my-channel\"\nchannel_id = \"{}\"\n```",
                channel
            ),
            ChatPlatform::Zulip => format!(
                "\n💡 **To register this channel in `sugie.toml`**:\n```toml\n[[notifications.zulip.channels]]\nname = \"my-channel\"\nchannel = \"{}\"\ntopic = \"announcements\"\n```",
                channel
            ),
            ChatPlatform::Api => String::new(),
        };

        out.push_str(&toml_snippet);
        out
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
            let is_admin = self.is_super_user(m.platform, &m.sender);
            format!(
                "- **Current User**: `{}` (@{}) — {}\n{}",
                m.sender.id,
                m.sender.username,
                if is_admin { "🟢 Super User" } else { "⚪ Standard User" },
                if let Some(ref ch) = m.stream_or_channel {
                    format!("- **Current Chat ID**: `{}`\n", ch)
                } else {
                    String::new()
                }
            )
        } else {
            String::new()
        };

        let cmd_cfg = &self.config.default_rules.commands;
        let mut super_summary = Vec::new();
        if !cmd_cfg.super_users.is_empty() {
            super_summary.push(format!("GitHub/Global: {}", cmd_cfg.super_users.join(", ")));
        }
        if !cmd_cfg.lark_super_users.is_empty() {
            super_summary.push(format!("Lark: {}", cmd_cfg.lark_super_users.join(", ")));
        }
        if !cmd_cfg.discord_super_users.is_empty() {
            super_summary.push(format!("Discord: {}", cmd_cfg.discord_super_users.join(", ")));
        }
        if !cmd_cfg.zulip_super_users.is_empty() {
            super_summary.push(format!("Zulip: {}", cmd_cfg.zulip_super_users.join(", ")));
        }
        let super_str = if super_summary.is_empty() {
            "(None)".to_string()
        } else {
            super_summary.join(" | ")
        };

        let mode_str = if cmd_cfg.admin_only {
            "🔒 **Admin Only** (all bot commands require super user)".to_string()
        } else if cmd_cfg.restricted_commands.is_empty() {
            "Open (no command restrictions)".to_string()
        } else {
            format!("/{}", cmd_cfg.restricted_commands.join(", /"))
        };

        let mut ch_whitelist_summary = Vec::new();
        if !cmd_cfg.lark_allowed_chats.is_empty() {
            ch_whitelist_summary.push(format!("Lark: {} groups", cmd_cfg.lark_allowed_chats.len()));
        }
        if !cmd_cfg.discord_allowed_channels.is_empty() {
            ch_whitelist_summary.push(format!("Discord: {} channels", cmd_cfg.discord_allowed_channels.len()));
        }
        if !cmd_cfg.zulip_allowed_channels.is_empty() {
            ch_whitelist_summary.push(format!("Zulip: {} streams", cmd_cfg.zulip_allowed_channels.len()));
        }
        let ch_whitelist_str = if ch_whitelist_summary.is_empty() {
            "Open (all groups/channels permitted)".to_string()
        } else {
            ch_whitelist_summary.join(" | ")
        };

        format!(
            "### 🤖 Sugie Bot Status\n\n\
            - **Status**: 🟢 Running\n\
            - **Version**: `v{}`\n\
            - **Monitored Repositories**: {}\n\
            - **Notification Channels**: {}\n\
            {} - **Super Users**: {}\n\
            - **Restricted Commands**: {}\n\
            - **Channel Whitelist**: {}\n",
            env!("CARGO_PKG_VERSION"),
            repos.len(),
            if ch_list.is_empty() {
                "None configured".to_string()
            } else {
                ch_list.join(", ")
            },
            current_chat_info,
            super_str,
            mode_str,
            ch_whitelist_str,
        )
    }

    fn render_repos(&self, is_super: bool) -> String {
        let mut repos = self
            .triage
            .storage()
            .list_repositories()
            .unwrap_or_default();

        if let Some(ref allowed) = self.config.notifications.allowed_repositories {
            if !allowed.is_empty() {
                repos.retain(|(_, name, _)| allowed.iter().any(|a| a.eq_ignore_ascii_case(name)));
            }
        }

        if repos.is_empty() {
            return "📦 No repositories currently onboarded in local storage.".to_string();
        }

        let mut lines = vec![format!("📦 **Onboarded Repositories ({})**:", repos.len())];
        for (id, name, inst_id) in repos {
            if is_super {
                lines.push(format!(
                    "- `{}` (ID: {}, Installation: {})",
                    name, id, inst_id
                ));
            } else {
                lines.push(format!("- `{}`", name));
            }
        }
        lines.join("\n")
    }

    fn render_workload(&self, target_repo: Option<&str>) -> String {
        if let Some(r) = target_repo {
            if !self.is_repo_allowed(r) {
                return format!("⛔ Access to repository `{r}` is not permitted by configuration.");
            }
        }

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

    pub fn is_super_user(&self, platform: ChatPlatform, sender: &InboundSender) -> bool {
        Self::check_super_user(&self.config.default_rules.commands, platform, sender)
    }

    pub fn check_super_user(
        commands_cfg: &crate::config::CommandsConfig,
        platform: ChatPlatform,
        sender: &InboundSender,
    ) -> bool {
        let platform_super_users: &[String] = match platform {
            ChatPlatform::Lark => &commands_cfg.lark_super_users,
            ChatPlatform::Discord => &commands_cfg.discord_super_users,
            ChatPlatform::Zulip => &commands_cfg.zulip_super_users,
            ChatPlatform::Api => &[],
        };

        let matches_user = |su: &str| {
            let clean = su.trim().trim_start_matches('@');
            if clean.is_empty() {
                return false;
            }
            let (prefix_opt, target) = match clean.split_once(':') {
                Some((p, rest)) => (Some(p), rest.trim()),
                None => (None, clean),
            };

            let platform_matches = match (prefix_opt, platform) {
                (Some(p), ChatPlatform::Lark) => {
                    p.eq_ignore_ascii_case("lark") || p.eq_ignore_ascii_case("feishu")
                }
                (Some(p), ChatPlatform::Discord) => p.eq_ignore_ascii_case("discord"),
                (Some(p), ChatPlatform::Zulip) => p.eq_ignore_ascii_case("zulip"),
                (Some(p), ChatPlatform::Api) => p.eq_ignore_ascii_case("api"),
                // Unprefixed entries in super_users apply to GitHub and interactive platforms,
                // but NOT to generic API requests (which require explicit api: prefix).
                (None, ChatPlatform::Api) => false,
                (None, _) => true,
            };

            if !platform_matches {
                return false;
            }

            // Strictly check immutable IDs (id, unique username, extra_ids).
            // Do NOT check display_name / nick, which can be modified by untrusted users.
            target.eq_ignore_ascii_case(&sender.id)
                || target.eq_ignore_ascii_case(&sender.username)
                || sender
                    .extra_ids
                    .iter()
                    .any(|eid| target.eq_ignore_ascii_case(eid))
        };

        if platform_super_users.iter().any(|su| matches_user(su)) {
            return true;
        }

        commands_cfg.super_users.iter().any(|su| matches_user(su))
    }

    pub fn is_channel_allowed(&self, message: &InboundChatMessage) -> bool {
        Self::check_channel_allowed(
            &self.config.default_rules.commands,
            message.platform,
            message.stream_or_channel.as_deref(),
            &message.extra_channel_ids,
        )
    }

    pub fn check_channel_allowed(
        commands_cfg: &crate::config::CommandsConfig,
        platform: ChatPlatform,
        channel: Option<&str>,
        extra_channel_ids: &[String],
    ) -> bool {
        let allowed_list: &[String] = match platform {
            ChatPlatform::Lark => &commands_cfg.lark_allowed_chats,
            ChatPlatform::Discord => &commands_cfg.discord_allowed_channels,
            ChatPlatform::Zulip => &commands_cfg.zulip_allowed_channels,
            ChatPlatform::Api => &[],
        };

        // If no whitelist is configured for this platform, all channels/groups are allowed
        if allowed_list.is_empty() {
            return true;
        }

        let ch = match channel {
            Some(c) if !c.trim().is_empty() => c.trim(),
            _ => return false,
        };

        allowed_list.iter().any(|allowed| {
            let clean = allowed.trim();
            clean.eq_ignore_ascii_case(ch)
                || extra_channel_ids
                    .iter()
                    .any(|extra| clean.eq_ignore_ascii_case(extra.trim()))
        })
    }

    async fn client_for(&self, repo: &str) -> Result<GitHubClient> {
        match self.triage.get_client_for_repo(repo).await {
            Ok(c) => Ok(c),
            Err(_) => self.find_client_dynamically(repo).await,
        }
    }

    fn resolve_target_repo(&self, target: Option<&str>, cmd_hint: &str) -> Result<String, String> {
        let repo = target
            .map(|s| s.to_string())
            .or_else(|| self.get_known_repositories().first().cloned())
            .ok_or_else(|| format!("⚠️ Please specify repository: `{cmd_hint} <owner/repo>`."))?;

        if !self.is_repo_allowed(&repo) {
            return Err(format!("⛔ Access to repository `{repo}` is not permitted by configuration."));
        }
        Ok(repo)
    }

    async fn render_users(&self, target_repo: Option<&str>) -> String {
        let repo = match self.resolve_target_repo(target_repo, "users") {
            Ok(r) => r,
            Err(e) => return e,
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
        let repo = match self.resolve_target_repo(target_repo, "stats") {
            Ok(r) => r,
            Err(e) => return e,
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
        let cmd_cfg = &self.config.default_rules.commands;
        format!(
            "⚙️ **Active Configuration**\n\n\
            - Listen: `{}:{}`\n\
            - Notification channels: {}\n\
            - Repository filter: {}\n\
            - Admin-only mode: {}\n\
            - Super users (GitHub): {}\n\
            - Platform admins: Lark ({}) · Discord ({}) · Zulip ({})\n\
            - Whitelisted channels: Lark ({}) · Discord ({}) · Zulip ({})\n\
            - Restricted commands: {}\n\
            - Stale sweep: {} · Auto reviewers: {} · Auto labels: {}",
            s.host,
            s.port,
            if s.channels.is_empty() {
                "None".to_string()
            } else {
                s.channels.join(", ")
            },
            if s.allowed_repositories.is_empty() {
                "all onboarded repositories".to_string()
            } else {
                s.allowed_repositories.join(", ")
            },
            if cmd_cfg.admin_only { "🔒 ON (all commands require admin)" } else { "off" },
            if s.super_users.is_empty() {
                "(none)".to_string()
            } else {
                s.super_users.join(", ")
            },
            if cmd_cfg.lark_super_users.is_empty() { "none".to_string() } else { cmd_cfg.lark_super_users.join(", ") },
            if cmd_cfg.discord_super_users.is_empty() { "none".to_string() } else { cmd_cfg.discord_super_users.join(", ") },
            if cmd_cfg.zulip_super_users.is_empty() { "none".to_string() } else { cmd_cfg.zulip_super_users.join(", ") },
            if cmd_cfg.lark_allowed_chats.is_empty() { "all".to_string() } else { format!("{} chats", cmd_cfg.lark_allowed_chats.len()) },
            if cmd_cfg.discord_allowed_channels.is_empty() { "all".to_string() } else { format!("{} channels", cmd_cfg.discord_allowed_channels.len()) },
            if cmd_cfg.zulip_allowed_channels.is_empty() { "all".to_string() } else { format!("{} streams", cmd_cfg.zulip_allowed_channels.len()) },
            if s.restricted_commands.is_empty() {
                "(none)".to_string()
            } else {
                format!("/{}", s.restricted_commands.join(", /"))
            },
            if s.stale_enabled { "on" } else { "off" },
            if s.reviewers_enabled { "on" } else { "off" },
            if s.labels_enabled { "on" } else { "off" },
        )
    }

    async fn run_stale(&self, message: &InboundChatMessage, target_repo: Option<&str>) -> String {
        if !self.is_super_user(message.platform, &message.sender) {
            return format!(
                "⛔ Sorry @{}, `/stale` is restricted to authorized super users.\n\nYour User ID is `{}`.",
                message.sender.username, message.sender.id
            );
        }
        let rules = self.config.default_rules.stale.clone();
        if !rules.enabled {
            return "⚠️ Stale lifecycle rules are disabled (`default_rules.stale.enabled = false`).".to_string();
        }
        let repo = match self.resolve_target_repo(target_repo, "stale") {
            Ok(r) => r,
            Err(e) => return e,
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
        message: &InboundChatMessage,
        repo: &str,
        number: u64,
        body: &str,
    ) -> String {
        let sender = &message.sender;
        if !self.is_super_user(message.platform, sender) {
            return format!(
                "⛔ Sorry @{}, `/comment` is restricted to authorized super users.\n\nYour User ID is `{}`.",
                sender.username, sender.id
            );
        }
        if !self.is_repo_allowed(repo) {
            return format!("⛔ Access to repository `{repo}` is not permitted by configuration.");
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
        let repo = match self.resolve_target_repo(target_repo, cmd) {
            Ok(r) => r,
            Err(e) => return e,
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
        let repo = match self.resolve_target_repo(target_repo, "metrics") {
            Ok(r) => r,
            Err(e) => return e,
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
        message: &InboundChatMessage,
        repo: &str,
        number: u64,
        command: SlashCommand,
    ) -> String {
        let sender = &message.sender;
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
            let is_super = self.is_super_user(message.platform, sender);

            if !is_super {
                return format!(
                    "⛔ Sorry @{}, the command `/{}` is restricted to authorized super users.\n\nYour User ID is `{}`.",
                    sender.username, cmd_name, sender.id
                );
            }
        }

        if !self.is_repo_allowed(repo) {
            return format!("⛔ Access to repository `{repo}` is not permitted by configuration.");
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

    #[test]
    fn test_is_super_user_platform_matching() {
        let mut commands_cfg = crate::config::CommandsConfig::default();
        commands_cfg.super_users = vec!["octocat".to_string(), "discord:global_mod".to_string()];
        commands_cfg.lark_super_users = vec![
            "ou_lark_admin".to_string(),
            "emp_9999".to_string(),
        ];
        commands_cfg.discord_super_users = vec![
            "123456789012345678".to_string(),
            "discord_mod".to_string(),
        ];
        commands_cfg.zulip_super_users = vec![
            "admin@zulip.org".to_string(),
        ];

        // Lark sender tests
        let lark_sender_ok = InboundSender {
            id: "ou_lark_admin".to_string(),
            username: "John Doe".to_string(),
            display_name: None,
            extra_ids: vec!["emp_1234".to_string()],
        };
        assert!(ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Lark, &lark_sender_ok));

        let lark_sender_extra_match = InboundSender {
            id: "ou_other".to_string(),
            username: "Jane Doe".to_string(),
            display_name: None,
            extra_ids: vec!["emp_9999".to_string()],
        };
        assert!(ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Lark, &lark_sender_extra_match));

        let lark_sender_fail = InboundSender {
            id: "ou_regular".to_string(),
            username: "Regular User".to_string(),
            display_name: None,
            extra_ids: vec![],
        };
        assert!(!ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Lark, &lark_sender_fail));

        // Discord sender tests
        let discord_sender_id_match = InboundSender {
            id: "123456789012345678".to_string(),
            username: "coolguy".to_string(),
            display_name: None,
            extra_ids: vec![],
        };
        assert!(ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Discord, &discord_sender_id_match));

        let discord_sender_user_match = InboundSender {
            id: "9999999".to_string(),
            username: "discord_mod".to_string(),
            display_name: None,
            extra_ids: vec![],
        };
        assert!(ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Discord, &discord_sender_user_match));

        // Platform-prefixed in general super_users: "discord:global_mod"
        let discord_prefixed_match = InboundSender {
            id: "888888".to_string(),
            username: "global_mod".to_string(),
            display_name: None,
            extra_ids: vec![],
        };
        assert!(ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Discord, &discord_prefixed_match));
        // Same username on Lark should NOT match "discord:global_mod"
        assert!(!ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Lark, &discord_prefixed_match));

        // Fallback super user: "octocat"
        let github_fallback = InboundSender {
            id: "octocat".to_string(),
            username: "octocat".to_string(),
            display_name: None,
            extra_ids: vec![],
        };
        assert!(ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Discord, &github_fallback));

        // SECURITY: Attempting to spoof super user via mutable display_name / nickname MUST fail
        let display_name_spoof_attempt = InboundSender {
            id: "999999999".to_string(),
            username: "attacker".to_string(),
            display_name: Some("discord_mod".to_string()),
            extra_ids: vec![],
        };
        assert!(!ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Discord, &display_name_spoof_attempt));

        // SECURITY: Generic API caller cannot spoof GitHub super_users ("octocat") without explicit api: prefix
        let api_caller_spoof = InboundSender {
            id: "api:octocat".to_string(),
            username: "octocat".to_string(),
            display_name: None,
            extra_ids: vec!["api:octocat".to_string()],
        };
        assert!(!ChatCommandExecutor::check_super_user(&commands_cfg, ChatPlatform::Api, &api_caller_spoof));

        // When explicitly configured with "api:service_bot", it matches
        let mut api_super_cfg = commands_cfg.clone();
        api_super_cfg.super_users.push("api:service_bot".to_string());
        let api_caller_ok = InboundSender {
            id: "api:service_bot".to_string(),
            username: "service_bot".to_string(),
            display_name: None,
            extra_ids: vec!["api:service_bot".to_string()],
        };
        assert!(ChatCommandExecutor::check_super_user(&api_super_cfg, ChatPlatform::Api, &api_caller_ok));
    }

    #[tokio::test]
    async fn test_restricted_commands_including_status_and_chatid() {
        let mut config = SugieConfig::default();
        config.default_rules.commands.lark_super_users = vec!["ou_admin_ok".to_string()];
        config.default_rules.commands.restricted_commands = vec!["status".to_string(), "chatid".to_string()];

        let storage = crate::storage::Storage::new_in_memory().unwrap();
        let auth = std::sync::Arc::new(crate::github::auth::GitHubAuthManager::new(1, "key".into()));
        let notifier = crate::notifications::NotificationDispatcher::new(&config.notifications);
        let triage = crate::triage::TriageService::new(std::sync::Arc::new(config.clone()), storage, notifier, auth);
        let executor = ChatCommandExecutor::new(&config, &triage);

        // Standard user running /status
        let standard_user_msg = InboundChatMessage {
            platform: ChatPlatform::Lark,
            sender: InboundSender {
                id: "ou_standard_user".to_string(),
                username: "Support".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "@apichtopbot status".to_string(),
            stream_or_channel: Some("oc_123".to_string()),
            message_id: Some("om_123".to_string()),
            extra_channel_ids: vec![],
        };

        let reply = executor.execute(&standard_user_msg).await;
        assert!(reply.contains("⛔ Sorry @Support, the command `/status` is restricted to authorized super users."));

        // Standard user running /chatid
        let chatid_msg = InboundChatMessage {
            platform: ChatPlatform::Lark,
            sender: InboundSender {
                id: "ou_standard_user".to_string(),
                username: "Support".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "@apichtopbot chatid".to_string(),
            stream_or_channel: Some("oc_123".to_string()),
            message_id: Some("om_123".to_string()),
            extra_channel_ids: vec![],
        };
        let reply_chatid = executor.execute(&chatid_msg).await;
        assert!(reply_chatid.contains("⛔ Sorry @Support, the command `/chatid` is restricted to authorized super users."));

        // Authorized super user running /status
        let super_user_msg = InboundChatMessage {
            platform: ChatPlatform::Lark,
            sender: InboundSender {
                id: "ou_admin_ok".to_string(),
                username: "Support".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "@apichtopbot status".to_string(),
            stream_or_channel: Some("oc_123".to_string()),
            message_id: Some("om_123".to_string()),
            extra_channel_ids: vec![],
        };
        let super_reply = executor.execute(&super_user_msg).await;
        assert!(super_reply.contains("Sugie Bot Status"));
    }

    #[tokio::test]
    async fn test_channel_whitelist_authorization() {
        let mut config = SugieConfig::default();
        config.default_rules.commands.lark_allowed_chats = vec!["oc_allowed_group".to_string()];
        config.default_rules.commands.discord_allowed_channels = vec!["111222333".to_string(), "guild_999".to_string()];
        config.default_rules.commands.zulip_allowed_channels = vec!["authorized_stream".to_string()];

        let storage = crate::storage::Storage::new_in_memory().unwrap();
        let auth = std::sync::Arc::new(crate::github::auth::GitHubAuthManager::new(1, "key".into()));
        let notifier = crate::notifications::NotificationDispatcher::new(&config.notifications);
        let triage = crate::triage::TriageService::new(std::sync::Arc::new(config.clone()), storage, notifier, auth);
        let executor = ChatCommandExecutor::new(&config, &triage);

        // 1. Lark in unauthorized group
        let lark_blocked = InboundChatMessage {
            platform: ChatPlatform::Lark,
            sender: InboundSender {
                id: "ou_user1".to_string(),
                username: "User1".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "@bot help".to_string(),
            stream_or_channel: Some("oc_unauthorized_group".to_string()),
            message_id: None,
            extra_channel_ids: vec![],
        };
        let reply1 = executor.execute(&lark_blocked).await;
        assert!(reply1.contains("⛔ This chat group is not authorized to interact with Sugie Bot."));
        assert!(reply1.contains("oc_unauthorized_group"));
        assert!(reply1.contains("lark_allowed_chats"));

        // 2. Lark in authorized group
        let lark_allowed = InboundChatMessage {
            platform: ChatPlatform::Lark,
            sender: InboundSender {
                id: "ou_user1".to_string(),
                username: "User1".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "@bot help".to_string(),
            stream_or_channel: Some("oc_allowed_group".to_string()),
            message_id: None,
            extra_channel_ids: vec![],
        };
        let reply2 = executor.execute(&lark_allowed).await;
        assert!(reply2.contains("Sugie Bot Commands Reference"));

        // 3. Discord in unauthorized channel and unauthorized guild
        let discord_blocked = InboundChatMessage {
            platform: ChatPlatform::Discord,
            sender: InboundSender {
                id: "123".to_string(),
                username: "duser".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "/help".to_string(),
            stream_or_channel: Some("888888".to_string()),
            message_id: None,
            extra_channel_ids: vec!["guild_other".to_string()],
        };
        let reply3 = executor.execute(&discord_blocked).await;
        assert!(reply3.contains("⛔ This channel is not authorized to interact with Sugie Bot."));
        assert!(reply3.contains("888888"));

        // 4. Discord matched via guild_id in extra_channel_ids
        let discord_guild_allowed = InboundChatMessage {
            platform: ChatPlatform::Discord,
            sender: InboundSender {
                id: "123".to_string(),
                username: "duser".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "/help".to_string(),
            stream_or_channel: Some("888888".to_string()),
            message_id: None,
            extra_channel_ids: vec!["guild_999".to_string()],
        };
        let reply4 = executor.execute(&discord_guild_allowed).await;
        assert!(reply4.contains("Sugie Bot Commands Reference"));

        // 5. Zulip in authorized stream
        let zulip_allowed = InboundChatMessage {
            platform: ChatPlatform::Zulip,
            sender: InboundSender {
                id: "zuser@domain.com".to_string(),
                username: "zuser".to_string(),
                display_name: None,
                extra_ids: vec![],
            },
            raw_content: "help".to_string(),
            stream_or_channel: Some("authorized_stream".to_string()),
            message_id: None,
            extra_channel_ids: vec![],
        };
        let reply5 = executor.execute(&zulip_allowed).await;
        assert!(reply5.contains("Sugie Bot Commands Reference"));
    }
}

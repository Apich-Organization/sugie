use crate::config::CommandsConfig;
use crate::github::client::GitHubClient;
use crate::github::models::{Issue, User};
use crate::notifications::{
    DiscordEmbedField, NotificationDispatcher, NotificationLevel, SugieEventType,
};
use crate::triage::projects::ProjectsService;
use crate::triage::reviewers::ReviewerLoadBalancer;
use anyhow::Result;
use serde::Deserialize;
use tracing::info;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SlashCommand {
    Assign(Vec<String>),
    Unassign(Vec<String>),
    Label {
        add: Vec<String>,
        remove: Vec<String>,
    },
    Unlabel(Vec<String>),
    Close(Option<String>),
    Reopen,
    Retest,
    Project {
        target: String,
        status: Option<String>,
    },
    Move {
        target_repo: String,
    },
    Duplicate(u64),
    Reviewer {
        add: Vec<String>,
        remove: Vec<String>,
    },
    Help,
}

impl SlashCommand {
    pub fn name(&self) -> &'static str {
        match self {
            SlashCommand::Assign(_) => "assign",
            SlashCommand::Unassign(_) => "unassign",
            SlashCommand::Label { .. } => "label",
            SlashCommand::Unlabel(_) => "unlabel",
            SlashCommand::Close(_) => "close",
            SlashCommand::Reopen => "reopen",
            SlashCommand::Retest => "retest",
            SlashCommand::Project { .. } => "project",
            SlashCommand::Move { .. } => "move",
            SlashCommand::Duplicate(_) => "duplicate",
            SlashCommand::Reviewer { .. } => "reviewer",
            SlashCommand::Help => "help",
        }
    }
}

pub struct CommandParser;

impl CommandParser {
    pub fn parse_comment(comment_body: &str, config: &CommandsConfig) -> Vec<SlashCommand> {
        let mut commands = Vec::new();
        let prefix = &config.prefix;

        for line in comment_body.lines() {
            let mut trimmed = line.trim();
            let mut is_bot_mention = false;

            if trimmed.starts_with('@') {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if !parts.is_empty() {
                    let mention_token = parts[0].trim_matches(|c: char| c == ':' || c == ',' || c == ';');
                    let lower_mention = mention_token.to_lowercase();
                    // Match bot mentions like @starlabssisugie, @sugie, @...-bot, @...[bot]
                    if lower_mention.contains("sugie")
                        || lower_mention.contains("starlabs")
                        || lower_mention.contains("bot")
                        || parts.len() == 1
                    {
                        is_bot_mention = true;
                        if parts.len() == 1 {
                            // User simply mentioned the bot on a line: e.g. "@starlabssisugie" -> trigger Help
                            commands.push(SlashCommand::Help);
                            continue;
                        }
                        trimmed = trimmed[parts[0].len()..].trim_start();
                    }
                }
            }

            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            let (raw_cmd, args) = if trimmed.starts_with(prefix) {
                let cmd = parts[0].strip_prefix(prefix).unwrap_or("").to_lowercase();
                (cmd, &parts[1..])
            } else if is_bot_mention {
                // If preceded by @bot, allow command without slash prefix (e.g. "@bot label +bug" or "@bot /label +bug")
                let cmd = parts[0].strip_prefix(prefix).unwrap_or(parts[0]).to_lowercase();
                (cmd, &parts[1..])
            } else {
                continue;
            };

            let effective_cmd = config
                .aliases
                .get(&raw_cmd)
                .map(|s| s.as_str())
                .unwrap_or(&raw_cmd);

            match effective_cmd {
                "assign" => {
                    let users: Vec<String> = args
                        .iter()
                        .map(|s| s.trim_start_matches('@').to_string())
                        .collect();
                    commands.push(SlashCommand::Assign(users));
                }
                "unassign" => {
                    let users: Vec<String> = args
                        .iter()
                        .map(|s| s.trim_start_matches('@').to_string())
                        .collect();
                    commands.push(SlashCommand::Unassign(users));
                }
                "label" => {
                    let mut add = Vec::new();
                    let mut remove = Vec::new();
                    for arg in args {
                        if let Some(lbl) = arg.strip_prefix('+') {
                            add.push(lbl.to_string());
                        } else if let Some(lbl) = arg.strip_prefix('-') {
                            remove.push(lbl.to_string());
                        } else {
                            add.push(arg.to_string());
                        }
                    }
                    if !add.is_empty() || !remove.is_empty() {
                        commands.push(SlashCommand::Label { add, remove });
                    }
                }
                "unlabel" => {
                    let labels: Vec<String> = args.iter().map(|s| s.to_string()).collect();
                    if !labels.is_empty() {
                        commands.push(SlashCommand::Unlabel(labels));
                    }
                }
                "close" => {
                    let reason = if !args.is_empty() {
                        Some(args.join(" "))
                    } else {
                        None
                    };
                    commands.push(SlashCommand::Close(reason));
                }
                "reopen" => {
                    commands.push(SlashCommand::Reopen);
                }
                "retest" => {
                    commands.push(SlashCommand::Retest);
                }
                "project" => {
                    if !args.is_empty() {
                        let target = args[0].to_string();
                        let status = if args.len() > 1 {
                            Some(args[1..].join(" "))
                        } else {
                            None
                        };
                        commands.push(SlashCommand::Project { target, status });
                    }
                }
                "move" => {
                    if !args.is_empty() {
                        commands.push(SlashCommand::Move {
                            target_repo: args[0].to_string(),
                        });
                    }
                }
                "duplicate" => {
                    if !args.is_empty() {
                        let num_str = args[0].trim_start_matches('#');
                        if let Ok(num) = num_str.parse::<u64>() {
                            commands.push(SlashCommand::Duplicate(num));
                        }
                    }
                }
                "reviewer" | "reviewers" | "review" => {
                    let mut add = Vec::new();
                    let mut remove = Vec::new();
                    for arg in args {
                        let clean = arg.trim_start_matches('@');
                        if let Some(user) = clean.strip_prefix('+') {
                            add.push(user.trim_start_matches('@').to_string());
                        } else if let Some(user) = clean.strip_prefix('-') {
                            remove.push(user.trim_start_matches('@').to_string());
                        } else if let Some(user) = arg.strip_prefix('+') {
                            add.push(user.trim_start_matches('@').to_string());
                        } else if let Some(user) = arg.strip_prefix('-') {
                            remove.push(user.trim_start_matches('@').to_string());
                        } else {
                            add.push(clean.to_string());
                        }
                    }
                    commands.push(SlashCommand::Reviewer { add, remove });
                }
                "unreviewer" | "unreviewers" => {
                    let remove: Vec<String> = args
                        .iter()
                        .map(|s| s.trim_start_matches('@').to_string())
                        .collect();
                    commands.push(SlashCommand::Reviewer {
                        add: Vec::new(),
                        remove,
                    });
                }
                "help" => {
                    commands.push(SlashCommand::Help);
                }
                _ => {}
            }
        }

        commands
    }
}

pub struct CommandExecutor<'a> {
    client: &'a GitHubClient,
    projects: &'a ProjectsService,
    notifier: &'a NotificationDispatcher,
    config: &'a CommandsConfig,
    balancer: Option<ReviewerLoadBalancer>,
}

impl<'a> CommandExecutor<'a> {
    pub fn new(
        client: &'a GitHubClient,
        projects: &'a ProjectsService,
        notifier: &'a NotificationDispatcher,
        config: &'a CommandsConfig,
        balancer: Option<ReviewerLoadBalancer>,
    ) -> Self {
        Self {
            client,
            projects,
            notifier,
            config,
            balancer,
        }
    }

    pub fn is_authorized(&self, command: &SlashCommand, sender: &User) -> bool {
        let cmd_name = command.name();

        let is_restricted = self.config.restricted_commands.iter().any(|r| {
            r.trim_start_matches('/').eq_ignore_ascii_case(cmd_name)
        });

        if !is_restricted {
            return true;
        }

        // If restricted, sender MUST be in super_users list
        self.config.super_users.iter().any(|su| {
            su.trim_start_matches('@').eq_ignore_ascii_case(&sender.login)
        })
    }

    pub async fn execute(
        &self,
        command: &SlashCommand,
        owner: &str,
        repo: &str,
        issue: &Issue,
        sender: &User,
    ) -> Result<()> {
        let repo_full_name = format!("{}/{}", owner, repo);
        let issue_url = format!(
            "https://github.com/{}/{}/issues/{}",
            owner, repo, issue.number
        );

        if !self.is_authorized(command, sender) {
            info!(
                "Blocked restricted command '/{}' by @{} on {}#{}",
                command.name(),
                sender.login,
                repo_full_name,
                issue.number
            );
            let comment_text = format!(
                "⛔ @{}, the command `/{}` is restricted to authorized super users.",
                sender.login,
                command.name()
            );
            self.client
                .create_comment(owner, repo, issue.number, &comment_text)
                .await?;
            return Ok(());
        }

        match command {
            SlashCommand::Assign(users) => {
                let to_assign = if users.is_empty() {
                    vec![sender.login.clone()]
                } else {
                    users.clone()
                };
                self.client
                    .add_assignees(owner, repo, issue.number, &to_assign)
                    .await?;
                info!(
                    "Assigned {:?} to {}#{}",
                    to_assign, repo_full_name, issue.number
                );

                let title = format!(
                    "👤 Assign: {}#{} to {}",
                    repo_full_name,
                    issue.number,
                    to_assign.join(", ")
                );
                let msg = format!(
                    "**@{}** assigned **{}** to: {}\n\n**Issue**: {}",
                    sender.login,
                    to_assign.join(", "),
                    issue.title,
                    issue_url
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::Assigned,
                        &title,
                        &msg,
                        Some(&issue_url),
                        NotificationLevel::Info,
                        vec![
                            DiscordEmbedField {
                                name: "Assignee(s)".to_string(),
                                value: to_assign.join(", "),
                                inline: true,
                            },
                            DiscordEmbedField {
                                name: "Assigned By".to_string(),
                                value: sender.login.clone(),
                                inline: true,
                            },
                        ],
                    )
                    .await;
            }
            SlashCommand::Unassign(users) => {
                let to_unassign = if users.is_empty() {
                    vec![sender.login.clone()]
                } else {
                    users.clone()
                };
                self.client
                    .remove_assignees(owner, repo, issue.number, &to_unassign)
                    .await?;
                info!(
                    "Unassigned {:?} from {}#{}",
                    to_unassign, repo_full_name, issue.number
                );

                let title = format!(
                    "👤 Unassign: {}#{} removed {}",
                    repo_full_name,
                    issue.number,
                    to_unassign.join(", ")
                );
                let msg = format!(
                    "**@{}** removed **{}** from: {}",
                    sender.login,
                    to_unassign.join(", "),
                    issue.title
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::Unassigned,
                        &title,
                        &msg,
                        Some(&issue_url),
                        NotificationLevel::Info,
                        Vec::new(),
                    )
                    .await;
            }
            SlashCommand::Label { add, remove } => {
                if !add.is_empty() {
                    self.client
                        .add_labels(owner, repo, issue.number, add)
                        .await?;
                }
                for lbl in remove {
                    let _ = self
                        .client
                        .remove_label(owner, repo, issue.number, lbl)
                        .await;
                }
                info!("Updated labels on {}#{}", repo_full_name, issue.number);

                let title = format!("🏷️ Label Update: {}#{}", repo_full_name, issue.number);
                let desc = format!(
                    "**@{}** updated labels on **#{}**: +{:?} -{:?}",
                    sender.login, issue.number, add, remove
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::LabelChanged,
                        &title,
                        &desc,
                        Some(&issue_url),
                        NotificationLevel::Info,
                        Vec::new(),
                    )
                    .await;
            }
            SlashCommand::Unlabel(labels) => {
                for lbl in labels {
                    let _ = self
                        .client
                        .remove_label(owner, repo, issue.number, lbl)
                        .await;
                }
                info!(
                    "Removed labels {:?} from {}#{}",
                    labels, repo_full_name, issue.number
                );
            }
            SlashCommand::Close(reason) => {
                let state_reason = match reason.as_deref() {
                    Some(r) if r.contains("not planned") => Some("not_planned"),
                    _ => Some("completed"),
                };
                self.client
                    .update_issue_state(owner, repo, issue.number, "closed", state_reason)
                    .await?;
                let comment_body = if let Some(r) = reason {
                    format!("Closed by @{}: {}", sender.login, r)
                } else {
                    format!("Closed by @{}.", sender.login)
                };
                self.client
                    .create_comment(owner, repo, issue.number, &comment_body)
                    .await?;
                info!("Closed {}#{}", repo_full_name, issue.number);

                let title = format!("🔒 Closed: {}#{}", repo_full_name, issue.number);
                let desc = format!(
                    "Item closed by **@{}**: {}\n\n**Reason**: {}",
                    sender.login,
                    issue.title,
                    reason.as_deref().unwrap_or("completed")
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::IssueClosed,
                        &title,
                        &desc,
                        Some(&issue_url),
                        NotificationLevel::Warning,
                        Vec::new(),
                    )
                    .await;
            }
            SlashCommand::Reopen => {
                self.client
                    .update_issue_state(owner, repo, issue.number, "open", Some("reopened"))
                    .await?;
                self.client
                    .create_comment(
                        owner,
                        repo,
                        issue.number,
                        &format!("Reopened by @{}.", sender.login),
                    )
                    .await?;
                info!("Reopened {}#{}", repo_full_name, issue.number);
            }
            SlashCommand::Retest => {
                if issue.is_pr() {
                    let pr = self
                        .client
                        .get_pull_request(owner, repo, issue.number)
                        .await?;
                    let check_runs = self
                        .client
                        .list_check_runs_for_ref(owner, repo, &pr.head.sha)
                        .await?;
                    let mut retested_count = 0;
                    for cr in &check_runs {
                        if cr.conclusion.as_deref() == Some("failure") {
                            let _ = self.client.rerequest_check_suite(owner, repo, cr.id).await;
                            retested_count += 1;
                        }
                    }
                    self.client
                        .create_comment(
                            owner,
                            repo,
                            issue.number,
                            &format!(
                                "🔄 Retest triggered for {} failed check(s) by @{}.",
                                retested_count, sender.login
                            ),
                        )
                        .await?;
                } else {
                    self.client
                        .create_comment(
                            owner,
                            repo,
                            issue.number,
                            "⚠️ `/retest` is only applicable to pull requests.",
                        )
                        .await?;
                }
            }
            SlashCommand::Project { target, status } => {
                if let Ok(project_num) = target.parse::<i64>() {
                    let node_id = self
                        .get_node_id_for_issue(owner, repo, issue.number)
                        .await?;
                    if let Some(nid) = node_id {
                        self.projects
                            .add_item_to_project(
                                self.client,
                                &nid,
                                Some(owner),
                                Some(project_num),
                                status.as_deref(),
                            )
                            .await?;
                        self.client
                            .create_comment(
                                owner,
                                repo,
                                issue.number,
                                &format!(
                                    "📋 Successfully added to Project #{} (status: {:?}).",
                                    project_num, status
                                ),
                            )
                            .await?;
                    }
                } else {
                    self.client
                        .create_comment(
                            owner,
                            repo,
                            issue.number,
                            &format!("⚠️ Could not parse project number: `{}`", target),
                        )
                        .await?;
                }
            }
            SlashCommand::Move { target_repo } => {
                let (target_owner, target_name) = if target_repo.contains('/') {
                    let mut parts = target_repo.splitn(2, '/');
                    (parts.next().unwrap(), parts.next().unwrap())
                } else {
                    (owner, target_repo.as_str())
                };

                if target_owner == owner && target_name == repo {
                    self.client
                        .create_comment(
                            owner,
                            repo,
                            issue.number,
                            "⚠️ Cannot move issue to the same repository.",
                        )
                        .await?;
                    return Ok(());
                }

                let migrated_body = format!(
                    "> 📦 *Migrated from {}/{}#{} originally opened by @{}*\n\n---\n\n{}",
                    owner,
                    repo,
                    issue.number,
                    issue.user.login,
                    issue.body.as_deref().unwrap_or("*No description provided*")
                );

                let label_names: Vec<String> =
                    issue.labels.iter().map(|l| l.name.clone()).collect();

                let new_issue = self
                    .client
                    .create_issue(
                        target_owner,
                        target_name,
                        &issue.title,
                        Some(&migrated_body),
                        &label_names,
                        &[],
                    )
                    .await?;

                let new_issue_url = format!(
                    "https://github.com/{}/{}/issues/{}",
                    target_owner, target_name, new_issue.number
                );

                let close_comment = format!(
                    "📦 This issue has been migrated to {}/{}#{} by @{}.",
                    target_owner, target_name, new_issue.number, sender.login
                );
                self.client
                    .create_comment(owner, repo, issue.number, &close_comment)
                    .await?;
                self.client
                    .update_issue_state(owner, repo, issue.number, "closed", Some("completed"))
                    .await?;

                info!(
                    "Migrated {}/{}#{} to {}/{}#{}",
                    owner, repo, issue.number, target_owner, target_name, new_issue.number
                );

                // Notify Discord & Zulip of cross-repo move
                let title = format!(
                    "📦 Issue Migrated: {}#{} ➔ {}/{}#{}",
                    repo_full_name, issue.number, target_owner, target_name, new_issue.number
                );
                let msg = format!(
                    "**@{}** migrated **{}**\nFrom: `{}`\nTo: `{}/{}`",
                    sender.login, issue.title, repo_full_name, target_owner, target_name
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::CrossRepoMove,
                        &title,
                        &msg,
                        Some(&new_issue_url),
                        NotificationLevel::Info,
                        vec![
                            DiscordEmbedField {
                                name: "Origin".to_string(),
                                value: format!("{}#{}", repo_full_name, issue.number),
                                inline: true,
                            },
                            DiscordEmbedField {
                                name: "Destination".to_string(),
                                value: format!(
                                    "{}/{}#{}",
                                    target_owner, target_name, new_issue.number
                                ),
                                inline: true,
                            },
                        ],
                    )
                    .await;
            }
            SlashCommand::Duplicate(target_issue_num) => {
                self.client
                    .add_labels(owner, repo, issue.number, &["duplicate".to_string()])
                    .await?;
                let close_comment = format!(
                    "🔒 Marked as duplicate of #{} by @{}.\n\nClosing this issue.",
                    target_issue_num, sender.login
                );
                self.client
                    .create_comment(owner, repo, issue.number, &close_comment)
                    .await?;
                self.client
                    .update_issue_state(owner, repo, issue.number, "closed", Some("not_planned"))
                    .await?;
                info!(
                    "Marked {}#{} as duplicate of #{}",
                    repo_full_name, issue.number, target_issue_num
                );
            }
            SlashCommand::Reviewer { add, remove } => {
                if !issue.is_pr() {
                    self.client
                        .create_comment(
                            owner,
                            repo,
                            issue.number,
                            "⚠️ Reviewers can only be assigned or updated on Pull Requests, not issues.",
                        )
                        .await?;
                    return Ok(());
                }

                let mut to_add = add.clone();
                let to_remove = remove.clone();

                // If no reviewers specified (e.g. `/reviewer`), attempt auto-selection using load balancer
                if to_add.is_empty() && to_remove.is_empty() {
                    if let Some(balancer) = &self.balancer {
                        let pr_files = match self.client.list_pr_files(owner, repo, issue.number).await {
                            Ok(files) => files.into_iter().map(|f| f.filename).collect(),
                            Err(_) => Vec::new(),
                        };
                        let exclude = vec![sender.login.clone(), issue.user.login.clone()];
                        if let Ok(chosen) = balancer.select_reviewers_for_pr(
                            &repo_full_name,
                            &pr_files,
                            &[],
                            &exclude,
                        ) {
                            to_add = chosen;
                        }
                    }
                }

                if to_add.is_empty() && to_remove.is_empty() {
                    self.client
                        .create_comment(
                            owner,
                            repo,
                            issue.number,
                            "⚠️ No reviewers specified or available to assign.",
                        )
                        .await?;
                    return Ok(());
                }

                let mut comment_parts = Vec::new();

                if !to_add.is_empty() {
                    self.client
                        .request_reviewers(owner, repo, issue.number, &to_add)
                        .await?;
                    if let Some(balancer) = &self.balancer {
                        for r in &to_add {
                            let _ = balancer.on_review_requested(&repo_full_name, r);
                        }
                    }
                    let mentions: Vec<String> = to_add.iter().map(|u| format!("@{}", u)).collect();
                    comment_parts.push(format!("Requested review from: {}", mentions.join(", ")));
                }

                if !to_remove.is_empty() {
                    self.client
                        .remove_reviewers(owner, repo, issue.number, &to_remove)
                        .await?;
                    if let Some(balancer) = &self.balancer {
                        for r in &to_remove {
                            let _ = balancer.on_review_completed_or_closed(&repo_full_name, r);
                        }
                    }
                    let mentions: Vec<String> = to_remove.iter().map(|u| format!("@{}", u)).collect();
                    comment_parts.push(format!("Removed review request for: {}", mentions.join(", ")));
                }

                let comment_body = format!(
                    "👥 **Reviewer update by @{}**:\n- {}",
                    sender.login,
                    comment_parts.join("\n- ")
                );
                self.client
                    .create_comment(owner, repo, issue.number, &comment_body)
                    .await?;

                info!(
                    "Updated reviewers on {}#{} (add: {:?}, remove: {:?})",
                    repo_full_name, issue.number, to_add, to_remove
                );

                let title = format!(
                    "👥 Reviewers Updated: {}#{}",
                    repo_full_name, issue.number
                );
                let mut embed_fields = Vec::new();
                if !to_add.is_empty() {
                    embed_fields.push(DiscordEmbedField {
                        name: "Added Reviewers".to_string(),
                        value: to_add.join(", "),
                        inline: true,
                    });
                }
                if !to_remove.is_empty() {
                    embed_fields.push(DiscordEmbedField {
                        name: "Removed Reviewers".to_string(),
                        value: to_remove.join(", "),
                        inline: true,
                    });
                }
                embed_fields.push(DiscordEmbedField {
                    name: "Updated By".to_string(),
                    value: sender.login.clone(),
                    inline: true,
                });

                self.notifier
                    .dispatch_event(
                        SugieEventType::Assigned,
                        &title,
                        &comment_body,
                        Some(&issue_url),
                        NotificationLevel::Info,
                        embed_fields,
                    )
                    .await;
            }
            SlashCommand::Help => {
                let help_text = r#"
### 🤖 Sugie Bot Commands Reference

You can trigger commands either with a slash (`/command`) or by mentioning the bot (e.g. `@starlabssisugie label +bug`):

| Command | Usage | Description |
| :--- | :--- | :--- |
| `/assign [@user...]` | `/assign @alice` or `@bot assign` | Assigns specified user(s) or self |
| `/unassign [@user...]` | `/unassign @bob` or `@bot unassign` | Removes assignment |
| `/label [+name] [-name]` | `/label +bug -wip` or `@bot label +bug` | Adds/removes labels |
| `/unlabel <name>` | `/unlabel bug` | Removes a label |
| `/reviewer [+@user] [-@user]` | `/reviewer @alice` or `/reviewer +@alice -@bob` | Requests or removes reviewers (PRs only) |
| `/unreviewer <@user...>` | `/unreviewer @bob` | Removes requested reviewer (PRs only) |
| `/close [reason]` | `/close completed` | Closes issue or PR |
| `/reopen` | `/reopen` | Reopens issue or PR |
| `/retest` | `/retest` | Retests failed CI checks (PRs only) |
| `/project <num> [status]` | `/project 1 Triage` | Adds to GitHub Project v2 board |
| `/move <target_repo>` | `/move org/new-repo` | Migrates issue across repositories |
| `/duplicate <#num>` | `/duplicate #123` | Closes issue as duplicate of #123 |
| `/help` | `/help` or `@bot` or `@bot help` | Displays this help manual |
"#;
                self.client
                    .create_comment(owner, repo, issue.number, help_text.trim())
                    .await?;
            }
        }

        Ok(())
    }

    async fn get_node_id_for_issue(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
    ) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct IssueNodeData {
            repository: Option<RepoNode>,
        }
        #[derive(Deserialize)]
        struct RepoNode {
            issue: Option<NodeDetails>,
            #[serde(rename = "pullRequest")]
            pull_request: Option<NodeDetails>,
        }
        #[derive(Deserialize)]
        struct NodeDetails {
            id: String,
        }

        let query = r#"
        query($owner: String!, $repo: String!, $number: Int!) {
            repository(owner: $owner, name: $repo) {
                issue(number: $number) { id }
                pullRequest(number: $number) { id }
            }
        }
        "#;

        let vars = serde_json::json!({
            "owner": owner,
            "repo": repo,
            "number": issue_number,
        });

        let data: IssueNodeData = self.client.graphql(query, vars).await?;
        let node_id = data.repository.and_then(|r| {
            r.issue
                .map(|i| i.id)
                .or_else(|| r.pull_request.map(|p| p.id))
        });
        Ok(node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_parser_with_prefix_and_alias() {
        let comment = r#"
        /a @alice
        /l +bug -wip
        /close not planned
        "#;

        let config = CommandsConfig::default();
        let cmds = CommandParser::parse_comment(comment, &config);
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], SlashCommand::Assign(vec!["alice".to_string()]));
        assert_eq!(
            cmds[1],
            SlashCommand::Label {
                add: vec!["bug".to_string()],
                remove: vec!["wip".to_string()],
            }
        );
        assert_eq!(
            cmds[2],
            SlashCommand::Close(Some("not planned".to_string()))
        );
    }

    #[test]
    fn test_reviewer_command_parsing() {
        let comment = r#"
        /reviewer @alice @bob
        /reviewer +@charlie -@david
        /unreviewer @eve
        @bot review @frank
        @bot unreviewers @grace
        /reviewer
        "#;

        let config = CommandsConfig::default();
        let cmds = CommandParser::parse_comment(comment, &config);
        assert_eq!(cmds.len(), 6);
        assert_eq!(
            cmds[0],
            SlashCommand::Reviewer {
                add: vec!["alice".to_string(), "bob".to_string()],
                remove: vec![],
            }
        );
        assert_eq!(
            cmds[1],
            SlashCommand::Reviewer {
                add: vec!["charlie".to_string()],
                remove: vec!["david".to_string()],
            }
        );
        assert_eq!(
            cmds[2],
            SlashCommand::Reviewer {
                add: vec![],
                remove: vec!["eve".to_string()],
            }
        );
        assert_eq!(
            cmds[3],
            SlashCommand::Reviewer {
                add: vec!["frank".to_string()],
                remove: vec![],
            }
        );
        assert_eq!(
            cmds[4],
            SlashCommand::Reviewer {
                add: vec![],
                remove: vec!["grace".to_string()],
            }
        );
        assert_eq!(
            cmds[5],
            SlashCommand::Reviewer {
                add: vec![],
                remove: vec![],
            }
        );
    }
}

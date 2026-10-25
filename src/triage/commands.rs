use crate::github::client::GitHubClient;
use crate::github::models::{Issue, User};
use crate::triage::projects::ProjectsService;
use anyhow::Result;
use serde::Deserialize;
use tracing::info;

#[derive(Debug, PartialEq, Eq)]
pub enum SlashCommand {
    Assign(Vec<String>),
    Unassign(Vec<String>),
    Label { add: Vec<String>, remove: Vec<String> },
    Unlabel(Vec<String>),
    Close(Option<String>),
    Reopen,
    Retest,
    Project { target: String, status: Option<String> },
    Move { target_repo: String },
    Duplicate(u64),
    Help,
}

pub struct CommandParser;

impl CommandParser {
    pub fn parse_comment(comment_body: &str) -> Vec<SlashCommand> {
        let mut commands = Vec::new();

        for line in comment_body.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('/') {
                continue;
            }

            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            let cmd_name = parts[0].to_lowercase();
            let args = &parts[1..];

            match cmd_name.as_str() {
                "/assign" => {
                    let users: Vec<String> = args
                        .iter()
                        .map(|s| s.trim_start_matches('@').to_string())
                        .collect();
                    commands.push(SlashCommand::Assign(users));
                }
                "/unassign" => {
                    let users: Vec<String> = args
                        .iter()
                        .map(|s| s.trim_start_matches('@').to_string())
                        .collect();
                    commands.push(SlashCommand::Unassign(users));
                }
                "/label" => {
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
                "/unlabel" => {
                    let labels: Vec<String> = args.iter().map(|s| s.to_string()).collect();
                    if !labels.is_empty() {
                        commands.push(SlashCommand::Unlabel(labels));
                    }
                }
                "/close" => {
                    let reason = if !args.is_empty() {
                        Some(args.join(" "))
                    } else {
                        None
                    };
                    commands.push(SlashCommand::Close(reason));
                }
                "/reopen" => {
                    commands.push(SlashCommand::Reopen);
                }
                "/retest" => {
                    commands.push(SlashCommand::Retest);
                }
                "/project" => {
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
                "/move" => {
                    if !args.is_empty() {
                        commands.push(SlashCommand::Move {
                            target_repo: args[0].to_string(),
                        });
                    }
                }
                "/duplicate" => {
                    if !args.is_empty() {
                        let num_str = args[0].trim_start_matches('#');
                        if let Ok(num) = num_str.parse::<u64>() {
                            commands.push(SlashCommand::Duplicate(num));
                        }
                    }
                }
                "/help" => {
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
}

impl<'a> CommandExecutor<'a> {
    pub fn new(client: &'a GitHubClient, projects: &'a ProjectsService) -> Self {
        Self { client, projects }
    }

    pub async fn execute(
        &self,
        command: &SlashCommand,
        owner: &str,
        repo: &str,
        issue: &Issue,
        sender: &User,
    ) -> Result<()> {
        match command {
            SlashCommand::Assign(users) => {
                let to_assign = if users.is_empty() {
                    vec![sender.login.clone()]
                } else {
                    users.clone()
                };
                self.client.add_assignees(owner, repo, issue.number, &to_assign).await?;
                info!("Assigned {:?} to {}/{}#{}", to_assign, owner, repo, issue.number);
            }
            SlashCommand::Unassign(users) => {
                let to_unassign = if users.is_empty() {
                    vec![sender.login.clone()]
                } else {
                    users.clone()
                };
                self.client.remove_assignees(owner, repo, issue.number, &to_unassign).await?;
                info!("Unassigned {:?} from {}/{}#{}", to_unassign, owner, repo, issue.number);
            }
            SlashCommand::Label { add, remove } => {
                if !add.is_empty() {
                    self.client.add_labels(owner, repo, issue.number, add).await?;
                }
                for lbl in remove {
                    let _ = self.client.remove_label(owner, repo, issue.number, lbl).await;
                }
                info!("Updated labels on {}/{}#{}", owner, repo, issue.number);
            }
            SlashCommand::Unlabel(labels) => {
                for lbl in labels {
                    let _ = self.client.remove_label(owner, repo, issue.number, lbl).await;
                }
                info!("Removed labels {:?} from {}/{}#{}", labels, owner, repo, issue.number);
            }
            SlashCommand::Close(reason) => {
                let state_reason = match reason.as_deref() {
                    Some(r) if r.contains("not planned") => Some("not_planned"),
                    _ => Some("completed"),
                };
                self.client.update_issue_state(owner, repo, issue.number, "closed", state_reason).await?;
                let comment_body = if let Some(r) = reason {
                    format!("Closed by @{}: {}", sender.login, r)
                } else {
                    format!("Closed by @{}.", sender.login)
                };
                self.client.create_comment(owner, repo, issue.number, &comment_body).await?;
                info!("Closed {}/{}#{}", owner, repo, issue.number);
            }
            SlashCommand::Reopen => {
                self.client.update_issue_state(owner, repo, issue.number, "open", Some("reopened")).await?;
                self.client.create_comment(
                    owner,
                    repo,
                    issue.number,
                    &format!("Reopened by @{}.", sender.login),
                ).await?;
                info!("Reopened {}/{}#{}", owner, repo, issue.number);
            }
            SlashCommand::Retest => {
                if issue.is_pr() {
                    let pr = self.client.get_pull_request(owner, repo, issue.number).await?;
                    let check_runs = self.client.list_check_runs_for_ref(owner, repo, &pr.head.sha).await?;
                    let mut retested_count = 0;
                    for cr in &check_runs {
                        if cr.conclusion.as_deref() == Some("failure") {
                            let _ = self.client.rerequest_check_suite(owner, repo, cr.id).await;
                            retested_count += 1;
                        }
                    }
                    self.client.create_comment(
                        owner,
                        repo,
                        issue.number,
                        &format!("🔄 Triggered retest for {} failed check suite(s).", retested_count),
                    ).await?;
                } else {
                    self.client.create_comment(
                        owner,
                        repo,
                        issue.number,
                        "⚠️ `/retest` is only applicable to pull requests.",
                    ).await?;
                }
            }
            SlashCommand::Project { target, status } => {
                // If target is a number, parse it
                if let Ok(project_num) = target.parse::<i64>() {
                    let node_id = self.get_node_id_for_issue(owner, repo, issue.number).await?;
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
                        self.client.create_comment(
                            owner,
                            repo,
                            issue.number,
                            &format!("📋 Successfully added to Project #{} (status: {:?}).", project_num, status),
                        ).await?;
                    }
                } else {
                    self.client.create_comment(
                        owner,
                        repo,
                        issue.number,
                        &format!("⚠️ Could not parse project number: `{}`", target),
                    ).await?;
                }
            }
            SlashCommand::Move { target_repo } => {
                // Cross-repo migration:
                // target_repo can be "org/repo" or "repo" (defaults to same owner)
                let (target_owner, target_name) = if target_repo.contains('/') {
                    let mut parts = target_repo.splitn(2, '/');
                    (parts.next().unwrap(), parts.next().unwrap())
                } else {
                    (owner, target_repo.as_str())
                };

                // Prevent moving to self
                if target_owner == owner && target_name == repo {
                    self.client.create_comment(
                        owner,
                        repo,
                        issue.number,
                        "⚠️ Cannot move issue to the same repository.",
                    ).await?;
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

                let label_names: Vec<String> = issue.labels.iter().map(|l| l.name.clone()).collect();

                let new_issue = self.client.create_issue(
                    target_owner,
                    target_name,
                    &issue.title,
                    Some(&migrated_body),
                    &label_names,
                    &[],
                ).await?;

                // Close original issue with migration link
                let close_comment = format!(
                    "📦 This issue has been migrated to {}/{}#{} by @{}.",
                    target_owner, target_name, new_issue.number, sender.login
                );
                self.client.create_comment(owner, repo, issue.number, &close_comment).await?;
                self.client.update_issue_state(owner, repo, issue.number, "closed", Some("completed")).await?;

                info!(
                    "Migrated {}/{}#{} to {}/{}#{}",
                    owner, repo, issue.number, target_owner, target_name, new_issue.number
                );
            }
            SlashCommand::Duplicate(target_issue_num) => {
                self.client.add_labels(owner, repo, issue.number, &["duplicate".to_string()]).await?;
                let close_comment = format!(
                    "🔒 Marked as duplicate of #{} by @{}.\n\nClosing this issue.",
                    target_issue_num, sender.login
                );
                self.client.create_comment(owner, repo, issue.number, &close_comment).await?;
                self.client.update_issue_state(owner, repo, issue.number, "closed", Some("not_planned")).await?;
                info!("Marked {}/{}#{} as duplicate of #{}", owner, repo, issue.number, target_issue_num);
            }
            SlashCommand::Help => {
                let help_text = r#"
### 🤖 Sugie Bot Commands Reference

| Command | Usage | Description |
| :--- | :--- | :--- |
| `/assign [@user...]` | `/assign @alice` or `/assign` | Assigns specified user(s) or self |
| `/unassign [@user...]` | `/unassign @bob` or `/unassign` | Removes assignment |
| `/label [+name] [-name]` | `/label +bug -wip` | Adds/removes labels |
| `/unlabel <name>` | `/unlabel bug` | Removes a label |
| `/close [reason]` | `/close completed` | Closes issue or PR |
| `/reopen` | `/reopen` | Reopens issue or PR |
| `/retest` | `/retest` | Retests failed CI checks (PRs only) |
| `/project <num> [status]` | `/project 1 Triage` | Adds to GitHub Project v2 board |
| `/move <target_repo>` | `/move org/new-repo` | Migrates issue across repositories |
| `/duplicate <#num>` | `/duplicate #123` | Closes issue as duplicate of #123 |
| `/help` | `/help` | Displays this help manual |
"#;
                self.client.create_comment(owner, repo, issue.number, help_text.trim()).await?;
            }
        }

        Ok(())
    }

    async fn get_node_id_for_issue(&self, owner: &str, repo: &str, issue_number: u64) -> Result<Option<String>> {
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
        let node_id = data
            .repository
            .and_then(|r| r.issue.map(|i| i.id).or_else(|| r.pull_request.map(|p| p.id)));
        Ok(node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_parser() {
        let comment = r#"
        LGTM!
        /assign @alice @bob
        /label +bug +priority-high -wip
        /move my-org/other-repo
        /retest
        /duplicate #456
        /close completed
        "#;

        let cmds = CommandParser::parse_comment(comment);
        assert_eq!(cmds.len(), 6);
        assert_eq!(cmds[0], SlashCommand::Assign(vec!["alice".to_string(), "bob".to_string()]));
        assert_eq!(cmds[1], SlashCommand::Label {
            add: vec!["bug".to_string(), "priority-high".to_string()],
            remove: vec!["wip".to_string()],
        });
        assert_eq!(cmds[2], SlashCommand::Move {
            target_repo: "my-org/other-repo".to_string(),
        });
        assert_eq!(cmds[3], SlashCommand::Retest);
        assert_eq!(cmds[4], SlashCommand::Duplicate(456));
        assert_eq!(cmds[5], SlashCommand::Close(Some("completed".to_string())));
    }
}

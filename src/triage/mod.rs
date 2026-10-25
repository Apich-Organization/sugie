pub mod ci_summary;
pub mod codeowners;
pub mod commands;
pub mod dco;
pub mod duplicate;
pub mod first_time;
pub mod growth;
pub mod labels;
pub mod projects;
pub mod reviewers;

use crate::config::{RepoConfig, SugieConfig};
use crate::github::auth::GitHubAuthManager;
use crate::github::client::GitHubClient;
use crate::github::models::*;
use crate::notifications::{
    DiscordEmbedField, NotificationDispatcher, NotificationLevel, SugieEventType,
};
use crate::storage::Storage;
use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct TriageService {
    config: Arc<SugieConfig>,
    storage: Storage,
    notifier: NotificationDispatcher,
    auth: Arc<GitHubAuthManager>,
}

impl TriageService {
    pub fn new(
        config: Arc<SugieConfig>,
        storage: Storage,
        notifier: NotificationDispatcher,
        auth: Arc<GitHubAuthManager>,
    ) -> Self {
        Self {
            config,
            storage,
            notifier,
            auth,
        }
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    pub fn notifier(&self) -> &NotificationDispatcher {
        &self.notifier
    }

    pub async fn get_client_for_installation(&self, installation_id: u64) -> Result<GitHubClient> {
        let token = self.auth.get_installation_token(installation_id).await?;
        Ok(GitHubClient::new(token, None))
    }

    pub async fn get_client_for_repo(&self, full_name: &str) -> Result<GitHubClient> {
        let inst_id = self
            .storage
            .get_installation_id_for_repo(full_name)?
            .context(format!("No installation ID found for repo {}", full_name))?;
        self.get_client_for_installation(inst_id).await
    }

    pub async fn load_repo_config(
        &self,
        client: &GitHubClient,
        owner: &str,
        repo: &str,
    ) -> RepoConfig {
        match client
            .get_repo_file_content(owner, repo, ".github/sugie.toml", None)
            .await
        {
            Ok(Some(content)) => match toml::from_str::<RepoConfig>(&content) {
                Ok(repo_cfg) => {
                    info!("Loaded custom .github/sugie.toml for {}/{}", owner, repo);
                    return self
                        .config
                        .default_rules
                        .merge_with_override(Some(repo_cfg));
                }
                Err(e) => {
                    warn!(
                        "Failed to parse .github/sugie.toml in {}/{}: {:?}",
                        owner, repo, e
                    );
                }
            },
            Ok(None) => {}
            Err(e) => {
                warn!(
                    "Error fetching .github/sugie.toml from {}/{}: {:?}",
                    owner, repo, e
                );
            }
        }
        self.config.default_rules.clone()
    }

    // --- Webhook Event Handlers ---

    pub async fn handle_installation_repositories(
        &self,
        event: InstallationRepositoriesWebhookEvent,
    ) -> Result<()> {
        let inst_id = event.installation.id;
        for repo in &event.repositories_added {
            self.storage
                .upsert_repository(repo.id, &repo.full_name, inst_id)?;
            info!(
                "Onboarded repository {} (ID: {}) for installation {}",
                repo.full_name, repo.id, inst_id
            );
        }
        for repo in &event.repositories_removed {
            self.storage.remove_repository(&repo.full_name)?;
            info!("Removed repository {} from triage bot", repo.full_name);
        }
        Ok(())
    }

    pub async fn handle_installation(&self, event: InstallationWebhookEvent) -> Result<()> {
        let inst_id = event.installation.id;
        if event.action == "created" {
            for repo in &event.repositories {
                self.storage
                    .upsert_repository(repo.id, &repo.full_name, inst_id)?;
                info!(
                    "Onboarded repository {} on initial installation {}",
                    repo.full_name, inst_id
                );
            }
        } else if event.action == "deleted" {
            for repo in &event.repositories {
                self.storage.remove_repository(&repo.full_name)?;
            }
        }
        Ok(())
    }

    pub async fn handle_issue(&self, event: IssueWebhookEvent) -> Result<()> {
        let owner = &event.repository.owner.login;
        let repo = &event.repository.name;
        let full_name = &event.repository.full_name;
        let issue_url = format!(
            "https://github.com/{}/{}/issues/{}",
            owner, repo, event.issue.number
        );

        let inst_id = match event.installation.as_ref().map(|i| i.id) {
            Some(id) => {
                self.storage
                    .upsert_repository(event.repository.id, full_name, id)?;
                id
            }
            None => self
                .storage
                .get_installation_id_for_repo(full_name)?
                .context("No installation ID for issue event")?,
        };

        let client = self.get_client_for_installation(inst_id).await?;
        let repo_config = self.load_repo_config(&client, owner, repo).await;

        if event.action == "opened" {
            let mut labels_to_add = Vec::new();

            // 1. Notify Discord/Zulip of new issue opened
            let issue_title_msg = format!(
                "🆕 New Issue: {}#{} by @{}",
                full_name, event.issue.number, event.sender.login
            );
            self.notifier
                .dispatch_event(
                    SugieEventType::IssueOpened,
                    &issue_title_msg,
                    &event.issue.title,
                    Some(&issue_url),
                    NotificationLevel::Info,
                    vec![
                        DiscordEmbedField {
                            name: "Author".to_string(),
                            value: event.sender.login.clone(),
                            inline: true,
                        },
                        DiscordEmbedField {
                            name: "Issue".to_string(),
                            value: format!("#{}", event.issue.number),
                            inline: true,
                        },
                    ],
                )
                .await;

            // 2. First-time contributor handling
            let first_time = first_time::FirstTimeHandler::new(repo_config.first_time.clone());
            let stats = self
                .storage
                .get_contributor_stats(&event.sender.login, full_name)?;
            let has_prior = stats
                .map(|s| s.opened_issues + s.merged_prs > 0)
                .unwrap_or(false);

            if first_time.is_first_time(event.issue.author_association.as_deref(), has_prior) {
                let welcome = first_time.build_welcome_comment(&event.sender.login, false);
                let _ = client
                    .create_comment(owner, repo, event.issue.number, &welcome)
                    .await;
                for l in first_time.labels_to_add(false) {
                    labels_to_add.push(l.clone());
                }
            }

            // 3. Duplicate detection
            if repo_config.duplicates.enabled {
                let detector = duplicate::DuplicateDetector::new(repo_config.duplicates.clone());
                let existing = client.list_repo_issues(owner, repo, "open", 100).await?;
                let matches = detector.find_duplicates(
                    event.issue.number,
                    &event.issue.title,
                    event.issue.body.as_deref(),
                    &existing,
                );

                if !matches.is_empty() {
                    let should_close = detector.should_auto_close(&matches);
                    let dup_comment = detector.build_comment_body(
                        &event.sender.login,
                        event.issue.number,
                        &matches,
                        should_close,
                    );
                    let _ = client
                        .create_comment(owner, repo, event.issue.number, &dup_comment)
                        .await;

                    if let Some(ref dup_lbl) = repo_config.duplicates.add_label {
                        labels_to_add.push(dup_lbl.clone());
                    }

                    if should_close {
                        let _ = client
                            .update_issue_state(
                                owner,
                                repo,
                                event.issue.number,
                                "closed",
                                Some("not_planned"),
                            )
                            .await;
                    }

                    // Notify Discord & Zulip of duplicate
                    let top_match = &matches[0];
                    let title = format!(
                        "🔍 Duplicate Detected: {}#{}",
                        full_name, event.issue.number
                    );
                    let desc = format!(
                        "Issue #{} matches existing **#{}** with {:.1}% similarity.\n\nTitle: **{}**",
                        event.issue.number, top_match.issue_number, top_match.similarity * 100.0, event.issue.title
                    );
                    self.notifier
                        .dispatch_event(
                            SugieEventType::DuplicateDetected,
                            &title,
                            &desc,
                            Some(&issue_url),
                            NotificationLevel::Warning,
                            vec![
                                DiscordEmbedField {
                                    name: "Matched Issue".to_string(),
                                    value: format!("#{}", top_match.issue_number),
                                    inline: true,
                                },
                                DiscordEmbedField {
                                    name: "Similarity".to_string(),
                                    value: format!("{:.1}%", top_match.similarity * 100.0),
                                    inline: true,
                                },
                            ],
                        )
                        .await;
                }
            }

            // 4. Keyword / Template / Conventional Commits label triage
            if repo_config.labels.enabled {
                let engine = labels::LabelEngine::new(repo_config.labels.clone())?;
                let matched = engine.match_text(&event.issue.title, event.issue.body.as_deref());
                for l in matched {
                    labels_to_add.push(l);
                }
            }

            if !labels_to_add.is_empty() {
                let _ = client
                    .add_labels(owner, repo, event.issue.number, &labels_to_add)
                    .await;
            }

            // 5. CODEOWNERS / Maintainer routing
            if repo_config.codeowners.enabled {
                let codeowners =
                    codeowners::CodeownersEngine::new(repo_config.codeowners.clone(), None);
                let maintainers = codeowners
                    .find_maintainers_for_issue(&event.issue.title, event.issue.body.as_deref());
                if !maintainers.is_empty() {
                    let maintainers_vec: Vec<String> = maintainers.into_iter().collect();
                    if repo_config.codeowners.auto_assign {
                        let _ = client
                            .add_assignees(owner, repo, event.issue.number, &maintainers_vec)
                            .await;

                        let assign_title = format!(
                            "👤 Auto-assigned: {}#{} to {}",
                            full_name,
                            event.issue.number,
                            maintainers_vec.join(", ")
                        );
                        self.notifier
                            .dispatch_event(
                                SugieEventType::Assigned,
                                &assign_title,
                                &event.issue.title,
                                Some(&issue_url),
                                NotificationLevel::Info,
                                vec![DiscordEmbedField {
                                    name: "Assignee(s)".to_string(),
                                    value: maintainers_vec.join(", "),
                                    inline: true,
                                }],
                            )
                            .await;
                    }
                }
            }

            // 6. GitHub Projects v2
            if repo_config.projects.enabled {
                let target_proj = repo_config
                    .projects
                    .issue_project_number
                    .or(repo_config.projects.project_number);
                let projects = projects::ProjectsService::new(repo_config.projects.clone());
                let _ = projects
                    .add_item_to_project(
                        &client,
                        &format!("issue:{}", event.issue.id),
                        None,
                        target_proj,
                        None,
                    )
                    .await;
            }
        }

        Ok(())
    }

    pub async fn handle_pull_request(&self, event: PullRequestWebhookEvent) -> Result<()> {
        let owner = &event.repository.owner.login;
        let repo = &event.repository.name;
        let full_name = &event.repository.full_name;
        let pr_url = format!(
            "https://github.com/{}/{}/pull/{}",
            owner, repo, event.pull_request.number
        );

        let inst_id = match event.installation.as_ref().map(|i| i.id) {
            Some(id) => {
                self.storage
                    .upsert_repository(event.repository.id, full_name, id)?;
                id
            }
            None => self
                .storage
                .get_installation_id_for_repo(full_name)?
                .context("No installation ID for PR event")?,
        };

        let client = self.get_client_for_installation(inst_id).await?;
        let repo_config = self.load_repo_config(&client, owner, repo).await;

        if event.action == "opened" || event.action == "synchronize" {
            let mut labels_to_add = Vec::new();

            // Notify Discord & Zulip on PR opened
            if event.action == "opened" {
                let pr_title_msg = format!(
                    "🔀 New PR: {}#{} by @{}",
                    full_name, event.pull_request.number, event.sender.login
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::PrOpened,
                        &pr_title_msg,
                        &event.pull_request.title,
                        Some(&pr_url),
                        NotificationLevel::Info,
                        vec![
                            DiscordEmbedField {
                                name: "Author".to_string(),
                                value: event.sender.login.clone(),
                                inline: true,
                            },
                            DiscordEmbedField {
                                name: "Pull Request".to_string(),
                                value: format!("#{}", event.pull_request.number),
                                inline: true,
                            },
                        ],
                    )
                    .await;
            }

            // 1. DCO Sign-off validation
            if repo_config.dco.enabled {
                let validator = dco::DcoValidator::new(repo_config.dco.clone());
                let commits = client
                    .list_pr_commits(owner, repo, event.pull_request.number)
                    .await?;
                let dco_res = validator.validate_commits(&commits, &event.pull_request.user.login);

                if !dco_res.is_valid {
                    if let Some(ref missing_lbl) = repo_config.dco.missing_label {
                        labels_to_add.push(missing_lbl.clone());
                    }
                    if event.action == "opened" {
                        let comment = validator.build_failure_comment(&dco_res.unsigned_commits);
                        let _ = client
                            .create_comment(owner, repo, event.pull_request.number, &comment)
                            .await;

                        let title = format!(
                            "⚠️ DCO Check Failed: {}#{}",
                            full_name, event.pull_request.number
                        );
                        let desc = format!(
                            "Commits missing `Signed-off-by` in PR #{}: **{}**",
                            event.pull_request.number, event.pull_request.title
                        );
                        self.notifier
                            .dispatch_event(
                                SugieEventType::DcoFailed,
                                &title,
                                &desc,
                                Some(&pr_url),
                                NotificationLevel::Warning,
                                vec![DiscordEmbedField {
                                    name: "Unsigned Commits".to_string(),
                                    value: dco_res.unsigned_commits.len().to_string(),
                                    inline: true,
                                }],
                            )
                            .await;
                    }
                    if repo_config.dco.set_commit_status {
                        let _ = client
                            .create_commit_status(
                                owner,
                                repo,
                                &event.pull_request.head.sha,
                                "failure",
                                validator.status_context(),
                                "Some commits are missing a valid Signed-off-by trailer",
                                None,
                            )
                            .await;
                    }
                } else {
                    if let Some(ref missing_lbl) = repo_config.dco.missing_label {
                        let _ = client
                            .remove_label(owner, repo, event.pull_request.number, missing_lbl)
                            .await;
                    }
                    if repo_config.dco.set_commit_status {
                        let _ = client
                            .create_commit_status(
                                owner,
                                repo,
                                &event.pull_request.head.sha,
                                "success",
                                validator.status_context(),
                                "All commits are properly signed off",
                                None,
                            )
                            .await;
                    }
                }
            }

            // 2. Path-based labeling, Size-based labeling & Draft status
            let pr_files = client
                .list_pr_files(owner, repo, event.pull_request.number)
                .await?;
            let file_paths: Vec<String> = pr_files.into_iter().map(|f| f.filename).collect();

            if repo_config.labels.enabled {
                let engine = labels::LabelEngine::new(repo_config.labels.clone())?;

                // Path rules
                let path_labels = engine.match_paths(&file_paths);
                for l in path_labels {
                    labels_to_add.push(l);
                }

                // Text / conventional commits rules
                let text_labels = engine.match_text(
                    &event.pull_request.title,
                    event.pull_request.body.as_deref(),
                );
                for l in text_labels {
                    labels_to_add.push(l);
                }

                // Size-based labeling
                let additions = event.pull_request.additions.unwrap_or(0);
                let deletions = event.pull_request.deletions.unwrap_or(0);
                if let Some(size_lbl) = engine.match_pr_size(additions, deletions) {
                    labels_to_add.push(size_lbl);
                }

                // Draft status labeling
                let (draft_add, draft_remove) = engine.draft_label_status(event.pull_request.draft);
                if let Some(d_add) = draft_add {
                    labels_to_add.push(d_add);
                }
                if let Some(d_rem) = draft_remove {
                    let _ = client
                        .remove_label(owner, repo, event.pull_request.number, &d_rem)
                        .await;
                }
            }

            // 3. CODEOWNERS & Reviewer load balancing
            let codeowners_content = client
                .get_repo_file_content(owner, repo, &repo_config.codeowners.file_path, None)
                .await
                .ok()
                .flatten();

            let codeowners_engine = codeowners::CodeownersEngine::new(
                repo_config.codeowners.clone(),
                codeowners_content.as_deref(),
            );
            let codeowner_candidates: Vec<String> = codeowners_engine
                .find_maintainers_for_pr(&file_paths)
                .into_iter()
                .collect();

            if repo_config.reviewers.enabled && event.action == "opened" {
                let balancer = reviewers::ReviewerLoadBalancer::new(
                    repo_config.reviewers.clone(),
                    self.storage.clone(),
                );
                let exclude = vec![event.pull_request.user.login.clone()];

                if let Ok(chosen_reviewers) = balancer.select_reviewers_for_pr(
                    full_name,
                    &file_paths,
                    &codeowner_candidates,
                    &exclude,
                ) {
                    if !chosen_reviewers.is_empty() {
                        let _ = client
                            .request_reviewers(
                                owner,
                                repo,
                                event.pull_request.number,
                                &chosen_reviewers,
                            )
                            .await;
                        for cr in &chosen_reviewers {
                            let _ = balancer.on_review_requested(full_name, cr);
                        }
                        info!(
                            "Assigned reviewers {:?} to {}#{}",
                            chosen_reviewers, full_name, event.pull_request.number
                        );

                        let assign_title = format!(
                            "👥 Reviewers Assigned: {}#{} to {}",
                            full_name,
                            event.pull_request.number,
                            chosen_reviewers.join(", ")
                        );
                        self.notifier
                            .dispatch_event(
                                SugieEventType::Assigned,
                                &assign_title,
                                &event.pull_request.title,
                                Some(&pr_url),
                                NotificationLevel::Info,
                                vec![DiscordEmbedField {
                                    name: "Reviewer(s)".to_string(),
                                    value: chosen_reviewers.join(", "),
                                    inline: true,
                                }],
                            )
                            .await;
                    }
                }
            }

            // 4. First-time contributor
            if event.action == "opened" {
                let first_time = first_time::FirstTimeHandler::new(repo_config.first_time.clone());
                let stats = self
                    .storage
                    .get_contributor_stats(&event.sender.login, full_name)?;
                let has_prior = stats
                    .map(|s| s.opened_issues + s.merged_prs > 0)
                    .unwrap_or(false);

                if first_time
                    .is_first_time(event.pull_request.author_association.as_deref(), has_prior)
                {
                    let welcome = first_time.build_welcome_comment(&event.sender.login, true);
                    let _ = client
                        .create_comment(owner, repo, event.pull_request.number, &welcome)
                        .await;
                    for l in first_time.labels_to_add(true) {
                        labels_to_add.push(l.clone());
                    }
                }
            }

            if !labels_to_add.is_empty() {
                let _ = client
                    .add_labels(owner, repo, event.pull_request.number, &labels_to_add)
                    .await;
            }

            // 5. GitHub Projects v2
            if repo_config.projects.enabled && event.action == "opened" {
                let target_proj = repo_config
                    .projects
                    .pr_project_number
                    .or(repo_config.projects.project_number);
                let projects = projects::ProjectsService::new(repo_config.projects.clone());
                let _ = projects
                    .add_item_to_project(
                        &client,
                        &format!("pr:{}", event.pull_request.id),
                        None,
                        target_proj,
                        None,
                    )
                    .await;
            }
        } else if event.action == "closed" {
            let balancer = reviewers::ReviewerLoadBalancer::new(
                repo_config.reviewers.clone(),
                self.storage.clone(),
            );
            for r in &event.pull_request.requested_reviewers {
                let _ = balancer.on_review_completed_or_closed(full_name, &r.login);
            }

            if event.pull_request.merged {
                let title = format!(
                    "🎉 PR Merged: {}#{} by @{}",
                    full_name, event.pull_request.number, event.pull_request.user.login
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::PrMerged,
                        &title,
                        &event.pull_request.title,
                        Some(&pr_url),
                        NotificationLevel::Success,
                        vec![
                            DiscordEmbedField {
                                name: "Author".to_string(),
                                value: event.pull_request.user.login.clone(),
                                inline: true,
                            },
                            DiscordEmbedField {
                                name: "Merged By".to_string(),
                                value: event.sender.login.clone(),
                                inline: true,
                            },
                        ],
                    )
                    .await;

                let growth = growth::GrowthTracker::new(
                    repo_config.growth.clone(),
                    self.storage.clone(),
                    self.notifier.clone(),
                );
                if let Ok(Some(alert)) = growth
                    .on_pr_merged(
                        &event.pull_request.user.login,
                        full_name,
                        event.pull_request.number,
                        Some(&pr_url),
                    )
                    .await
                {
                    let celebration = growth.build_pr_comment(&alert);
                    let _ = client
                        .create_comment(owner, repo, event.pull_request.number, &celebration)
                        .await;
                }
            }
        }

        Ok(())
    }

    pub async fn handle_issue_comment(&self, event: IssueCommentWebhookEvent) -> Result<()> {
        if event.action != "created" {
            return Ok(());
        }

        if event.sender.r#type.as_deref() == Some("Bot") {
            return Ok(());
        }

        let owner = &event.repository.owner.login;
        let repo = &event.repository.name;
        let full_name = &event.repository.full_name;

        let inst_id = match event.installation.as_ref().map(|i| i.id) {
            Some(id) => {
                self.storage
                    .upsert_repository(event.repository.id, full_name, id)?;
                id
            }
            None => self
                .storage
                .get_installation_id_for_repo(full_name)?
                .context("No installation ID for comment event")?,
        };

        let client = self.get_client_for_installation(inst_id).await?;
        let repo_config = self.load_repo_config(&client, owner, repo).await;

        let commands =
            commands::CommandParser::parse_comment(&event.comment.body, &repo_config.commands);
        if commands.is_empty() {
            return Ok(());
        }

        let projects = projects::ProjectsService::new(repo_config.projects.clone());
        let executor = commands::CommandExecutor::new(&client, &projects, &self.notifier);

        for cmd in &commands {
            if let Err(e) = executor
                .execute(cmd, owner, repo, &event.issue, &event.sender)
                .await
            {
                error!("Error executing slash command {:?}: {:?}", cmd, e);
                let _ = client
                    .create_comment(
                        owner,
                        repo,
                        event.issue.number,
                        &format!("⚠️ Error executing command: `{}`", e),
                    )
                    .await;
            }
        }

        Ok(())
    }

    pub async fn handle_check_suite(&self, event: CheckSuiteWebhookEvent) -> Result<()> {
        if event.action != "completed" {
            return Ok(());
        }

        let owner = &event.repository.owner.login;
        let repo = &event.repository.name;
        let full_name = &event.repository.full_name;

        let inst_id = match event.installation.as_ref().map(|i| i.id) {
            Some(id) => {
                self.storage
                    .upsert_repository(event.repository.id, full_name, id)?;
                id
            }
            None => match self.storage.get_installation_id_for_repo(full_name)? {
                Some(id) => id,
                None => return Ok(()),
            },
        };

        let client = self.get_client_for_installation(inst_id).await?;
        let repo_config = self.load_repo_config(&client, owner, repo).await;

        if !repo_config.ci_summary.enabled {
            return Ok(());
        }

        let check_runs = client
            .list_check_runs_for_ref(owner, repo, &event.check_suite.head_sha)
            .await?;
        let summarizer = ci_summary::CiSummarizer::new(repo_config.ci_summary.clone());

        for pr_min in &event.check_suite.pull_requests {
            if let Err(e) = summarizer
                .update_or_create_summary(&client, owner, repo, pr_min.number, &check_runs)
                .await
            {
                warn!(
                    "Failed to update CI summary on PR #{}: {:?}",
                    pr_min.number, e
                );
            }

            // If CI failed, notify Discord & Zulip if configured
            let has_failure = check_runs.iter().any(|r| {
                matches!(
                    r.conclusion.as_deref(),
                    Some("failure") | Some("timed_out") | Some("action_required")
                )
            });

            if has_failure && repo_config.ci_summary.notify_on_failure {
                let pr_url = format!(
                    "https://github.com/{}/{}/pull/{}",
                    owner, repo, pr_min.number
                );
                let title = format!("❌ CI Failure: {}#{}", full_name, pr_min.number);
                let failed_names: Vec<String> = check_runs
                    .iter()
                    .filter(|r| {
                        matches!(r.conclusion.as_deref(), Some("failure") | Some("timed_out"))
                    })
                    .map(|r| r.name.clone())
                    .collect();

                let desc = format!(
                    "CI checks failed on PR #{}:\n- {}",
                    pr_min.number,
                    failed_names.join("\n- ")
                );
                self.notifier
                    .dispatch_event(
                        SugieEventType::CiFailure,
                        &title,
                        &desc,
                        Some(&pr_url),
                        NotificationLevel::Error,
                        Vec::new(),
                    )
                    .await;
            }
        }

        Ok(())
    }
}

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
use crate::notifications::NotificationDispatcher;
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
                    return self.config.default_rules.merge_with_override(Some(repo_cfg));
                }
                Err(e) => {
                    warn!("Failed to parse .github/sugie.toml in {}/{}: {:?}", owner, repo, e);
                }
            },
            Ok(None) => {}
            Err(e) => {
                warn!("Error fetching .github/sugie.toml from {}/{}: {:?}", owner, repo, e);
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

        let inst_id = match event.installation.as_ref().map(|i| i.id) {
            Some(id) => {
                self.storage.upsert_repository(event.repository.id, full_name, id)?;
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

            // 1. First-time contributor handling
            let first_time = first_time::FirstTimeHandler::new(repo_config.first_time.clone());
            let stats = self.storage.get_contributor_stats(&event.sender.login, full_name)?;
            let has_prior = stats.map(|s| s.opened_issues + s.merged_prs > 0).unwrap_or(false);

            if first_time.is_first_time(event.issue.author_association.as_deref(), has_prior) {
                let welcome = first_time.build_welcome_comment(&event.sender.login);
                let _ = client.create_comment(owner, repo, event.issue.number, &welcome).await;
                for l in first_time.labels_to_add() {
                    labels_to_add.push(l.clone());
                }
            }

            // 2. Duplicate detection
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
                    let dup_comment = detector.build_comment_body(&matches);
                    let _ = client.create_comment(owner, repo, event.issue.number, &dup_comment).await;
                    if let Some(ref dup_lbl) = repo_config.duplicates.add_label {
                        labels_to_add.push(dup_lbl.clone());
                    }
                }
            }

            // 3. Keyword / Template label triage
            if repo_config.labels.enabled {
                let engine = labels::LabelEngine::new(repo_config.labels.clone())?;
                let matched = engine.match_text(&event.issue.title, event.issue.body.as_deref());
                for l in matched {
                    labels_to_add.push(l);
                }
            }

            if !labels_to_add.is_empty() {
                let _ = client.add_labels(owner, repo, event.issue.number, &labels_to_add).await;
            }

            // 4. CODEOWNERS / Maintainer routing
            if repo_config.codeowners.enabled {
                let codeowners = codeowners::CodeownersEngine::new(repo_config.codeowners.clone(), None);
                let maintainers = codeowners.find_maintainers_for_issue(&event.issue.title, event.issue.body.as_deref());
                if !maintainers.is_empty() {
                    let maintainers_vec: Vec<String> = maintainers.into_iter().collect();
                    if repo_config.codeowners.auto_assign {
                        let _ = client.add_assignees(owner, repo, event.issue.number, &maintainers_vec).await;
                    }
                }
            }

            // 5. GitHub Projects v2
            if repo_config.projects.enabled {
                let projects = projects::ProjectsService::new(repo_config.projects.clone());
                let _ = projects
                    .add_item_to_project(
                        &client,
                        &format!("issue:{}", event.issue.id),
                        None,
                        None,
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

        let inst_id = match event.installation.as_ref().map(|i| i.id) {
            Some(id) => {
                self.storage.upsert_repository(event.repository.id, full_name, id)?;
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

            // 1. DCO Sign-off validation
            if repo_config.dco.enabled {
                let validator = dco::DcoValidator::new(repo_config.dco.clone());
                let commits = client.list_pr_commits(owner, repo, event.pull_request.number).await?;
                let dco_res = validator.validate_commits(&commits);

                if !dco_res.is_valid {
                    if let Some(ref missing_lbl) = repo_config.dco.missing_label {
                        labels_to_add.push(missing_lbl.clone());
                    }
                    if event.action == "opened" {
                        let comment = validator.build_failure_comment(&dco_res.unsigned_commits);
                        let _ = client.create_comment(owner, repo, event.pull_request.number, &comment).await;
                    }
                    if repo_config.dco.set_commit_status {
                        let _ = client
                            .create_commit_status(
                                owner,
                                repo,
                                &event.pull_request.head.sha,
                                "failure",
                                "sugie/dco",
                                "Some commits are missing a valid Signed-off-by trailer",
                                None,
                            )
                            .await;
                    }
                } else {
                    if let Some(ref missing_lbl) = repo_config.dco.missing_label {
                        let _ = client.remove_label(owner, repo, event.pull_request.number, missing_lbl).await;
                    }
                    if repo_config.dco.set_commit_status {
                        let _ = client
                            .create_commit_status(
                                owner,
                                repo,
                                &event.pull_request.head.sha,
                                "success",
                                "sugie/dco",
                                "All commits are properly signed off",
                                None,
                            )
                            .await;
                    }
                }
            }

            // 2. Path-based labeling & CODEOWNERS
            let pr_files = client.list_pr_files(owner, repo, event.pull_request.number).await?;
            let file_paths: Vec<String> = pr_files.into_iter().map(|f| f.filename).collect();

            if repo_config.labels.enabled {
                let engine = labels::LabelEngine::new(repo_config.labels.clone())?;
                let path_labels = engine.match_paths(&file_paths);
                for l in path_labels {
                    labels_to_add.push(l);
                }
                let text_labels = engine.match_text(&event.pull_request.title, event.pull_request.body.as_deref());
                for l in text_labels {
                    labels_to_add.push(l);
                }
            }

            // Fetch CODEOWNERS content from repository
            let codeowners_content = client
                .get_repo_file_content(owner, repo, &repo_config.codeowners.file_path, None)
                .await
                .ok()
                .flatten();

            let codeowners_engine = codeowners::CodeownersEngine::new(
                repo_config.codeowners.clone(),
                codeowners_content.as_deref(),
            );
            let codeowner_candidates = codeowners_engine.find_maintainers_for_pr(&file_paths);

            // 3. Reviewer assignment with load balancing
            if repo_config.reviewers.enabled && event.action == "opened" {
                let balancer = reviewers::ReviewerLoadBalancer::new(
                    repo_config.reviewers.clone(),
                    self.storage.clone(),
                );
                let candidates_vec: Vec<String> = codeowner_candidates.into_iter().collect();
                let exclude = vec![event.pull_request.user.login.clone()];

                if let Ok(Some(chosen_reviewer)) =
                    balancer.select_best_reviewer(full_name, &candidates_vec, &exclude)
                {
                    let _ = client
                        .request_reviewers(owner, repo, event.pull_request.number, &[chosen_reviewer.clone()])
                        .await;
                    let _ = balancer.on_review_requested(full_name, &chosen_reviewer);
                    info!(
                        "Assigned reviewer @{} (load balanced) to {}#{}",
                        chosen_reviewer, full_name, event.pull_request.number
                    );
                }
            }

            // 4. First-time contributor
            if event.action == "opened" {
                let first_time = first_time::FirstTimeHandler::new(repo_config.first_time.clone());
                let stats = self.storage.get_contributor_stats(&event.sender.login, full_name)?;
                let has_prior = stats.map(|s| s.opened_issues + s.merged_prs > 0).unwrap_or(false);

                if first_time.is_first_time(event.pull_request.author_association.as_deref(), has_prior) {
                    let welcome = first_time.build_welcome_comment(&event.sender.login);
                    let _ = client.create_comment(owner, repo, event.pull_request.number, &welcome).await;
                    for l in first_time.labels_to_add() {
                        labels_to_add.push(l.clone());
                    }
                }
            }

            if !labels_to_add.is_empty() {
                let _ = client.add_labels(owner, repo, event.pull_request.number, &labels_to_add).await;
            }
        } else if event.action == "closed" {
            // Decrement active workload for reviewers
            let balancer = reviewers::ReviewerLoadBalancer::new(
                repo_config.reviewers.clone(),
                self.storage.clone(),
            );
            for r in &event.pull_request.requested_reviewers {
                let _ = balancer.on_review_completed_or_closed(full_name, &r.login);
            }

            // If merged, track contributor growth
            if event.pull_request.merged {
                let growth = growth::GrowthTracker::new(
                    repo_config.growth.clone(),
                    self.storage.clone(),
                    self.notifier.clone(),
                );
                let pr_url = format!("https://github.com/{}/{}/pull/{}", owner, repo, event.pull_request.number);
                if let Ok(Some(alert)) = growth
                    .on_pr_merged(&event.pull_request.user.login, full_name, event.pull_request.number, Some(&pr_url))
                    .await
                {
                    let celebration = growth.build_pr_comment(&alert);
                    let _ = client.create_comment(owner, repo, event.pull_request.number, &celebration).await;
                }
            }
        }

        Ok(())
    }

    pub async fn handle_issue_comment(&self, event: IssueCommentWebhookEvent) -> Result<()> {
        if event.action != "created" {
            return Ok(());
        }

        // Avoid reacting to bot's own comments
        if event.sender.r#type.as_deref() == Some("Bot") {
            return Ok(());
        }

        let commands = commands::CommandParser::parse_comment(&event.comment.body);
        if commands.is_empty() {
            return Ok(());
        }

        let owner = &event.repository.owner.login;
        let repo = &event.repository.name;
        let full_name = &event.repository.full_name;

        let inst_id = match event.installation.as_ref().map(|i| i.id) {
            Some(id) => {
                self.storage.upsert_repository(event.repository.id, full_name, id)?;
                id
            }
            None => self
                .storage
                .get_installation_id_for_repo(full_name)?
                .context("No installation ID for comment event")?,
        };

        let client = self.get_client_for_installation(inst_id).await?;
        let repo_config = self.load_repo_config(&client, owner, repo).await;
        let projects = projects::ProjectsService::new(repo_config.projects.clone());
        let executor = commands::CommandExecutor::new(&client, &projects);

        for cmd in &commands {
            if let Err(e) = executor.execute(cmd, owner, repo, &event.issue, &event.sender).await {
                error!("Error executing slash command {:?}: {:?}", cmd, e);
                let _ = client.create_comment(
                    owner,
                    repo,
                    event.issue.number,
                    &format!("⚠️ Error executing command: `{}`", e),
                ).await;
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
                self.storage.upsert_repository(event.repository.id, full_name, id)?;
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

        let check_runs = client.list_check_runs_for_ref(owner, repo, &event.check_suite.head_sha).await?;
        let summarizer = ci_summary::CiSummarizer::new(repo_config.ci_summary.clone());

        for pr_min in &event.check_suite.pull_requests {
            if let Err(e) = summarizer
                .update_or_create_summary(&client, owner, repo, pr_min.number, &check_runs)
                .await
            {
                warn!("Failed to update CI summary on PR #{}: {:?}", pr_min.number, e);
            }
        }

        Ok(())
    }
}

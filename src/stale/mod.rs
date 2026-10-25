use crate::config::StaleConfig;
use crate::github::client::GitHubClient;
use crate::notifications::{NotificationDispatcher, NotificationLevel};
use crate::storage::Storage;
use anyhow::Result;
use chrono::{Duration, Utc};
use std::collections::HashSet;
use tracing::info;

pub struct StaleManager {
    config: StaleConfig,
    storage: Storage,
    notifier: NotificationDispatcher,
}

#[derive(Debug, Default)]
pub struct StaleRunReport {
    pub warned_count: u32,
    pub closed_count: u32,
    pub locked_count: u32,
}

impl StaleManager {
    pub fn new(config: StaleConfig, storage: Storage, notifier: NotificationDispatcher) -> Self {
        Self {
            config,
            storage,
            notifier,
        }
    }

    pub async fn process_repository(
        &self,
        client: &GitHubClient,
        owner: &str,
        repo: &str,
    ) -> Result<StaleRunReport> {
        if !self.config.enabled {
            return Ok(StaleRunReport::default());
        }

        let repo_full_name = format!("{}/{}", owner, repo);
        let exempt_set: HashSet<String> = self.config.exempt_labels.iter().cloned().collect();
        let now = Utc::now();
        let warn_threshold = now - Duration::days(self.config.warn_days as i64);
        let close_threshold_duration = Duration::days(self.config.close_days as i64);

        let mut report = StaleRunReport::default();

        // 1. Process open issues and PRs for warning or closing
        let open_items = client.list_repo_issues(owner, repo, "open", 100).await?;

        for item in open_items {
            let has_exempt_label = item.labels.iter().any(|l| exempt_set.contains(&l.name));
            if has_exempt_label {
                continue;
            }

            let is_marked_stale = item.labels.iter().any(|l| l.name == self.config.stale_label);

            if is_marked_stale {
                // Check if it's time to close
                let warned_at = self
                    .storage
                    .get_stale_warned(&repo_full_name, item.number)?
                    .unwrap_or(item.updated_at);

                if now - warned_at >= close_threshold_duration {
                    // Stage 2: Close stale item
                    client
                        .create_comment(owner, repo, item.number, &self.config.close_message)
                        .await?;
                    client
                        .update_issue_state(owner, repo, item.number, "closed", Some("not_planned"))
                        .await?;

                    report.closed_count += 1;
                    info!("Closed stale issue/PR {}#{}", repo_full_name, item.number);

                    let title = format!("⏳ Stale Item Closed: {}#{}", repo_full_name, item.number);
                    self.notifier
                        .dispatch(
                            &title,
                            &format!("Closed inactive item: **{}**", item.title),
                            None,
                            NotificationLevel::Warning,
                            Vec::new(),
                        )
                        .await;
                }
            } else if item.updated_at < warn_threshold {
                // Stage 1: Mark as stale & warn
                client
                    .add_labels(owner, repo, item.number, &[self.config.stale_label.clone()])
                    .await?;
                client
                    .create_comment(owner, repo, item.number, &self.config.warn_message)
                    .await?;

                self.storage.record_stale_warned(&repo_full_name, item.number)?;

                report.warned_count += 1;
                info!("Marked issue/PR {}#{} as stale", repo_full_name, item.number);

                let title = format!("⚠️ Stale Warning: {}#{}", repo_full_name, item.number);
                self.notifier
                    .dispatch(
                        &title,
                        &format!("Item marked stale due to inactivity: **{}**", item.title),
                        None,
                        NotificationLevel::Info,
                        Vec::new(),
                    )
                    .await;
            }
        }

        // 2. Process Stage 3: Lock long-closed stale items if lock_days is configured
        if let Some(lock_days) = self.config.lock_days {
            let lock_threshold = now - Duration::days(lock_days as i64);
            let closed_items = client.list_repo_issues(owner, repo, "closed", 50).await?;

            for item in closed_items {
                let is_marked_stale = item.labels.iter().any(|l| l.name == self.config.stale_label);
                if is_marked_stale {
                    if let Some(closed_at) = item.closed_at {
                        if closed_at < lock_threshold {
                            let _ = client.lock_issue(owner, repo, item.number, Some("resolved")).await;
                            let _ = self.storage.clear_stale_warned(&repo_full_name, item.number);
                            report.locked_count += 1;
                            info!("Locked closed stale issue/PR {}#{}", repo_full_name, item.number);
                        }
                    }
                }
            }
        }

        Ok(report)
    }
}

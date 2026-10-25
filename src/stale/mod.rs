use crate::config::{interpolate, StaleConfig};
use crate::github::client::GitHubClient;
use crate::notifications::{DiscordEmbedField, NotificationDispatcher, NotificationLevel, SugieEventType};
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
        let exempt_authors: HashSet<String> = self.config.exempt_authors.iter().cloned().collect();
        let now = Utc::now();
        let warn_threshold = now - Duration::days(self.config.warn_days as i64);
        let close_threshold_duration = Duration::days(self.config.close_days as i64);

        let mut report = StaleRunReport::default();

        let open_items = client.list_repo_issues(owner, repo, "open", 100).await?;

        for item in open_items {
            let is_pr = item.is_pr();
            if is_pr && !self.config.enable_prs {
                continue;
            }
            if !is_pr && !self.config.enable_issues {
                continue;
            }

            if exempt_authors.contains(&item.user.login) {
                continue;
            }

            let has_exempt_label = item.labels.iter().any(|l| exempt_set.contains(&l.name));
            if has_exempt_label {
                continue;
            }

            let is_marked_stale = item.labels.iter().any(|l| l.name == self.config.stale_label);
            let item_url = format!("https://github.com/{}/{}/issues/{}", owner, repo, item.number);
            let days_inactive = (now - item.updated_at).num_days().to_string();
            let warn_days_str = self.config.warn_days.to_string();
            let close_days_str = self.config.close_days.to_string();

            let vars = [
                ("author", item.user.login.as_str()),
                ("number", &item.number.to_string()),
                ("title", item.title.as_str()),
                ("days", &days_inactive),
                ("warn_days", &warn_days_str),
                ("close_days", &close_days_str),
                ("stale_label", &self.config.stale_label),
            ];

            if is_marked_stale {
                let warned_at = self
                    .storage
                    .get_stale_warned(&repo_full_name, item.number)?
                    .unwrap_or(item.updated_at);

                if now - warned_at >= close_threshold_duration {
                    let close_body = interpolate(&self.config.close_message, &vars);
                    client.create_comment(owner, repo, item.number, &close_body).await?;

                    if let Some(ref cl_lbl) = self.config.closed_label {
                        let _ = client.add_labels(owner, repo, item.number, &[cl_lbl.clone()]).await;
                    }

                    client
                        .update_issue_state(owner, repo, item.number, "closed", Some("not_planned"))
                        .await?;

                    report.closed_count += 1;
                    info!("Closed stale issue/PR {}#{}", repo_full_name, item.number);

                    let title = format!("⏳ Stale Item Closed: {}#{}", repo_full_name, item.number);
                    let desc = format!("Automatically closed due to inactivity:\n\n**{}**\n\nInactive for {} days.", item.title, days_inactive);
                    self.notifier
                        .dispatch_event(
                            SugieEventType::StaleClosed,
                            &title,
                            &desc,
                            Some(&item_url),
                            NotificationLevel::Warning,
                            vec![DiscordEmbedField {
                                name: "Inactive Duration".to_string(),
                                value: format!("{} days", days_inactive),
                                inline: true,
                            }],
                        )
                        .await;
                }
            } else if item.updated_at < warn_threshold {
                let warn_body = interpolate(&self.config.warn_message, &vars);
                client
                    .add_labels(owner, repo, item.number, &[self.config.stale_label.clone()])
                    .await?;
                client
                    .create_comment(owner, repo, item.number, &warn_body)
                    .await?;

                self.storage.record_stale_warned(&repo_full_name, item.number)?;

                report.warned_count += 1;
                info!("Marked issue/PR {}#{} as stale", repo_full_name, item.number);

                let title = format!("⚠️ Marked as Stale: {}#{}", repo_full_name, item.number);
                let desc = format!("Marked as stale due to {} days without updates:\n\n**{}**", days_inactive, item.title);
                self.notifier
                    .dispatch_event(
                        SugieEventType::StaleWarned,
                        &title,
                        &desc,
                        Some(&item_url),
                        NotificationLevel::Info,
                        vec![
                            DiscordEmbedField {
                                name: "Inactivity".to_string(),
                                value: format!("{} days", days_inactive),
                                inline: true,
                            },
                            DiscordEmbedField {
                                name: "Will close in".to_string(),
                                value: format!("{} days", self.config.close_days),
                                inline: true,
                            },
                        ],
                    )
                    .await;
            }
        }

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

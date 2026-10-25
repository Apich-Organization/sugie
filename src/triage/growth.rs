use crate::config::GrowthConfig;
use crate::notifications::{NotificationDispatcher, NotificationLevel};
use crate::storage::Storage;
use anyhow::Result;

pub struct GrowthTracker {
    config: GrowthConfig,
    storage: Storage,
    notifier: NotificationDispatcher,
}

#[derive(Debug, Clone)]
pub struct MilestoneAlert {
    pub username: String,
    pub repo: String,
    pub merged_prs: u32,
    pub message: String,
    pub recommend_role: Option<String>,
}

impl GrowthTracker {
    pub fn new(config: GrowthConfig, storage: Storage, notifier: NotificationDispatcher) -> Self {
        Self {
            config,
            storage,
            notifier,
        }
    }

    pub async fn on_pr_merged(
        &self,
        username: &str,
        repo: &str,
        pr_number: u64,
        pr_url: Option<&str>,
    ) -> Result<Option<MilestoneAlert>> {
        if !self.config.enabled {
            return Ok(None);
        }

        let count = self.storage.record_merged_pr(username, repo)?;
        let stats = self.storage.get_contributor_stats(username, repo)?;
        let last_milestone = stats.map(|s| s.last_milestone_notified).unwrap_or(0);

        // Find applicable milestone that has not been notified yet
        let mut triggered_milestone = None;
        for ms in &self.config.thresholds {
            if count >= ms.merged_prs && ms.merged_prs > last_milestone {
                triggered_milestone = Some(ms);
            }
        }

        if let Some(milestone) = triggered_milestone {
            self.storage
                .update_last_milestone_notified(username, repo, milestone.merged_prs)?;

            let alert = MilestoneAlert {
                username: username.to_string(),
                repo: repo.to_string(),
                merged_prs: milestone.merged_prs,
                message: milestone.message.clone(),
                recommend_role: milestone.recommend_role.clone(),
            };

            // Notify channels if configured
            if milestone.notify_channels {
                let title = format!(
                    "🌟 Contributor Growth Alert: @{} ({} PRs)",
                    username, milestone.merged_prs
                );
                let desc = format!(
                    "Contributor **@{}** has merged **{}** pull requests in `{}`!\n\n**Note**: {}\n\nPR #{}",
                    username, milestone.merged_prs, repo, milestone.message, pr_number
                );

                self.notifier
                    .dispatch(
                        &title,
                        &desc,
                        pr_url,
                        NotificationLevel::Success,
                        Vec::new(),
                    )
                    .await;
            }

            return Ok(Some(alert));
        }

        Ok(None)
    }

    pub fn build_pr_comment(&self, alert: &MilestoneAlert) -> String {
        format!(
            "### 🌟 Contributor Milestone Reached!\n\n\
            Congratulations @{}! You have successfully merged **{}** pull requests into this repository.\n\n\
            {}\n\n\
            *Thank you for being an active part of our open source community!*",
            alert.username, alert.merged_prs, alert.message
        )
    }
}

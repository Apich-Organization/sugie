use crate::config::CiSummaryConfig;
use crate::github::client::GitHubClient;
use crate::github::models::CheckRun;
use anyhow::Result;

pub struct CiSummarizer {
    config: CiSummaryConfig,
}

impl CiSummarizer {
    pub fn new(config: CiSummaryConfig) -> Self {
        Self { config }
    }

    pub fn should_summarize(&self, runs: &[CheckRun]) -> bool {
        if !self.config.enabled {
            return false;
        }

        // Check if there is any failure or if all runs have finished
        let has_failure = runs.iter().any(|r| {
            matches!(
                r.conclusion.as_deref(),
                Some("failure") | Some("timed_out") | Some("action_required")
            )
        });

        if self.config.comment_on_failure {
            has_failure
        } else {
            true
        }
    }

    pub fn build_summary_markdown(&self, runs: &[CheckRun]) -> String {
        let mut md = String::from("<!-- sugie:ci-summary -->\n");
        md.push_str("### 🚦 CI Results Summary\n\n");

        let total = runs.len();
        let passed = runs.iter().filter(|r| r.conclusion.as_deref() == Some("success")).count();
        let failed = runs.iter().filter(|r| {
            matches!(
                r.conclusion.as_deref(),
                Some("failure") | Some("timed_out") | Some("action_required")
            )
        }).count();
        let pending = total - passed - failed;

        md.push_str(&format!(
            "**Status**: {} Passed, {} Failed, {} In Progress\n\n",
            passed, failed, pending
        ));

        md.push_str("| Check / Job | Status | Conclusion | Details |\n");
        md.push_str("| :--- | :---: | :---: | :--- |\n");

        for r in runs {
            let status_icon = match r.status.as_deref() {
                Some("completed") => "🏁 Completed",
                Some("in_progress") => "⏳ In Progress",
                Some("queued") => "💤 Queued",
                _ => "❓ Unknown",
            };

            let conclusion_icon = match r.conclusion.as_deref() {
                Some("success") => "✅ Success",
                Some("failure") => "❌ Failure",
                Some("timed_out") => "⏰ Timed Out",
                Some("cancelled") => "🚫 Cancelled",
                Some("skipped") => "⏭️ Skipped",
                Some("neutral") => "⚪ Neutral",
                _ => "⏳ Pending",
            };

            let details_link = if let Some(ref url) = r.html_url {
                format!("[View Logs]({})", url)
            } else {
                "-".to_string()
            };

            let summary_note = if let Some(ref output) = r.output {
                if let Some(ref title) = output.title {
                    format!(" ({})", title)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            md.push_str(&format!(
                "| **{}**{} | {} | {} | {} |\n",
                r.name, summary_note, status_icon, conclusion_icon, details_link
            ));
        }

        md
    }

    pub async fn update_or_create_summary(
        &self,
        client: &GitHubClient,
        owner: &str,
        repo: &str,
        pr_number: u64,
        runs: &[CheckRun],
    ) -> Result<()> {
        if !self.should_summarize(runs) {
            return Ok(());
        }

        let summary_text = self.build_summary_markdown(runs);

        if self.config.sticky {
            let comments = client.list_comments(owner, repo, pr_number).await?;
            for comment in comments {
                if comment.body.contains("<!-- sugie:ci-summary -->") {
                    // Update existing comment
                    client.update_comment(owner, repo, comment.id, &summary_text).await?;
                    return Ok(());
                }
            }
        }

        // Otherwise post a new comment
        client.create_comment(owner, repo, pr_number, &summary_text).await?;
        Ok(())
    }
}

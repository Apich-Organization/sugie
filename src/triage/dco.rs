use crate::config::DcoConfig;
use crate::github::models::CommitItem;
use regex::Regex;

#[derive(Debug, Clone)]
pub struct DcoValidationResult {
    pub is_valid: bool,
    pub unsigned_commits: Vec<(String, String)>, // (sha, message_summary)
}

pub struct DcoValidator {
    config: DcoConfig,
    regex: Regex,
}

impl DcoValidator {
    pub fn new(config: DcoConfig) -> Self {
        // Standard DCO format: Signed-off-by: Real Name <email@example.com>
        let regex = Regex::new(r"(?m)^Signed-off-by:\s+([^<]+)\s+<([^>]+)>").unwrap();
        Self { config, regex }
    }

    pub fn validate_commits(&self, commits: &[CommitItem]) -> DcoValidationResult {
        if !self.config.enabled || !self.config.require_signoff {
            return DcoValidationResult {
                is_valid: true,
                unsigned_commits: Vec::new(),
            };
        }

        let mut unsigned_commits = Vec::new();

        for commit_item in commits {
            let msg = &commit_item.commit.message;
            if !self.regex.is_match(msg) {
                let first_line = msg.lines().next().unwrap_or("No commit message").to_string();
                let short_sha = commit_item.sha.chars().take(7).collect::<String>();
                unsigned_commits.push((short_sha, first_line));
            }
        }

        let is_valid = unsigned_commits.is_empty();
        DcoValidationResult {
            is_valid,
            unsigned_commits,
        }
    }

    pub fn build_failure_comment(&self, unsigned_commits: &[(String, String)]) -> String {
        let mut comment = String::from("### ⚠️ Developer Certificate of Origin (DCO) Sign-off Missing\n\n");
        comment.push_str("To maintain open-source compliance, every commit in this PR must include a `Signed-off-by` trailer.\n\n");
        comment.push_str("The following commit(s) are missing a valid sign-off:\n");

        for (sha, title) in unsigned_commits {
            comment.push_str(&format!("- `{}`: {}\n", sha, title));
        }

        comment.push_str("\n#### How to fix:\n");
        comment.push_str("You can sign off your previous commits using git:\n");
        comment.push_str("```bash\n# If only your latest commit is unsigned:\ngit commit --amend --signoff\n\n# If multiple commits need sign-off:\ngit rebase --signoff HEAD~N\n\n# Force push with lease to update the PR:\ngit push --force-with-lease\n```\n");

        comment
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::models::CommitDetails;

    #[test]
    fn test_dco_validation() {
        let validator = DcoValidator::new(DcoConfig::default());

        let signed_commit = CommitItem {
            sha: "1234567890abcdef".to_string(),
            commit: CommitDetails {
                author: None,
                committer: None,
                message: "feat: add rust triage bot\n\nSigned-off-by: Sugie Bot <bot@sugie.rs>".to_string(),
            },
            author: None,
        };

        let unsigned_commit = CommitItem {
            sha: "abcdef1234567890".to_string(),
            commit: CommitDetails {
                author: None,
                committer: None,
                message: "feat: forgotten sign off".to_string(),
            },
            author: None,
        };

        let res1 = validator.validate_commits(&[signed_commit.clone()]);
        assert!(res1.is_valid);
        assert!(res1.unsigned_commits.is_empty());

        let res2 = validator.validate_commits(&[signed_commit, unsigned_commit]);
        assert!(!res2.is_valid);
        assert_eq!(res2.unsigned_commits.len(), 1);
        assert_eq!(res2.unsigned_commits[0].0, "abcdef1");
    }
}

use crate::config::DcoConfig;
use crate::github::models::CommitItem;
use regex::Regex;

#[derive(Debug, Clone)]
pub struct DcoValidationResult {
    pub is_valid: bool,
    pub unsigned_commits: Vec<(String, String)>,
}

pub struct DcoValidator {
    config: DcoConfig,
    regex: Regex,
}

impl DcoValidator {
    pub fn new(config: DcoConfig) -> Self {
        let regex = Regex::new(r"(?m)^Signed-off-by:\s+([^<]+)\s+<([^>]+)>").unwrap();
        Self { config, regex }
    }

    pub fn is_author_exempt(&self, author: &str) -> bool {
        self.config
            .exempt_authors
            .iter()
            .any(|ea| ea.eq_ignore_ascii_case(author))
    }

    pub fn status_context(&self) -> &str {
        &self.config.status_context
    }

    pub fn validate_commits(&self, commits: &[CommitItem], author: &str) -> DcoValidationResult {
        if !self.config.enabled || !self.config.require_signoff || self.is_author_exempt(author) {
            return DcoValidationResult {
                is_valid: true,
                unsigned_commits: Vec::new(),
            };
        }

        let mut unsigned_commits = Vec::new();

        for commit_item in commits {
            let msg = &commit_item.commit.message;
            if !self.regex.is_match(msg) {
                let first_line = msg
                    .lines()
                    .next()
                    .unwrap_or("No commit message")
                    .to_string();
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
        if let Some(ref custom_tmpl) = self.config.failure_comment {
            return custom_tmpl.clone();
        }

        let mut comment =
            String::from("### ⚠️ Developer Certificate of Origin (DCO) Sign-off Missing\n\n");
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
    fn test_dco_exempt_author() {
        let mut config = DcoConfig::default();
        config.exempt_authors = vec!["dependabot[bot]".to_string()];
        let validator = DcoValidator::new(config);

        let unsigned_commit = CommitItem {
            sha: "12345678".to_string(),
            commit: CommitDetails {
                author: None,
                committer: None,
                message: "chore(deps): bump serde from 1.0 to 1.1".to_string(),
            },
            author: None,
        };

        let res = validator.validate_commits(&[unsigned_commit], "dependabot[bot]");
        assert!(res.is_valid);
    }
}

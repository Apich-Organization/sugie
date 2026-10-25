use crate::config::{interpolate, FirstTimeConfig};

pub struct FirstTimeHandler {
    config: FirstTimeConfig,
}

impl FirstTimeHandler {
    pub fn new(config: FirstTimeConfig) -> Self {
        Self { config }
    }

    pub fn is_first_time(
        &self,
        author_association: Option<&str>,
        has_prior_contributions: bool,
    ) -> bool {
        if !self.config.enabled {
            return false;
        }

        if let Some(assoc) = author_association {
            if assoc == "FIRST_TIME_CONTRIBUTOR" || assoc == "FIRST_TIMER" {
                return true;
            }
        }

        !has_prior_contributions
    }

    pub fn build_welcome_comment(&self, username: &str, is_pr: bool) -> String {
        let tmpl = if is_pr {
            &self.config.welcome_pr_comment
        } else {
            &self.config.welcome_issue_comment
        };

        interpolate(tmpl, &[("author", username)])
    }

    pub fn labels_to_add(&self, is_pr: bool) -> &[String] {
        if is_pr {
            &self.config.pr_labels
        } else {
            &self.config.issue_labels
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_time_detection() {
        let handler = FirstTimeHandler::new(FirstTimeConfig::default());
        assert!(handler.is_first_time(Some("FIRST_TIME_CONTRIBUTOR"), false));
        assert!(handler.is_first_time(Some("FIRST_TIMER"), false));
        assert!(handler.is_first_time(None, false));
        assert!(!handler.is_first_time(Some("MEMBER"), true));
    }

    #[test]
    fn test_welcome_comment_interpolation() {
        let handler = FirstTimeHandler::new(FirstTimeConfig::default());
        let comment = handler.build_welcome_comment("ferris", false);
        assert!(comment.contains("@ferris"));
    }
}

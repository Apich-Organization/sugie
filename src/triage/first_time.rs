use crate::config::FirstTimeConfig;

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

    pub fn build_welcome_comment(&self, username: &str) -> String {
        let base = &self.config.welcome_comment;
        format!("👋 Welcome @{}, thank you for your contribution!\n\n{}", username, base)
    }

    pub fn labels_to_add(&self) -> &[String] {
        &self.config.labels
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
        assert!(!handler.is_first_time(Some("CONTRIBUTOR"), true));
    }
}

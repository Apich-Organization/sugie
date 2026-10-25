use crate::config::LabelsConfig;
use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use std::collections::HashSet;

pub struct LabelEngine {
    config: LabelsConfig,
    path_matchers: Vec<(GlobSet, Vec<String>)>,
}

impl LabelEngine {
    pub fn new(config: LabelsConfig) -> Result<Self> {
        let mut path_matchers = Vec::new();
        for rule in &config.path_rules {
            let mut builder = GlobSetBuilder::new();
            let glob = Glob::new(&rule.pattern)?;
            builder.add(glob);
            let set = builder.build()?;
            path_matchers.push((set, rule.labels.clone()));
        }

        Ok(Self {
            config,
            path_matchers,
        })
    }

    pub fn match_paths(&self, file_paths: &[String]) -> HashSet<String> {
        let mut matched_labels = HashSet::new();
        if !self.config.enabled {
            return matched_labels;
        }

        for path in file_paths {
            for (glob_set, labels) in &self.path_matchers {
                if glob_set.is_match(path) {
                    for label in labels {
                        matched_labels.insert(label.clone());
                    }
                }
            }
        }

        matched_labels
    }

    pub fn match_text(&self, title: &str, body: Option<&str>) -> HashSet<String> {
        let mut matched_labels = HashSet::new();
        if !self.config.enabled {
            return matched_labels;
        }

        let body_str = body.unwrap_or_default();
        let title_lower = title.to_lowercase();
        let body_lower = body_str.to_lowercase();

        // 1. Keyword / Regex rules
        for rule in &self.config.keyword_rules {
            let matches_target = match rule.target.as_str() {
                "title" => Self::check_keywords_and_regex(&rule.keywords, &rule.regex, &title_lower, title),
                "body" => Self::check_keywords_and_regex(&rule.keywords, &rule.regex, &body_lower, body_str),
                _ => {
                    // "both"
                    Self::check_keywords_and_regex(&rule.keywords, &rule.regex, &title_lower, title)
                        || Self::check_keywords_and_regex(&rule.keywords, &rule.regex, &body_lower, body_str)
                }
            };

            if matches_target {
                for label in &rule.labels {
                    matched_labels.insert(label.clone());
                }
            }
        }

        // 2. Template word / pattern rules
        for rule in &self.config.template_rules {
            if body_lower.contains(&rule.pattern.to_lowercase()) {
                for label in &rule.labels {
                    matched_labels.insert(label.clone());
                }
            }
        }

        matched_labels
    }

    fn check_keywords_and_regex(
        keywords: &[String],
        regex_opt: &Option<String>,
        lower_text: &str,
        raw_text: &str,
    ) -> bool {
        // Keyword check
        for kw in keywords {
            if lower_text.contains(&kw.to_lowercase()) {
                return true;
            }
        }

        // Regex check
        if let Some(pattern) = regex_opt {
            if let Ok(re) = Regex::new(pattern) {
                if re.is_match(raw_text) {
                    return true;
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{KeywordLabelRule, PathLabelRule, TemplateLabelRule};

    #[test]
    fn test_label_path_matching() {
        let config = LabelsConfig {
            enabled: true,
            path_rules: vec![
                PathLabelRule {
                    pattern: "crates/core/**".to_string(),
                    labels: vec!["area-core".to_string()],
                },
                PathLabelRule {
                    pattern: "**/*.md".to_string(),
                    labels: vec!["area-docs".to_string()],
                },
            ],
            keyword_rules: vec![],
            template_rules: vec![],
        };

        let engine = LabelEngine::new(config).unwrap();
        let paths = vec![
            "crates/core/src/lib.rs".to_string(),
            "docs/README.md".to_string(),
        ];
        let labels = engine.match_paths(&paths);
        assert!(labels.contains("area-core"));
        assert!(labels.contains("area-docs"));
    }

    #[test]
    fn test_label_keyword_and_template_matching() {
        let config = LabelsConfig {
            enabled: true,
            path_rules: vec![],
            keyword_rules: vec![
                KeywordLabelRule {
                    keywords: vec!["crash".to_string(), "panic".to_string()],
                    regex: None,
                    target: "title".to_string(),
                    labels: vec!["bug".to_string(), "severity-high".to_string()],
                },
                KeywordLabelRule {
                    keywords: vec![],
                    regex: Some(r"(?i)feature\s+request".to_string()),
                    target: "both".to_string(),
                    labels: vec!["enhancement".to_string()],
                },
            ],
            template_rules: vec![TemplateLabelRule {
                pattern: "### Steps to Reproduce".to_string(),
                labels: vec!["has-repro".to_string()],
            }],
        };

        let engine = LabelEngine::new(config).unwrap();
        let labels = engine.match_text(
            "Compiler panic on syntax error",
            Some("Here is what happened:\n### Steps to Reproduce\n1. Run cargo build"),
        );

        assert!(labels.contains("bug"));
        assert!(labels.contains("severity-high"));
        assert!(labels.contains("has-repro"));
    }
}

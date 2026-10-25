use crate::config::LabelsConfig;
use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use std::collections::HashSet;

pub struct LabelEngine {
    config: LabelsConfig,
    path_matchers: Vec<(GlobSet, Option<GlobSet>, Vec<String>)>,
}

impl LabelEngine {
    pub fn new(config: LabelsConfig) -> Result<Self> {
        let mut path_matchers = Vec::new();
        for rule in &config.path_rules {
            let mut include_builder = GlobSetBuilder::new();
            include_builder.add(Glob::new(&rule.pattern)?);
            let include_set = include_builder.build()?;

            let ignore_set = if !rule.ignore_patterns.is_empty() {
                let mut ignore_builder = GlobSetBuilder::new();
                for ignore_pat in &rule.ignore_patterns {
                    ignore_builder.add(Glob::new(ignore_pat)?);
                }
                Some(ignore_builder.build()?)
            } else {
                None
            };

            path_matchers.push((include_set, ignore_set, rule.labels.clone()));
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
            for (include_set, ignore_set, labels) in &self.path_matchers {
                if include_set.is_match(path) {
                    let is_ignored = ignore_set
                        .as_ref()
                        .map(|s| s.is_match(path))
                        .unwrap_or(false);
                    if !is_ignored {
                        for label in labels {
                            matched_labels.insert(label.clone());
                        }
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
                "title" => {
                    Self::check_keywords_and_regex(&rule.keywords, &rule.regex, &title_lower, title)
                }
                "body" => Self::check_keywords_and_regex(
                    &rule.keywords,
                    &rule.regex,
                    &body_lower,
                    body_str,
                ),
                _ => {
                    Self::check_keywords_and_regex(&rule.keywords, &rule.regex, &title_lower, title)
                        || Self::check_keywords_and_regex(
                            &rule.keywords,
                            &rule.regex,
                            &body_lower,
                            body_str,
                        )
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

        // 3. Conventional commits / title prefix mapping
        if self.config.conventional_commits.enabled {
            for (prefix, label) in &self.config.conventional_commits.mappings {
                let p1 = format!("{}:", prefix.to_lowercase());
                let p2 = format!("{}(", prefix.to_lowercase());
                if title_lower.starts_with(&p1) || title_lower.starts_with(&p2) {
                    matched_labels.insert(label.clone());
                }
            }
        }

        matched_labels
    }

    pub fn match_pr_size(&self, additions: u64, deletions: u64) -> Option<String> {
        if !self.config.size_rules.enabled || self.config.size_rules.labels.len() < 5 {
            return None;
        }

        let total_changes = additions + deletions;
        let cfg = &self.config.size_rules;

        let label = if total_changes <= cfg.xs_max {
            &cfg.labels[0]
        } else if total_changes <= cfg.s_max {
            &cfg.labels[1]
        } else if total_changes <= cfg.m_max {
            &cfg.labels[2]
        } else if total_changes <= cfg.l_max {
            &cfg.labels[3]
        } else {
            &cfg.labels[4]
        };

        Some(label.clone())
    }

    pub fn draft_label_status(&self, is_draft: bool) -> (Option<String>, Option<String>) {
        if !self.config.draft_rules.enabled {
            return (None, None);
        }

        if is_draft {
            (Some(self.config.draft_rules.draft_label.clone()), None)
        } else if self.config.draft_rules.remove_on_ready {
            (None, Some(self.config.draft_rules.draft_label.clone()))
        } else {
            (None, None)
        }
    }

    fn check_keywords_and_regex(
        keywords: &[String],
        regex_opt: &Option<String>,
        lower_text: &str,
        raw_text: &str,
    ) -> bool {
        for kw in keywords {
            if lower_text.contains(&kw.to_lowercase()) {
                return true;
            }
        }

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
    use crate::config::{ConventionalCommitsConfig, PathLabelRule, SizeLabelConfig};
    use std::collections::HashMap;

    #[test]
    fn test_label_path_matching_with_ignore() {
        let config = LabelsConfig {
            enabled: true,
            path_rules: vec![PathLabelRule {
                pattern: "crates/core/**".to_string(),
                ignore_patterns: vec!["crates/core/**/*.md".to_string()],
                labels: vec!["area-core".to_string()],
            }],
            keyword_rules: vec![],
            template_rules: vec![],
            size_rules: SizeLabelConfig::default(),
            draft_rules: Default::default(),
            conventional_commits: Default::default(),
        };

        let engine = LabelEngine::new(config).unwrap();
        let paths = vec![
            "crates/core/src/lib.rs".to_string(),
            "crates/core/README.md".to_string(),
        ];
        let labels = engine.match_paths(&paths);
        assert!(labels.contains("area-core"));
    }

    #[test]
    fn test_conventional_commits_and_size() {
        let mut mappings = HashMap::new();
        mappings.insert("feat".to_string(), "enhancement".to_string());
        mappings.insert("fix".to_string(), "bug".to_string());

        let mut config = LabelsConfig::default();
        config.conventional_commits = ConventionalCommitsConfig {
            enabled: true,
            mappings,
        };
        config.size_rules = SizeLabelConfig {
            enabled: true,
            xs_max: 10,
            s_max: 50,
            m_max: 200,
            l_max: 500,
            labels: vec![
                "size/XS".to_string(),
                "size/S".to_string(),
                "size/M".to_string(),
                "size/L".to_string(),
                "size/XL".to_string(),
            ],
        };

        let engine = LabelEngine::new(config).unwrap();
        let labels = engine.match_text("feat(parser): add unicode support", None);
        assert!(labels.contains("enhancement"));

        let size_label = engine.match_pr_size(15, 10).unwrap();
        assert_eq!(size_label, "size/S");
    }
}

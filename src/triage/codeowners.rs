use crate::config::CodeownersConfig;
use globset::{Glob, GlobMatcher};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct CodeownerEntry {
    pub pattern: String,
    pub matcher: GlobMatcher,
    pub owners: Vec<String>,
}

#[derive(Clone)]
pub struct CodeownersEngine {
    config: CodeownersConfig,
    entries: Vec<CodeownerEntry>,
}

impl CodeownersEngine {
    pub fn new(config: CodeownersConfig, codeowners_content: Option<&str>) -> Self {
        let mut entries = Vec::new();

        if let Some(content) = codeowners_content {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }

                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() < 2 {
                    continue;
                }

                let pattern = parts[0];
                let owners: Vec<String> = parts[1..]
                    .iter()
                    .map(|o| o.trim_start_matches('@').to_string())
                    .collect();

                // Format glob pattern for matching
                let normalized_glob = if pattern == "*" {
                    "**".to_string()
                } else if pattern.starts_with('/') {
                    let p = pattern.trim_start_matches('/');
                    if p.ends_with('/') {
                        format!("{}**", p)
                    } else {
                        p.to_string()
                    }
                } else if pattern.ends_with('/') {
                    format!("**/{}**", pattern)
                } else {
                    format!("**/{}", pattern)
                };

                if let Ok(glob) = Glob::new(&normalized_glob) {
                    entries.push(CodeownerEntry {
                        pattern: pattern.to_string(),
                        matcher: glob.compile_matcher(),
                        owners,
                    });
                }
            }
        }

        Self { config, entries }
    }

    pub fn find_maintainers_for_pr(&self, modified_files: &[String]) -> HashSet<String> {
        let mut maintainers = HashSet::new();
        if !self.config.enabled {
            return maintainers;
        }

        // 1. From CODEOWNERS file
        for file in modified_files {
            // Later rules in CODEOWNERS take precedence per file
            let mut matched_for_file: Option<&Vec<String>> = None;
            for entry in &self.entries {
                if entry.matcher.is_match(file) {
                    matched_for_file = Some(&entry.owners);
                }
            }

            if let Some(owners) = matched_for_file {
                for owner in owners {
                    maintainers.insert(owner.clone());
                }
            }
        }

        // 2. From TOML custom rules (path patterns)
        for rule in &self.config.rules {
            if let Some(ref path_pat) = rule.path_pattern {
                if let Ok(glob) = Glob::new(path_pat) {
                    let matcher = glob.compile_matcher();
                    for file in modified_files {
                        if matcher.is_match(file) {
                            for m in &rule.maintainers {
                                maintainers.insert(m.trim_start_matches('@').to_string());
                            }
                        }
                    }
                }
            }
        }

        maintainers
    }

    pub fn find_maintainers_for_issue(&self, title: &str, body: Option<&str>) -> HashSet<String> {
        let mut maintainers = HashSet::new();
        if !self.config.enabled {
            return maintainers;
        }

        let combined = format!("{} {}", title, body.unwrap_or_default()).to_lowercase();

        // Match from TOML keyword rules
        for rule in &self.config.rules {
            if let Some(ref kws) = rule.keywords {
                for kw in kws {
                    if combined.contains(&kw.to_lowercase()) {
                        for m in &rule.maintainers {
                            maintainers.insert(m.trim_start_matches('@').to_string());
                        }
                    }
                }
            }
        }

        maintainers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CodeownerRule;

    #[test]
    fn test_codeowners_parsing_and_matching() {
        let content = r#"
        # Global default
        * @global-team

        # Specific paths
        /crates/core/ @core-dev @alice
        *.rs @rust-dev
        /docs/ @doc-team
        "#;

        let config = CodeownersConfig {
            enabled: true,
            file_path: ".github/CODEOWNERS".to_string(),
            auto_assign: true,
            mention_in_comment: false,
            rules: vec![CodeownerRule {
                path_pattern: Some("benchmarks/**".to_string()),
                keywords: None,
                maintainers: vec!["@bench-guru".to_string()],
            }],
        };

        let engine = CodeownersEngine::new(config, Some(content));

        let files = vec![
            "crates/core/src/lib.rs".to_string(),
            "benchmarks/bench1.rs".to_string(),
        ];

        let maintainers = engine.find_maintainers_for_pr(&files);
        assert!(maintainers.contains("core-dev") || maintainers.contains("rust-dev"));
        assert!(maintainers.contains("bench-guru"));
    }
}

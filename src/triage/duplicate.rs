use crate::config::DuplicateConfig;
use crate::github::models::Issue;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct DuplicateMatch {
    pub issue_number: u64,
    pub title: String,
    pub similarity: f64,
}

pub struct DuplicateDetector {
    config: DuplicateConfig,
}

impl DuplicateDetector {
    pub fn new(config: DuplicateConfig) -> Self {
        Self { config }
    }

    pub fn find_duplicates(
        &self,
        current_number: u64,
        title: &str,
        body: Option<&str>,
        existing_issues: &[Issue],
    ) -> Vec<DuplicateMatch> {
        if !self.config.enabled || existing_issues.is_empty() {
            return Vec::new();
        }

        let mut matches = Vec::new();

        for candidate in existing_issues {
            if candidate.number == current_number {
                continue;
            }

            let sim = calculate_issue_similarity(
                title,
                body.unwrap_or_default(),
                &candidate.title,
                candidate.body.as_deref().unwrap_or_default(),
            );

            if sim >= self.config.threshold {
                matches.push(DuplicateMatch {
                    issue_number: candidate.number,
                    title: candidate.title.clone(),
                    similarity: sim,
                });
            }
        }

        matches.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap());
        matches
    }

    pub fn build_comment_body(&self, matches: &[DuplicateMatch]) -> String {
        if let Some(ref custom_tmpl) = self.config.comment_template {
            return custom_tmpl.clone();
        }

        let mut comment = String::from("### 🔍 Potential Duplicate Detected\n\n");
        comment.push_str("Sugie analyzed this issue and detected high similarity with existing issues:\n\n");

        for m in matches.iter().take(3) {
            comment.push_str(&format!(
                "- **#{}** - *{}* (Similarity: **{:.1}%**)\n",
                m.issue_number,
                m.title,
                m.similarity * 100.0
            ));
        }

        comment.push_str("\nIf this is indeed the same problem, please consider joining the discussion on the existing issue.");
        comment
    }
}

pub fn calculate_issue_similarity(
    title_a: &str,
    body_a: &str,
    title_b: &str,
    body_b: &str,
) -> f64 {
    // 1. Title similarity
    let title_sim = strsim::jaro_winkler(&title_a.to_lowercase(), &title_b.to_lowercase());

    // 2. Token Jaccard similarity of normalized text
    let tokens_a = tokenize(title_a, body_a);
    let tokens_b = tokenize(title_b, body_b);

    let jaccard_sim = if tokens_a.is_empty() || tokens_b.is_empty() {
        0.0
    } else {
        let intersection = tokens_a.intersection(&tokens_b).count();
        let union = tokens_a.union(&tokens_b).count();
        if union == 0 {
            0.0
        } else {
            intersection as f64 / union as f64
        }
    };

    // Weighted similarity: Title has higher weight in triage
    0.55 * title_sim + 0.45 * jaccard_sim
}

fn tokenize(title: &str, body: &str) -> HashSet<String> {
    let combined = format!("{} {}", title, strip_code_blocks(body));
    let stop_words: HashSet<&'static str> = [
        "the", "a", "an", "is", "in", "it", "to", "for", "and", "or", "of", "with",
        "on", "at", "by", "from", "up", "about", "into", "over", "after", "this",
        "that", "these", "those", "how", "what", "when", "where", "why", "who", "i",
        "you", "we", "they", "me", "my", "not", "no", "can", "could", "should",
    ]
    .iter()
    .cloned()
    .collect();

    combined
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|token| token.len() >= 3 && !stop_words.contains(token))
        .map(|s| s.to_string())
        .collect()
}

fn strip_code_blocks(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut in_code_fence = false;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if !in_code_fence {
            clean.push_str(line);
            clean.push(' ');
        }
    }

    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similarity_calculation() {
        let title1 = "Crash on startup when configuration file is missing";
        let body1 = "The application panics with unexpected None when loading config";

        let title2 = "Crash on startup if config file is not found";
        let body2 = "Panic occurs with None when config file cannot be loaded";

        let sim = calculate_issue_similarity(title1, body1, title2, body2);
        assert!(sim > 0.60, "Expected high similarity, got {}", sim);

        let title3 = "Add documentation for zulip webhooks";
        let body3 = "We need docs showing how to setup zulip notifications";
        let diff_sim = calculate_issue_similarity(title1, body1, title3, body3);
        assert!(diff_sim < 0.35, "Expected low similarity, got {}", diff_sim);
    }
}

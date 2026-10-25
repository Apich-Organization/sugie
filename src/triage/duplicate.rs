use crate::config::{interpolate, DuplicateConfig};
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

    pub fn should_auto_close(&self, matches: &[DuplicateMatch]) -> bool {
        if let Some(close_thresh) = self.config.auto_close_threshold {
            if let Some(top) = matches.first() {
                return top.similarity >= close_thresh;
            }
        }
        false
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
        let limit = self.config.max_search_candidates as usize;

        for candidate in existing_issues.iter().take(limit) {
            if candidate.number == current_number {
                continue;
            }

            let sim = match self.config.algorithm.as_str() {
                "jaro_winkler" => {
                    strsim::jaro_winkler(&title.to_lowercase(), &candidate.title.to_lowercase())
                }
                "jaccard" => {
                    let tokens_a = tokenize(title, body.unwrap_or_default());
                    let tokens_b = tokenize(
                        &candidate.title,
                        candidate.body.as_deref().unwrap_or_default(),
                    );
                    if tokens_a.is_empty() || tokens_b.is_empty() {
                        0.0
                    } else {
                        tokens_a.intersection(&tokens_b).count() as f64
                            / tokens_a.union(&tokens_b).count() as f64
                    }
                }
                _ => calculate_issue_similarity(
                    title,
                    body.unwrap_or_default(),
                    &candidate.title,
                    candidate.body.as_deref().unwrap_or_default(),
                ),
            };

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

    pub fn build_comment_body(
        &self,
        author: &str,
        current_number: u64,
        matches: &[DuplicateMatch],
        is_closing: bool,
    ) -> String {
        let top = match matches.first() {
            Some(t) => t,
            None => return String::new(),
        };

        let sim_percent = format!("{:.1}%", top.similarity * 100.0);
        let curr_num_str = current_number.to_string();
        let top_num_str = top.issue_number.to_string();

        if let Some(ref tmpl) = self.config.comment_template {
            return interpolate(
                tmpl,
                &[
                    ("author", author),
                    ("current_number", &curr_num_str),
                    ("matched_number", &top_num_str),
                    ("matched_title", &top.title),
                    ("similarity", &sim_percent),
                ],
            );
        }

        let mut comment = String::from("### 🔍 Potential Duplicate Detected\n\n");
        if is_closing {
            comment.push_str(&format!(
                "Sugie detected that this issue is an exact or near duplicate of **#{}** (*{}*) with **{}** confidence.\n\n\
                Closing this issue automatically. Please continue any discussion on #{}!\n\n",
                top.issue_number, top.title, sim_percent, top.issue_number
            ));
        } else {
            comment.push_str("Sugie detected high similarity with existing issue(s):\n\n");
            for m in matches.iter().take(3) {
                comment.push_str(&format!(
                    "- **#{}** - *{}* (Similarity: **{:.1}%**)\n",
                    m.issue_number,
                    m.title,
                    m.similarity * 100.0
                ));
            }
            comment.push_str("\nIf your problem is identical, please check the existing thread to avoid splitting discussions.");
        }

        comment
    }
}

pub fn calculate_issue_similarity(title_a: &str, body_a: &str, title_b: &str, body_b: &str) -> f64 {
    let title_sim = strsim::jaro_winkler(&title_a.to_lowercase(), &title_b.to_lowercase());

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

    0.55 * title_sim + 0.45 * jaccard_sim
}

fn tokenize(title: &str, body: &str) -> HashSet<String> {
    let combined = format!("{} {}", title, strip_code_blocks(body));
    let stop_words: HashSet<&'static str> = [
        "the", "a", "an", "is", "in", "it", "to", "for", "and", "or", "of", "with", "on", "at",
        "by", "from", "up", "about", "into", "over", "after", "this", "that", "these", "those",
        "how", "what", "when", "where", "why", "who", "i", "you", "we", "they", "me", "my", "not",
        "no", "can", "could", "should",
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

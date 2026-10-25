use crate::config::ReviewersConfig;
use crate::storage::Storage;
use anyhow::Result;
use globset::Glob;
use std::collections::HashSet;

pub struct ReviewerLoadBalancer {
    config: ReviewersConfig,
    storage: Storage,
}

impl ReviewerLoadBalancer {
    pub fn new(config: ReviewersConfig, storage: Storage) -> Self {
        Self { config, storage }
    }

    pub fn select_reviewers_for_pr(
        &self,
        repo: &str,
        file_paths: &[String],
        codeowner_candidates: &[String],
        exclude_users: &[String],
    ) -> Result<Vec<String>> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        let mut candidate_pool = HashSet::new();

        // 1. From component pools matching modified file paths
        for (pattern, pool) in &self.config.component_pools {
            if let Ok(glob) = Glob::new(pattern) {
                let matcher = glob.compile_matcher();
                if file_paths.iter().any(|f| matcher.is_match(f)) {
                    for member in pool {
                        candidate_pool.insert(member.clone());
                    }
                }
            }
        }

        // 2. From CODEOWNERS
        for co in codeowner_candidates {
            candidate_pool.insert(co.clone());
        }

        // 3. Fallback to default reviewer pool if empty
        if candidate_pool.is_empty() {
            for m in &self.config.pool {
                candidate_pool.insert(m.clone());
            }
        }

        let pool_vec: Vec<String> = candidate_pool.into_iter().collect();
        let target_count = self.config.reviewers_per_pr.max(1);
        self.select_best_reviewers(repo, &pool_vec, exclude_users, target_count)
    }

    pub fn select_best_reviewers(
        &self,
        repo: &str,
        candidate_pool: &[String],
        exclude_users: &[String],
        count: usize,
    ) -> Result<Vec<String>> {
        let excluded: HashSet<&str> = exclude_users.iter().map(|s| s.as_str()).collect();
        let ooo: HashSet<&str> = self
            .config
            .out_of_office
            .iter()
            .map(|s| s.as_str())
            .collect();

        let pool = if !candidate_pool.is_empty() {
            candidate_pool
        } else {
            &self.config.pool
        };

        let mut available_candidates = Vec::new();
        for candidate in pool {
            let handle = candidate.trim_start_matches('@');
            if !excluded.contains(handle) && !ooo.contains(handle) {
                let workload = self.storage.get_reviewer_workload(handle, repo)?;
                available_candidates.push((handle.to_string(), workload));
            }
        }

        if available_candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Sort by workload ascending (least busy first)
        available_candidates.sort_by_key(|(_, workload)| *workload);

        let selected: Vec<String> = available_candidates
            .into_iter()
            .take(count)
            .map(|(handle, _)| handle)
            .collect();

        Ok(selected)
    }

    pub fn on_review_requested(&self, repo: &str, reviewer: &str) -> Result<()> {
        let handle = reviewer.trim_start_matches('@');
        self.storage.increment_reviewer_workload(handle, repo)
    }

    pub fn on_review_completed_or_closed(&self, repo: &str, reviewer: &str) -> Result<()> {
        let handle = reviewer.trim_start_matches('@');
        self.storage.decrement_reviewer_workload(handle, repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reviewer_selection_least_busy() {
        let storage = Storage::new_in_memory().unwrap();
        let config = ReviewersConfig {
            enabled: true,
            pool: vec!["alice".to_string(), "bob".to_string(), "carol".to_string()],
            reviewers_per_pr: 1,
            max_active_reviews: 5,
            load_balancing: "least_busy".to_string(),
            out_of_office: vec!["carol".to_string()],
            component_pools: Default::default(),
        };

        let balancer = ReviewerLoadBalancer::new(config, storage.clone());
        let repo = "rust-lang/cargo";

        storage.increment_reviewer_workload("alice", repo).unwrap();

        let chosen = balancer
            .select_best_reviewers(repo, &[], &["pr_author".to_string()], 1)
            .unwrap();
        assert_eq!(chosen, vec!["bob".to_string()]);
    }
}

use crate::config::ReviewersConfig;
use crate::storage::Storage;
use anyhow::Result;
use std::collections::HashSet;

pub struct ReviewerLoadBalancer {
    config: ReviewersConfig,
    storage: Storage,
}

impl ReviewerLoadBalancer {
    pub fn new(config: ReviewersConfig, storage: Storage) -> Self {
        Self { config, storage }
    }

    pub fn select_best_reviewer(
        &self,
        repo: &str,
        candidate_pool: &[String],
        exclude_users: &[String],
    ) -> Result<Option<String>> {
        let excluded: HashSet<&str> = exclude_users.iter().map(|s| s.as_str()).collect();
        let ooo: HashSet<&str> = self.config.out_of_office.iter().map(|s| s.as_str()).collect();

        // Effective candidate pool: candidates or default pool
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
            return Ok(None);
        }

        // Sort by workload ascending (least busy first)
        available_candidates.sort_by_key(|(_, workload)| *workload);

        let (best_candidate, best_workload) = &available_candidates[0];

        // Check against max_active_reviews
        if self.config.max_active_reviews > 0 && *best_workload >= self.config.max_active_reviews {
            tracing::warn!(
                "Least busy reviewer {} is at or above max capacity ({}/{})",
                best_candidate,
                best_workload,
                self.config.max_active_reviews
            );
        }

        Ok(Some(best_candidate.clone()))
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
    fn test_reviewer_selection() {
        let storage = Storage::new_in_memory().unwrap();
        let config = ReviewersConfig {
            enabled: true,
            pool: vec!["alice".to_string(), "bob".to_string(), "carol".to_string()],
            max_active_reviews: 5,
            load_balancing: "least_busy".to_string(),
            out_of_office: vec!["carol".to_string()],
        };

        let balancer = ReviewerLoadBalancer::new(config, storage.clone());
        let repo = "rust-lang/cargo";

        // Preload alice with 2 reviews, bob with 0
        storage.increment_reviewer_workload("alice", repo).unwrap();
        storage.increment_reviewer_workload("alice", repo).unwrap();

        let chosen = balancer
            .select_best_reviewer(repo, &[], &["pr_author".to_string()])
            .unwrap();
        // bob should be chosen because carol is OOO and alice has 2 reviews
        assert_eq!(chosen.as_deref(), Some("bob"));

        balancer.on_review_requested(repo, "bob").unwrap();
        balancer.on_review_requested(repo, "bob").unwrap();
        balancer.on_review_requested(repo, "bob").unwrap();

        // Now alice has 2, bob has 3
        let next_chosen = balancer
            .select_best_reviewer(repo, &[], &["pr_author".to_string()])
            .unwrap();
        assert_eq!(next_chosen.as_deref(), Some("alice"));
    }
}

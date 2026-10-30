//! Repository statistics pulled from the GitHub API and cached briefly so that
//! a page refresh storm cannot burn through the installation's rate limit.

use crate::triage::TriageService;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, Serialize)]
pub struct RepoStats {
    pub full_name: String,
    pub open_issues: u64,
    pub open_prs: u64,
    pub commits_30d: u64,
    pub merged_30d: u64,
    pub stars: u64,
    pub forks: u64,
    pub default_branch: String,
    pub fetched_at: String,
    pub error: Option<String>,
}

impl RepoStats {
    pub fn failed(full_name: &str, err: impl ToString) -> Self {
        Self {
            full_name: full_name.to_string(),
            fetched_at: chrono::Utc::now().format("%H:%M:%S").to_string(),
            error: Some(err.to_string()),
            ..Default::default()
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

#[derive(Default)]
pub struct StatsCache {
    inner: Mutex<HashMap<String, (Instant, RepoStats)>>,
}

impl StatsCache {
    pub fn peek(&self, key: &str) -> Option<RepoStats> {
        let guard = self.inner.lock().ok()?;
        let (at, stats) = guard.get(key)?;
        if at.elapsed() < TTL {
            Some(stats.clone())
        } else {
            None
        }
    }

    pub fn put(&self, stats: RepoStats) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(stats.full_name.clone(), (Instant::now(), stats));
        }
    }
}

pub async fn fetch(
    triage: &TriageService,
    cache: &StatsCache,
    full_name: &str,
    force: bool,
) -> RepoStats {
    if !force {
        if let Some(hit) = cache.peek(full_name) {
            return hit;
        }
    }

    let stats = fetch_uncached(triage, full_name).await;
    cache.put(stats.clone());
    stats
}

async fn fetch_uncached(triage: &TriageService, full_name: &str) -> RepoStats {
    let Some((owner, repo)) = full_name.split_once('/') else {
        return RepoStats::failed(full_name, "Repository name must be in owner/repo format");
    };

    let client = match triage.get_client_for_repo(full_name).await {
        Ok(c) => c,
        Err(e) => {
            return RepoStats::failed(full_name, format!("Failed to get installation token: {e}"))
        }
    };

    let since = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    let meta_fut = client.get_repo(owner, repo);
    let issues_fut = client.list_repo_issues(owner, repo, "open", 100);
    let pulls_fut = client.list_repo_pulls(owner, repo, "open", 100);
    let commits_fut = client.list_commits(owner, repo, Some(&since), 100);

    let (meta, issues, pulls, commits) = tokio::join!(meta_fut, issues_fut, pulls_fut, commits_fut);

    let mut stats = RepoStats {
        full_name: full_name.to_string(),
        fetched_at: chrono::Utc::now().format("%H:%M:%S").to_string(),
        ..Default::default()
    };

    match meta {
        Ok(v) => {
            stats.stars = v
                .get("stargazers_count")
                .and_then(|n| n.as_u64())
                .unwrap_or(0);
            stats.forks = v.get("forks_count").and_then(|n| n.as_u64()).unwrap_or(0);
            stats.default_branch = v
                .get("default_branch")
                .and_then(|n| n.as_str())
                .unwrap_or("main")
                .to_string();
        }
        Err(e) => stats.error = Some(format!("Failed to fetch repository metadata: {e}")),
    }

    match issues {
        Ok(list) => stats.open_issues = list.iter().filter(|i| !i.is_pr()).count() as u64,
        Err(e) => {
            stats
                .error
                .get_or_insert(format!("Failed to fetch issue count: {e}"));
        }
    }

    match pulls {
        Ok(list) => stats.open_prs = list.len() as u64,
        Err(e) => {
            stats
                .error
                .get_or_insert(format!("Failed to fetch PR count: {e}"));
        }
    }

    match commits {
        Ok(list) => stats.commits_30d = list.len() as u64,
        Err(e) => {
            stats
                .error
                .get_or_insert(format!("Failed to fetch commit count: {e}"));
        }
    }

    if let Ok(closed) = client.list_repo_pulls(owner, repo, "closed", 50).await {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
        stats.merged_30d = closed
            .iter()
            .filter(|p| p.merged_at.map(|t| t > cutoff).unwrap_or(false))
            .count() as u64;
    }

    stats
}

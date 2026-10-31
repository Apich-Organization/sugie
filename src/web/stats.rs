//! Repository statistics pulled from the GitHub API and cached briefly so that
//! a page refresh storm cannot burn through the installation's rate limit.

use crate::triage::TriageService;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
    /// One async mutex per repository: concurrent callers for the same repo wait
    /// for a single upstream fetch instead of stampeding GitHub when the TTL
    /// expires (thundering herd).
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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

    fn flight(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut guard = match self.flights.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
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

    // Single-flight: only one task fetches per repository; the rest re-check the
    // cache after the winner has populated it.
    let gate = cache.flight(full_name);
    let _permit = gate.lock().await;
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
    let since_date = (chrono::Utc::now() - chrono::Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();

    let meta_fut = client.get_repo(owner, repo);
    let issues_fut = client.count_open_issues_and_prs(owner, repo);
    let pulls_fut = client.count_open_pulls(owner, repo);
    let commits_fut = client.count_commits_since(owner, repo, &since);
    let merged_fut = client.count_merged_since(owner, repo, &since_date);

    let (meta, issues_total, pulls, commits, merged) =
        tokio::join!(meta_fut, issues_fut, pulls_fut, commits_fut, merged_fut);

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

    // `/issues` counts issues *and* PRs, so subtract the PR total to get issues.
    match (issues_total, pulls) {
        (Ok(total), Ok(prs)) => {
            stats.open_prs = prs;
            stats.open_issues = total.saturating_sub(prs);
        }
        (Err(e), _) => {
            stats
                .error
                .get_or_insert(format!("Failed to fetch issue count: {e}"));
        }
        (_, Err(e)) => {
            stats
                .error
                .get_or_insert(format!("Failed to fetch PR count: {e}"));
        }
    }

    match commits {
        Ok(n) => stats.commits_30d = n,
        Err(e) => {
            stats
                .error
                .get_or_insert(format!("Failed to fetch commit count: {e}"));
        }
    }

    match merged {
        Ok(n) => stats.merged_30d = n,
        Err(e) => {
            stats
                .error
                .get_or_insert(format!("Failed to fetch merged count: {e}"));
        }
    }

    stats
}

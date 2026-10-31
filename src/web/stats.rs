//! Repository statistics pulled from the GitHub API and cached briefly so that
//! a page refresh storm cannot burn through the installation's rate limit.

use crate::triage::TriageService;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
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
    /// Repositories with a background refresh already in flight.
    refreshing: Mutex<HashSet<String>>,
}

impl StatsCache {
    pub fn peek(&self, key: &str) -> Option<RepoStats> {
        // Poison-safe: a panic elsewhere must not disable the cache forever.
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (at, stats) = guard.get(key)?;
        if at.elapsed() < TTL {
            Some(stats.clone())
        } else {
            None
        }
    }

    /// Cached stats for `key` with their age, even when stale. Used to render a page
    /// without blocking on GitHub; the caller kicks off a background refresh.
    pub fn get_any(&self, key: &str) -> Option<(RepoStats, Duration)> {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (at, stats) = guard.get(key)?;
        Some((stats.clone(), at.elapsed()))
    }

    pub fn put(&self, stats: RepoStats) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(stats.full_name.clone(), (Instant::now(), stats));
    }

    fn flight(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut guard = self
            .flights
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Returns true when the caller should spawn a refresh: at most one background
    /// refresh per repository is in flight at a time.
    pub fn claim_refresh(&self, key: &str) -> bool {
        let mut guard = self
            .refreshing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(key.to_string())
    }

    fn release_refresh(&self, key: &str) {
        let mut guard = self
            .refreshing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(key);
    }
}

/// Holds a background-refresh claim and releases it on drop, so a panicking task
/// cannot leave the repository permanently unrefreshable.
pub struct RefreshClaim {
    cache: Arc<StatsCache>,
    key: String,
}

impl RefreshClaim {
    pub fn new(cache: Arc<StatsCache>, key: &str) -> Self {
        Self {
            cache,
            key: key.to_string(),
        }
    }
}

impl Drop for RefreshClaim {
    fn drop(&mut self) {
        self.cache.release_refresh(&self.key);
    }
}

/// Releases a single-flight gate and drops its map entry once nobody else is
/// waiting, so the map cannot grow with arbitrary repository names.
struct FlightGuard<'a> {
    cache: &'a StatsCache,
    key: String,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        let mut flights = self
            .cache
            .flights
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Two strong refs means "map entry + this guard": no other waiter is
        // holding a clone. Checked under the lock so a new caller cannot slip in
        // between the count and the removal.
        if Arc::strong_count(&self.gate) == 2 {
            flights.remove(&self.key);
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

    // Single-flight: only one task fetches per repository; the rest re-check the
    // cache after the winner has populated it.
    let flight = FlightGuard {
        cache,
        key: full_name.to_string(),
        gate: cache.flight(full_name),
    };
    let _permit = flight.gate.lock().await;
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

use super::models::{
    CheckRun, Comment, CommitItem, Issue, Label, Milestone, PullRequest, PullRequestFile, User,
};
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;
use serde_json::json;

/// Percent-encode a single path segment. Values such as label names are free-form
/// user text; without this a `/` or `..` in a label would rewrite the request path
/// while still carrying the installation token.
fn seg(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

/// A commit status to publish; see [`GitHubClient::create_commit_status`].
pub struct CommitStatus<'a> {
    pub sha: &'a str,
    /// `error`, `failure`, `pending` or `success`.
    pub state: &'a str,
    pub context: &'a str,
    pub description: &'a str,
    pub target_url: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct CheckRunsResponse {
    check_runs: Vec<CheckRun>,
}

#[derive(Debug, Deserialize)]
struct ContentFileResponse {
    content: String,
    encoding: String,
}

/// One process-wide HTTP client. Building a `reqwest::Client` per call creates a
/// fresh connection pool, so every GitHub request would pay a new TLS handshake
/// and nothing would ever be reused — which also risks secondary rate limits.
/// The auth header is therefore attached per request instead of via
/// `default_headers`.
fn shared_http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(20))
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()
            .unwrap_or_default()
    })
}

impl GitHubClient {
    pub fn new(token: String, base_url: Option<String>) -> Self {
        Self {
            http: shared_http().clone(),
            base_url: base_url.unwrap_or_else(|| "https://api.github.com".to_string()),
            token,
        }
    }

    /// Headers that used to live on the client; now applied per request so the
    /// connection pool can be shared across installations.
    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(mut auth_header) = HeaderValue::from_str(&format!("Bearer {}", self.token)) {
            auth_header.set_sensitive(true);
            headers.insert(AUTHORIZATION, auth_header);
        }
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("sugie-bot/0.1.0"));
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
        headers
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    async fn send_logged(
        &self,
        method: reqwest::Method,
        url: &str,
        payload: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let start = std::time::Instant::now();
        let detail = payload.map(|p| p.to_string()).unwrap_or_default();
        let mut req = self
            .http
            .request(method.clone(), url)
            .headers(self.auth_headers());
        if let Some(body) = payload {
            req = req.json(body);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let ms = start.elapsed().as_millis() as u64;
                crate::security_log::log_github_request(method.as_str(), url, status, ms, &detail);
                Ok(resp)
            }
            Err(e) => {
                let ms = start.elapsed().as_millis() as u64;
                crate::security_log::log_github_request(
                    method.as_str(),
                    url,
                    0,
                    ms,
                    &format!("error={e} {detail}"),
                );
                Err(e.into())
            }
        }
    }

    // --- Issues API ---

    pub async fn get_user_repo_permission(
        &self,
        owner: &str,
        repo: &str,
        username: &str,
    ) -> Result<String> {
        let url = format!(
            "{}/repos/{}/{}/collaborators/{}/permission",
            self.base_url,
            seg(owner),
            seg(repo),
            seg(username)
        );
        let resp = self.send_logged(reqwest::Method::GET, &url, None).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok("none".to_string());
        }
        let resp = resp.error_for_status()?;
        let val: serde_json::Value = resp.json().await?;
        let perm = val
            .get("permission")
            .and_then(|v| v.as_str())
            .unwrap_or("none");
        Ok(perm.to_string())
    }

    pub async fn get_issue(&self, owner: &str, repo: &str, issue_number: u64) -> Result<Issue> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            self.base_url, owner, repo, issue_number
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let issue = resp.json::<Issue>().await?;
        Ok(issue)
    }

    pub async fn create_issue(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: Option<&str>,
        labels: &[String],
        assignees: &[String],
    ) -> Result<Issue> {
        let url = format!("{}/repos/{}/{}/issues", self.base_url, owner, repo);
        let mut payload = json!({
            "title": title,
        });
        if let Some(b) = body {
            payload["body"] = json!(b);
        }
        if !labels.is_empty() {
            payload["labels"] = json!(labels);
        }
        if !assignees.is_empty() {
            payload["assignees"] = json!(assignees);
        }

        let resp = self
            .send_logged(reqwest::Method::POST, &url, Some(&payload))
            .await?
            .error_for_status()?;
        let issue = resp.json::<Issue>().await?;
        Ok(issue)
    }

    pub async fn update_issue_state(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        state: &str,                // "open" or "closed"
        state_reason: Option<&str>, // "completed", "not_planned", "reopened"
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            self.base_url, owner, repo, issue_number
        );
        let mut payload = json!({ "state": state });
        if let Some(reason) = state_reason {
            payload["state_reason"] = json!(reason);
        }
        self.send_logged(reqwest::Method::PATCH, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn create_comment(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        body: &str,
    ) -> Result<Comment> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments",
            self.base_url, owner, repo, issue_number
        );
        let payload = json!({ "body": body });
        let resp = self
            .send_logged(reqwest::Method::POST, &url, Some(&payload))
            .await?
            .error_for_status()?;
        let comment = resp.json::<Comment>().await?;
        Ok(comment)
    }

    pub async fn update_comment(
        &self,
        owner: &str,
        repo: &str,
        comment_id: u64,
        body: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/comments/{}",
            self.base_url, owner, repo, comment_id
        );
        let payload = json!({ "body": body });
        self.send_logged(reqwest::Method::PATCH, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn list_comments(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
    ) -> Result<Vec<Comment>> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments?per_page=100",
            self.base_url, owner, repo, issue_number
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let comments = resp.json::<Vec<Comment>>().await?;
        Ok(comments)
    }

    pub async fn add_labels(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        labels: &[String],
    ) -> Result<()> {
        if labels.is_empty() {
            return Ok(());
        }
        let url = format!(
            "{}/repos/{}/{}/issues/{}/labels",
            self.base_url, owner, repo, issue_number
        );
        let payload = json!({ "labels": labels });
        self.send_logged(reqwest::Method::POST, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn remove_label(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        label: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/labels/{}",
            self.base_url,
            seg(owner),
            seg(repo),
            issue_number,
            seg(label)
        );
        let _ = self.send_logged(reqwest::Method::DELETE, &url, None).await;
        Ok(())
    }

    // --- Exact counters (avoid the 100-item page ceiling) ---

    /// Follow the `Link: rel="last"` header of a `per_page=1` request to obtain
    /// the true number of items. Listing 100 items and counting them saturates at
    /// 100 and silently under-reports busy repositories.
    async fn total_count(&self, path_and_query: &str) -> Result<u64> {
        let url = format!("{}{}", self.base_url, path_and_query);
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let link = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        if let Some(page) = link.as_deref().and_then(last_page) {
            return Ok(page);
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(body.as_array().map(|a| a.len() as u64).unwrap_or(0))
    }

    /// Open issues **and** PRs — the issues endpoint returns both.
    pub async fn count_open_issues_and_prs(&self, owner: &str, repo: &str) -> Result<u64> {
        self.total_count(&format!(
            "/repos/{}/{}/issues?state=open&per_page=1",
            seg(owner),
            seg(repo)
        ))
        .await
    }

    pub async fn count_open_pulls(&self, owner: &str, repo: &str) -> Result<u64> {
        self.total_count(&format!(
            "/repos/{}/{}/pulls?state=open&per_page=1",
            seg(owner),
            seg(repo)
        ))
        .await
    }

    pub async fn count_commits_since(&self, owner: &str, repo: &str, since: &str) -> Result<u64> {
        self.total_count(&format!(
            "/repos/{}/{}/commits?since={}&per_page=1",
            seg(owner),
            seg(repo),
            seg(since)
        ))
        .await
    }

    /// Merged PRs since `since_date` (`YYYY-MM-DD`) via the search API, which
    /// returns an exact `total_count` instead of a truncated list.
    pub async fn count_merged_since(
        &self,
        owner: &str,
        repo: &str,
        since_date: &str,
    ) -> Result<u64> {
        let q = format!(
            "repo:{}/{}+is:pr+is:merged+merged:>={}",
            owner, repo, since_date
        );
        let url = format!("{}/search/issues?q={}&per_page=1", self.base_url, q);
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let body: serde_json::Value = resp.json().await?;
        Ok(body
            .get("total_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0))
    }

    pub async fn add_assignees(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        assignees: &[String],
    ) -> Result<()> {
        if assignees.is_empty() {
            return Ok(());
        }
        let url = format!(
            "{}/repos/{}/{}/issues/{}/assignees",
            self.base_url, owner, repo, issue_number
        );
        let payload = json!({ "assignees": assignees });
        self.send_logged(reqwest::Method::POST, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn remove_assignees(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        assignees: &[String],
    ) -> Result<()> {
        if assignees.is_empty() {
            return Ok(());
        }
        let url = format!(
            "{}/repos/{}/{}/issues/{}/assignees",
            self.base_url, owner, repo, issue_number
        );
        let payload = json!({ "assignees": assignees });
        self.send_logged(reqwest::Method::DELETE, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn list_assignees(&self, owner: &str, repo: &str) -> Result<Vec<User>> {
        let url = format!(
            "{}/repos/{}/{}/assignees?per_page=100",
            self.base_url, owner, repo
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let users = resp.json::<Vec<User>>().await?;
        Ok(users)
    }

    pub async fn lock_issue(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        lock_reason: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/lock",
            self.base_url, owner, repo, issue_number
        );
        let mut payload = json!({});
        if let Some(r) = lock_reason {
            payload["lock_reason"] = json!(r);
        }
        self.send_logged(reqwest::Method::PUT, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Unlock a conversation that `lock_issue` closed.
    pub async fn unlock_issue(&self, owner: &str, repo: &str, issue_number: u64) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}/lock",
            self.base_url, owner, repo, issue_number
        );
        self.send_logged(reqwest::Method::DELETE, &url, None)
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Rename an issue or PR. Titles are the one field operators fix most often
    /// during triage (typos, missing component prefixes).
    pub async fn set_issue_title(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        title: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            self.base_url, owner, repo, issue_number
        );
        let payload = json!({ "title": title });
        self.send_logged(reqwest::Method::PATCH, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Assign a milestone by number, or clear it with `None` (`"milestone": null`).
    pub async fn set_issue_milestone(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        milestone: Option<u64>,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            self.base_url, owner, repo, issue_number
        );
        let payload = json!({ "milestone": milestone });
        self.send_logged(reqwest::Method::PATCH, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Repository labels, so `/label` can be typed without guessing exact names.
    pub async fn list_labels(&self, owner: &str, repo: &str) -> Result<Vec<Label>> {
        let url = format!(
            "{}/repos/{}/{}/labels?per_page=100",
            self.base_url, owner, repo
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        Ok(resp.json::<Vec<Label>>().await?)
    }

    /// Open milestones, newest first as GitHub returns them.
    pub async fn list_milestones(&self, owner: &str, repo: &str) -> Result<Vec<Milestone>> {
        let url = format!(
            "{}/repos/{}/{}/milestones?state=open&per_page=100",
            self.base_url, owner, repo
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        Ok(resp.json::<Vec<Milestone>>().await?)
    }

    /// Remaining API quota for the installation token. `/rate_limit` does not
    /// count against the limit, which makes it safe to poll.
    pub async fn rate_limit(&self) -> Result<serde_json::Value> {
        let url = format!("{}/rate_limit", self.base_url);
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn list_repo_issues(
        &self,
        owner: &str,
        repo: &str,
        state: &str,
        per_page: u32,
    ) -> Result<Vec<Issue>> {
        self.list_repo_issues_page(owner, repo, state, per_page, 1)
            .await
    }

    /// One page of the issues list. Note that this endpoint returns **issues and
    /// pull requests interleaved**, so a caller that wants issues only has to
    /// keep walking pages until its window is full.
    pub async fn list_repo_issues_page(
        &self,
        owner: &str,
        repo: &str,
        state: &str,
        per_page: u32,
        page: u32,
    ) -> Result<Vec<Issue>> {
        let url = format!(
            "{}/repos/{}/{}/issues?state={}&per_page={}&page={}&sort=updated&direction=desc",
            self.base_url,
            seg(owner),
            seg(repo),
            seg(state),
            per_page,
            page
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let issues = resp.json::<Vec<Issue>>().await?;
        Ok(issues)
    }

    // --- Pull Requests API ---

    pub async fn get_pull_request(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<PullRequest> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.base_url, owner, repo, pr_number
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let pr = resp.json::<PullRequest>().await?;
        Ok(pr)
    }

    pub async fn list_pr_files(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<PullRequestFile>> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/files?per_page=100",
            self.base_url, owner, repo, pr_number
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let files = resp.json::<Vec<PullRequestFile>>().await?;
        Ok(files)
    }

    pub async fn list_pr_commits(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<CommitItem>> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/commits?per_page=100",
            self.base_url, owner, repo, pr_number
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let commits = resp.json::<Vec<CommitItem>>().await?;
        Ok(commits)
    }

    pub async fn request_reviewers(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        reviewers: &[String],
    ) -> Result<()> {
        if reviewers.is_empty() {
            return Ok(());
        }
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/requested_reviewers",
            self.base_url, owner, repo, pr_number
        );
        let payload = json!({ "reviewers": reviewers });
        self.send_logged(reqwest::Method::POST, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn remove_reviewers(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        reviewers: &[String],
    ) -> Result<()> {
        if reviewers.is_empty() {
            return Ok(());
        }
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/requested_reviewers",
            self.base_url, owner, repo, pr_number
        );
        let payload = json!({ "reviewers": reviewers });
        self.send_logged(reqwest::Method::DELETE, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn list_repo_pulls(
        &self,
        owner: &str,
        repo: &str,
        state: &str,
        per_page: u32,
    ) -> Result<Vec<PullRequest>> {
        self.list_repo_pulls_page(owner, repo, state, per_page, 1)
            .await
    }

    /// One page of the pull-request list (no issues mixed in, so page `n` starts
    /// at `(n - 1) * per_page + 1`).
    pub async fn list_repo_pulls_page(
        &self,
        owner: &str,
        repo: &str,
        state: &str,
        per_page: u32,
        page: u32,
    ) -> Result<Vec<PullRequest>> {
        let url = format!(
            "{}/repos/{}/{}/pulls?state={}&per_page={}&page={}&sort=updated&direction=desc",
            self.base_url,
            seg(owner),
            seg(repo),
            seg(state),
            per_page,
            page
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let pulls = resp.json::<Vec<PullRequest>>().await?;
        Ok(pulls)
    }

    // --- Commit Status API ---

    /// One commit status update.
    ///
    /// Grouped into a struct rather than five positional `&str` parameters:
    /// `state`, `context` and `description` are all free-form strings, and a
    /// positional call makes swapping them a silent, compiles-fine mistake.
    pub async fn create_commit_status(
        &self,
        owner: &str,
        repo: &str,
        status: CommitStatus<'_>,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/statuses/{}",
            self.base_url, owner, repo, status.sha
        );
        let mut payload = json!({
            "state": status.state, // "error", "failure", "pending", "success"
            "context": status.context,
            "description": status.description,
        });
        if let Some(target) = status.target_url {
            payload["target_url"] = json!(target);
        }
        self.send_logged(reqwest::Method::POST, &url, Some(&payload))
            .await?
            .error_for_status()?;
        Ok(())
    }

    // --- Checks API ---

    pub async fn list_check_runs_for_ref(
        &self,
        owner: &str,
        repo: &str,
        git_ref: &str,
    ) -> Result<Vec<CheckRun>> {
        let url = format!(
            "{}/repos/{}/{}/commits/{}/check-runs",
            self.base_url, owner, repo, git_ref
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let data = resp.json::<CheckRunsResponse>().await?;
        Ok(data.check_runs)
    }

    pub async fn rerequest_check_suite(
        &self,
        owner: &str,
        repo: &str,
        check_suite_id: u64,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/check-suites/{}/rerequest",
            self.base_url, owner, repo, check_suite_id
        );
        self.send_logged(reqwest::Method::POST, &url, None)
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Check suites attached to a commit ref — used by the console `/retest`.
    pub async fn list_check_suites_for_ref(
        &self,
        owner: &str,
        repo: &str,
        git_ref: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "{}/repos/{}/{}/commits/{}/check-suites?per_page=50",
            self.base_url, owner, repo, git_ref
        );
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        let data: serde_json::Value = resp.json().await?;
        Ok(data
            .get("check_suites")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    // --- Repository Metadata & Commits (admin console) ---

    /// Raw repository payload: stars, forks, open issue counters, default branch.
    pub async fn get_repo(&self, owner: &str, repo: &str) -> Result<serde_json::Value> {
        let url = format!("{}/repos/{}/{}", self.base_url, owner, repo);
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        Ok(resp.json::<serde_json::Value>().await?)
    }

    /// Commits on the default branch, optionally since an RFC3339 timestamp.
    pub async fn list_commits(
        &self,
        owner: &str,
        repo: &str,
        since: Option<&str>,
        per_page: u32,
    ) -> Result<Vec<serde_json::Value>> {
        let mut url = format!(
            "{}/repos/{}/{}/commits?per_page={}",
            self.base_url, owner, repo, per_page
        );
        if let Some(since) = since {
            url.push_str(&format!("&since={}", since));
        }
        let resp = self
            .send_logged(reqwest::Method::GET, &url, None)
            .await?
            .error_for_status()?;
        Ok(resp.json::<Vec<serde_json::Value>>().await?)
    }

    // --- Repository Content API ---

    pub async fn get_repo_file_content(
        &self,
        owner: &str,
        repo: &str,
        file_path: &str,
        git_ref: Option<&str>,
    ) -> Result<Option<String>> {
        let mut url = format!(
            "{}/repos/{}/{}/contents/{}",
            self.base_url, owner, repo, file_path
        );
        if let Some(r) = git_ref {
            url.push_str(&format!("?ref={}", r));
        }

        let resp = self.send_logged(reqwest::Method::GET, &url, None).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        let file_data = resp.json::<ContentFileResponse>().await?;

        if file_data.encoding == "base64" {
            // Clean up whitespace/newlines from base64
            let cleaned: String = file_data
                .content
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(cleaned.as_bytes())
                .context("Failed to decode base64 file content")?;
            let content_str = String::from_utf8(bytes)?;
            Ok(Some(content_str))
        } else {
            Ok(Some(file_data.content))
        }
    }

    // --- GraphQL API (Projects v2) ---

    pub async fn graphql<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T> {
        let url = if self.base_url.ends_with("/api/v3") {
            format!("{}/graphql", self.base_url)
        } else {
            "https://api.github.com/graphql".to_string()
        };

        let payload = json!({
            "query": query,
            "variables": variables,
        });

        let resp = self
            .send_logged(reqwest::Method::POST, &url, Some(&payload))
            .await?
            .error_for_status()?;
        let result = resp.json::<GraphQLResponse<T>>().await?;
        if let Some(errors) = result.errors {
            let err_msgs: Vec<String> = errors.into_iter().map(|e| e.message).collect();
            anyhow::bail!("GraphQL error: {}", err_msgs.join(", "));
        }
        match result.data {
            Some(data) => Ok(data),
            None => anyhow::bail!("No data returned in GraphQL response"),
        }
    }
}

#[derive(Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize)]
struct GraphQLError {
    message: String,
}

/// Extract the page number from a `Link` header's `rel="last"` entry.
fn last_page(link: &str) -> Option<u64> {
    link.split(',')
        .find(|part| part.contains("rel=\"last\""))
        .and_then(|part| part.split("page=").nth(1))
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse::<u64>().ok())
}

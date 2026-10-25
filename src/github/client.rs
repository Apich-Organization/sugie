use super::models::{
    CheckRun, Comment, CommitItem, Issue, PullRequest, PullRequestFile,
};
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;
use serde_json::json;

#[derive(Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
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

impl GitHubClient {
    pub fn new(token: String, base_url: Option<String>) -> Self {
        let mut headers = HeaderMap::new();
        let auth_val = format!("Bearer {}", token);
        let mut auth_header = HeaderValue::from_str(&auth_val).unwrap();
        auth_header.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_header);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("sugie-bot/0.1.0"));
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_default();

        Self {
            http,
            base_url: base_url.unwrap_or_else(|| "https://api.github.com".to_string()),
            token,
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    // --- Issues API ---

    pub async fn get_issue(&self, owner: &str, repo: &str, issue_number: u64) -> Result<Issue> {
        let url = format!("{}/repos/{}/{}/issues/{}", self.base_url, owner, repo, issue_number);
        let resp = self.http.get(&url).send().await?.error_for_status()?;
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

        let resp = self.http.post(&url).json(&payload).send().await?.error_for_status()?;
        let issue = resp.json::<Issue>().await?;
        Ok(issue)
    }

    pub async fn update_issue_state(
        &self,
        owner: &str,
        repo: &str,
        issue_number: u64,
        state: &str, // "open" or "closed"
        state_reason: Option<&str>, // "completed", "not_planned", "reopened"
    ) -> Result<()> {
        let url = format!("{}/repos/{}/{}/issues/{}", self.base_url, owner, repo, issue_number);
        let mut payload = json!({ "state": state });
        if let Some(reason) = state_reason {
            payload["state_reason"] = json!(reason);
        }
        self.http.patch(&url).json(&payload).send().await?.error_for_status()?;
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
        let resp = self.http.post(&url).json(&payload).send().await?.error_for_status()?;
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
        self.http.patch(&url).json(&payload).send().await?.error_for_status()?;
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
        let resp = self.http.get(&url).send().await?.error_for_status()?;
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
        self.http.post(&url).json(&payload).send().await?.error_for_status()?;
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
            self.base_url, owner, repo, issue_number, label
        );
        let _ = self.http.delete(&url).send().await;
        Ok(())
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
        self.http.post(&url).json(&payload).send().await?.error_for_status()?;
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
        self.http.delete(&url).json(&payload).send().await?.error_for_status()?;
        Ok(())
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
        self.http.put(&url).json(&payload).send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn list_repo_issues(
        &self,
        owner: &str,
        repo: &str,
        state: &str,
        per_page: u32,
    ) -> Result<Vec<Issue>> {
        let url = format!(
            "{}/repos/{}/{}/issues?state={}&per_page={}&sort=updated&direction=desc",
            self.base_url, owner, repo, state, per_page
        );
        let resp = self.http.get(&url).send().await?.error_for_status()?;
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
        let url = format!("{}/repos/{}/{}/pulls/{}", self.base_url, owner, repo, pr_number);
        let resp = self.http.get(&url).send().await?.error_for_status()?;
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
        let resp = self.http.get(&url).send().await?.error_for_status()?;
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
        let resp = self.http.get(&url).send().await?.error_for_status()?;
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
        self.http.post(&url).json(&payload).send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn list_repo_pulls(
        &self,
        owner: &str,
        repo: &str,
        state: &str,
        per_page: u32,
    ) -> Result<Vec<PullRequest>> {
        let url = format!(
            "{}/repos/{}/{}/pulls?state={}&per_page={}&sort=updated&direction=desc",
            self.base_url, owner, repo, state, per_page
        );
        let resp = self.http.get(&url).send().await?.error_for_status()?;
        let pulls = resp.json::<Vec<PullRequest>>().await?;
        Ok(pulls)
    }

    // --- Commit Status API ---

    pub async fn create_commit_status(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
        state: &str, // "error", "failure", "pending", "success"
        context: &str,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/statuses/{}",
            self.base_url, owner, repo, sha
        );
        let mut payload = json!({
            "state": state,
            "context": context,
            "description": description,
        });
        if let Some(target) = target_url {
            payload["target_url"] = json!(target);
        }
        self.http.post(&url).json(&payload).send().await?.error_for_status()?;
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
        let resp = self.http.get(&url).send().await?.error_for_status()?;
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
        self.http.post(&url).send().await?.error_for_status()?;
        Ok(())
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

        let resp = self.http.get(&url).send().await?;
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

        let resp = self.http.post(&url).json(&payload).send().await?.error_for_status()?;
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

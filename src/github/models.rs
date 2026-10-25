use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub login: String,
    pub id: u64,
    #[serde(default)]
    pub r#type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    pub owner: RepoOwner,
    #[serde(default = "default_branch_name")]
    pub default_branch: String,
    #[serde(default)]
    pub private: bool,
}

fn default_branch_name() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoOwner {
    pub login: String,
    pub id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: Option<u64>,
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub user: User,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<User>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub pull_request: Option<IssuePullRequestRef>,
    pub author_association: Option<String>,
    pub comments: Option<u32>,
}

impl Issue {
    pub fn is_pr(&self) -> bool {
        self.pull_request.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuePullRequestRef {
    pub url: Option<String>,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRef {
    pub r#ref: String,
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub user: User,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<User>,
    #[serde(default)]
    pub requested_reviewers: Vec<User>,
    pub head: GitRef,
    pub base: GitRef,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    pub merged_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub author_association: Option<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_files: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub body: String,
    pub user: User,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallationRef {
    pub id: u64,
    pub account: Option<User>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestFile {
    pub sha: Option<String>,
    pub filename: String,
    pub status: Option<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitAuthor {
    pub name: String,
    pub email: String,
    pub date: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitDetails {
    pub author: Option<CommitAuthor>,
    pub committer: Option<CommitAuthor>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitItem {
    pub sha: String,
    pub commit: CommitDetails,
    pub author: Option<User>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckSuite {
    pub id: u64,
    pub head_sha: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub pull_requests: Vec<PullRequestMin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestMin {
    pub id: u64,
    pub number: u64,
    pub head: GitRef,
    pub base: GitRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRun {
    pub id: u64,
    pub name: String,
    pub head_sha: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub html_url: Option<String>,
    pub output: Option<CheckRunOutput>,
    #[serde(default)]
    pub pull_requests: Vec<PullRequestMin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRunOutput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub text: Option<String>,
}

// Webhook Event Envelopes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueWebhookEvent {
    pub action: String,
    pub issue: Issue,
    pub repository: Repository,
    pub sender: User,
    pub installation: Option<InstallationRef>,
    pub label: Option<Label>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestWebhookEvent {
    pub action: String,
    pub pull_request: PullRequest,
    pub repository: Repository,
    pub sender: User,
    pub installation: Option<InstallationRef>,
    pub label: Option<Label>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueCommentWebhookEvent {
    pub action: String,
    pub issue: Issue,
    pub comment: Comment,
    pub repository: Repository,
    pub sender: User,
    pub installation: Option<InstallationRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckSuiteWebhookEvent {
    pub action: String,
    pub check_suite: CheckSuite,
    pub repository: Repository,
    pub sender: User,
    pub installation: Option<InstallationRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRunWebhookEvent {
    pub action: String,
    pub check_run: CheckRun,
    pub repository: Repository,
    pub sender: User,
    pub installation: Option<InstallationRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallationRepositoriesWebhookEvent {
    pub action: String,
    pub installation: InstallationRef,
    #[serde(default)]
    pub repositories_added: Vec<RepositoryMin>,
    #[serde(default)]
    pub repositories_removed: Vec<RepositoryMin>,
    pub sender: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallationWebhookEvent {
    pub action: String,
    pub installation: InstallationRef,
    #[serde(default)]
    pub repositories: Vec<RepositoryMin>,
    pub sender: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryMin {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    #[serde(default)]
    pub private: bool,
}

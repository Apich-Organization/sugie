//! The web command console: parse a slash command, enforce permissions, run it
//! against the GitHub API and stream the log lines back to the browser.

use crate::config::SugieConfig;
use crate::metrics::MetricsReporter;
use crate::notifications::NotificationDispatcher;
use crate::stale::StaleManager;
use crate::web::config_store::ConfigSummary;
use crate::web::perms::{command_spec, CurrentUser, COMMANDS};
use crate::web::stats;
use crate::web::WebState;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const MAX_JOBS: usize = 120;
/// Hard cap on retained output per job. A chatty command (a full `/stale` sweep)
/// could otherwise grow without bound and eventually exhaust memory.
const MAX_LINES: usize = 5000;
/// `/issues` and `/prs` default row count, and the page size requested from
/// GitHub (which caps `per_page` at 100).
const DEFAULT_LIST_COUNT: usize = 10;
const LIST_PAGE_SIZE: u32 = 100;
/// Raw API pages one list command may walk. The issues endpoint interleaves pull
/// requests, so filling a 100-row window can take several pages; this bounds the
/// cost of a single command.
const MAX_RAW_PAGES: u32 = 5;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
    /// Stopped on request. A distinct state so the UI can say "cancelled"
    /// instead of blaming the command for a failure.
    Cancelled,
}

#[derive(Clone, Serialize, Debug)]
pub struct LogLine {
    pub level: String,
    pub text: String,
    pub ts: String,
}

#[derive(Clone, Serialize, Debug)]
pub struct Job {
    pub id: String,
    pub username: String,
    pub repo: String,
    pub command: String,
    pub status: JobStatus,
    pub lines: Vec<LogLine>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Handle used to stop the task. Not part of the API payload: it is an
    /// implementation detail and not serialisable.
    #[serde(skip)]
    pub abort: Option<tokio::task::AbortHandle>,
}

#[derive(Default)]
pub struct JobRegistry {
    inner: Mutex<(HashMap<String, Job>, VecDeque<String>)>,
}

impl JobRegistry {
    pub fn create(&self, username: &str, repo: &str, command: &str) -> String {
        let id = crate::web::auth::random_token()
            .chars()
            .take(12)
            .collect::<String>();
        let job = Job {
            id: id.clone(),
            username: username.to_string(),
            repo: repo.to_string(),
            command: command.to_string(),
            status: JobStatus::Running,
            lines: vec![LogLine {
                level: "cmd".to_string(),
                text: command.to_string(),
                ts: clock(),
            }],
            started_at: clock(),
            finished_at: None,
            abort: None,
        };
        if let Ok(mut guard) = self.inner.lock() {
            let (jobs, order) = &mut *guard;
            jobs.insert(id.clone(), job);
            order.push_back(id.clone());
            evict(jobs, order);
        }
        id
    }

    pub fn log(&self, id: &str, level: &str, text: impl Into<String>) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(job) = guard.0.get_mut(id) {
                if job.lines.len() >= MAX_LINES {
                    // Append the notice exactly once, then drop further lines.
                    if job.lines.len() == MAX_LINES {
                        job.lines.push(LogLine {
                            level: "err".to_string(),
                            text: format!("… output truncated at {MAX_LINES} lines"),
                            ts: clock(),
                        });
                    }
                    return;
                }
                job.lines.push(LogLine {
                    level: level.to_string(),
                    text: text.into(),
                    ts: clock(),
                });
            }
        }
    }

    pub fn finish(&self, id: &str, ok: bool) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(job) = guard.0.get_mut(id) {
                // First terminal state wins: a job cancelled a moment before it
                // completed must not be relabelled "done" by the late task.
                if job.status != JobStatus::Running {
                    return;
                }
                job.status = if ok {
                    JobStatus::Done
                } else {
                    JobStatus::Failed
                };
                job.finished_at = Some(clock());
            }
        }
    }

    /// Remember how to stop a job that was just spawned.
    pub fn attach(&self, id: &str, handle: tokio::task::AbortHandle) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(job) = guard.0.get_mut(id) {
                job.abort = Some(handle);
            }
        }
    }

    /// Stop a running job. Returns the owner and command so the caller can write
    /// an audit entry; `None` when the job is gone or already finished.
    pub fn cancel(&self, id: &str) -> Option<(String, String, String)> {
        let (handle, info) = {
            let mut guard = self.inner.lock().ok()?;
            let job = guard.0.get_mut(id)?;
            if job.status != JobStatus::Running {
                return None;
            }
            job.status = JobStatus::Cancelled;
            job.finished_at = Some(clock());
            job.lines.push(LogLine {
                level: "err".to_string(),
                text: "Cancelled on request.".to_string(),
                ts: clock(),
            });
            (
                job.abort.take(),
                (job.username.clone(), job.repo.clone(), job.command.clone()),
            )
        };
        if let Some(h) = handle {
            h.abort();
        }
        Some(info)
    }

    pub fn snapshot(&self, id: &str) -> Option<Job> {
        self.inner.lock().ok()?.0.get(id).cloned()
    }

    pub fn since(&self, id: &str, from: usize) -> Option<(Vec<LogLine>, JobStatus)> {
        let guard = self.inner.lock().ok()?;
        let job = guard.0.get(id)?;
        let lines = job.lines.iter().skip(from).cloned().collect();
        Some((lines, job.status))
    }

    pub fn recent_for(&self, username: &str, limit: usize) -> Vec<Job> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        // Walk the order deque newest-first and clone only what is returned: the
        // previous version cloned every retained job (up to 5001 log lines each).
        guard
            .1
            .iter()
            .rev()
            .filter_map(|id| guard.0.get(id))
            .filter(|j| j.username.eq_ignore_ascii_case(username))
            .take(limit)
            .cloned()
            .collect()
    }
}

/// Drop the oldest *finished* job until the registry fits in `MAX_JOBS`.
///
/// Evicting a `Running` entry would make its SSE client see "Job expired" in the
/// middle of a long `/stale` sweep, so running jobs are skipped. If every job is
/// still running the map is allowed to overshoot up to `MAX_JOBS * 2`, after
/// which the oldest running job is dropped as a last resort.
fn evict(jobs: &mut HashMap<String, Job>, order: &mut VecDeque<String>) {
    while order.len() > MAX_JOBS {
        let pos = order.iter().position(|id| {
            jobs.get(id)
                .map(|j| j.status != JobStatus::Running)
                // An order entry with no job left is stale: remove it too.
                .unwrap_or(true)
        });
        if let Some(pos) = pos {
            if let Some(old) = order.remove(pos) {
                jobs.remove(&old);
            }
        } else if order.len() > MAX_JOBS * 2 {
            if let Some(old) = order.pop_front() {
                jobs.remove(&old);
            }
        } else {
            break;
        }
    }
}

fn clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

pub fn restricted_commands(cfg: &SugieConfig) -> Vec<String> {
    cfg.default_rules.commands.restricted_commands.clone()
}

/// Validate a submission synchronously so the HTTP layer can answer 403 before
/// a job is ever created, then run it in the background.
pub fn dispatch(
    state: WebState,
    user: CurrentUser,
    repo: String,
    raw: String,
    ip: &str,
) -> Result<String, String> {
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return Err("Command cannot be empty".to_string());
    }
    let (raw_cmd, args) = split_command(&raw);
    // Honour `[default_rules.commands.aliases]` from sugie.toml so the console
    // behaves like the bot (`/l 42 bug` → `/label 42 bug`).
    let cmd = state
        .config
        .default_rules
        .commands
        .aliases
        .get(&raw_cmd)
        .map(|v| v.trim_start_matches('/').to_ascii_lowercase())
        .unwrap_or(raw_cmd);
    let spec = command_spec(&cmd)
        .ok_or_else(|| format!("Unknown command `/{cmd}`, run /help to see supported commands"))?;

    if spec.admin_only && !user.is_admin() {
        return Err(format!("/{} is restricted to administrators", spec.name));
    }
    if !user.can_run(spec.name, &restricted_commands(&state.config)) {
        return Err(format!("Permission denied to run /{}", spec.name));
    }
    if !user.can_access_repo(&repo) {
        return Err(format!("No access to repository {repo}"));
    }

    let job_id = state.jobs.create(&user.username, &repo, &raw);
    let spawn_state = state.clone();
    let spawn_job = job_id.clone();
    let audit_ip = ip.to_string();
    let handle = tokio::spawn(async move {
        let ctx = Ctx {
            state: &spawn_state,
            job: &spawn_job,
            repo: &repo,
            user: &user,
        };
        let ok = run(&ctx, &cmd, &args).await;
        spawn_state.jobs.finish(&spawn_job, ok);
        spawn_state.db.audit(
            &user.username,
            "command",
            &repo,
            &format!("/{} {}", cmd, args.join(" ")),
            &audit_ip,
            ok,
        );
    });
    // Keep a handle so a second request can stop the job. Dropping the future at
    // its next await point is the cooperative stop every command loop already
    // respects, because each iteration calls GitHub.
    state.jobs.attach(&job_id, handle.abort_handle());

    Ok(job_id)
}

fn split_command(raw: &str) -> (String, Vec<String>) {
    let mut parts = split_args(raw);
    if parts.is_empty() {
        return (String::new(), Vec::new());
    }
    let head = parts.remove(0);
    (head.trim_start_matches('/').to_ascii_lowercase(), parts)
}

/// Whitespace split that keeps `"quoted strings"` together.
fn split_args(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in raw.chars() {
        match ch {
            '"' | '\'' if quote.is_none() => quote = Some(ch),
            c if Some(c) == quote => quote = None,
            c if c.is_whitespace() && quote.is_none() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct Ctx<'a> {
    state: &'a WebState,
    job: &'a str,
    repo: &'a str,
    user: &'a CurrentUser,
}

impl<'a> Ctx<'a> {
    fn info(&self, msg: impl Into<String>) {
        self.state.jobs.log(self.job, "info", msg);
    }
    fn ok(&self, msg: impl Into<String>) {
        self.state.jobs.log(self.job, "ok", msg);
    }
    fn err(&self, msg: impl Into<String>) {
        self.state.jobs.log(self.job, "err", msg);
    }
    fn out(&self, msg: impl Into<String>) {
        self.state.jobs.log(self.job, "out", msg);
    }

    fn parts(&self) -> Option<(&str, &str)> {
        self.repo.split_once('/')
    }

    fn need_number(&self, args: &[String]) -> Option<u64> {
        match args
            .first()
            .and_then(|a| a.trim_start_matches('#').parse::<u64>().ok())
        {
            Some(n) => Some(n),
            None => {
                self.err("Requires an issue/PR number, e.g. `/label 42 bug`");
                None
            }
        }
    }
}

async fn run(ctx: &Ctx<'_>, cmd: &str, args: &[String]) -> bool {
    let Some((owner, repo)) = ctx.parts() else {
        ctx.err("Invalid repository name, must be in owner/repo format");
        return false;
    };

    match cmd {
        "help" => {
            let restricted = restricted_commands(&ctx.state.config);
            ctx.info(format!(
                "Current user @{} · Role {} · Repository {}",
                ctx.user.username,
                ctx.user.role.label(),
                ctx.repo
            ));
            for spec in COMMANDS {
                let allowed = ctx.user.can_run(spec.name, &restricted);
                ctx.out(format!(
                    "{:<32} {:<38} {}",
                    spec.usage,
                    spec.help,
                    if allowed { "" } else { "(no permission)" }
                ));
            }
            true
        }

        "status" => {
            let repos = ctx
                .state
                .triage
                .storage()
                .list_repositories()
                .unwrap_or_default();
            let uptime = chrono::Utc::now() - ctx.state.started_at;
            ctx.ok(format!(
                "Sugie v{} running · {} connected repositories · Uptime {}",
                env!("CARGO_PKG_VERSION"),
                repos.len(),
                human_duration(uptime)
            ));
            ctx.out(format!(
                "Configuration: {}",
                ctx.state.files.toml_path.display()
            ));
            ctx.out(format!("Database:      {}", ctx.state.config.db_path));
            for (_, name, inst) in repos.iter().take(20) {
                ctx.out(format!("  · {name}  (installation {inst})"));
            }
            true
        }

        "stats" => {
            ctx.info("Fetching stats from GitHub…");
            let s = stats::fetch(&ctx.state.triage, &ctx.state.stats, ctx.repo, true).await;
            if let Some(ref e) = s.error {
                ctx.err(e.clone());
            }
            ctx.out(format!("Repository    {}", s.full_name));
            ctx.out(format!(
                "open issues {} · open PRs {} · 30d commits {} · 30d merged {}",
                s.open_issues, s.open_prs, s.commits_30d, s.merged_30d
            ));
            ctx.out(format!(
                "stars {} · forks {} · default branch {}",
                s.stars, s.forks, s.default_branch
            ));
            s.is_ok()
        }

        "issues" | "prs" => {
            if cmd == "issues" {
                run_issue_list(ctx, owner, repo, args).await
            } else {
                run_pr_list(ctx, owner, repo, args).await
            }
        }

        "issue" | "show" => run_issue_detail(ctx, owner, repo, args).await,
        "checks" => run_checks(ctx, owner, repo, args).await,
        "labels" => run_labels(ctx, owner, repo).await,
        "milestones" => run_milestones(ctx, owner, repo).await,
        "rate" => run_rate(ctx).await,

        "label" | "unlabel" => {
            let Some(num) = ctx.need_number(args) else {
                return false;
            };
            if args.len() < 2 {
                ctx.err(
                    "Please specify label name(s), e.g. `/label 42 bug` or `/label 42 +bug -wip`",
                );
                return false;
            }
            // Same `+add` / `-remove` syntax as the bot's issue-comment commands.
            // Multi-word labels must be quoted: `/label 42 "good first issue"`.
            let mut add: Vec<String> = Vec::new();
            let mut remove: Vec<String> = Vec::new();
            for tok in &args[1..] {
                if let Some(rest) = tok.strip_prefix('-').filter(|r| !r.is_empty()) {
                    remove.push(rest.to_string());
                } else if let Some(rest) = tok.strip_prefix('+').filter(|r| !r.is_empty()) {
                    add.push(rest.to_string());
                } else if cmd == "unlabel" {
                    remove.push(tok.clone());
                } else {
                    add.push(tok.clone());
                }
            }
            if add.is_empty() && remove.is_empty() {
                ctx.err("Please specify label name(s), e.g. `/label 42 bug`");
                return false;
            }

            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };

            let mut ok = true;
            if !add.is_empty() {
                match client.add_labels(owner, repo, num, &add).await {
                    Ok(_) => ctx.ok(format!(
                        "Added labels on {}{}: {}",
                        ctx.repo,
                        format!("#{num}"),
                        add.join(", ")
                    )),
                    Err(e) => {
                        ctx.err(format!("Failed to add labels {}: {e}", add.join(", ")));
                        ok = false;
                    }
                }
            }
            for l in &remove {
                match client.remove_label(owner, repo, num, l).await {
                    Ok(_) => ctx.ok(format!(
                        "Removed label `{l}` on {}{}",
                        ctx.repo,
                        format!("#{num}")
                    )),
                    Err(e) => {
                        ctx.err(format!("Failed to remove label `{l}`: {e}"));
                        ok = false;
                    }
                }
            }
            ok
        }

        "assign" | "unassign" => {
            let Some(num) = args
                .first()
                .and_then(|a| a.trim_start_matches('#').parse::<u64>().ok())
            else {
                ctx.err(format!(
                    "Usage: /{cmd} <issue/PR-num> <@username...>  (e.g. `/{cmd} 42 @octocat`)"
                ));
                print_user_hints(ctx, owner, repo).await;
                return false;
            };
            let users: Vec<String> = args[1..]
                .iter()
                .map(|u| u.trim_start_matches('@').to_string())
                .filter(|u| !u.is_empty())
                .collect();
            if users.is_empty() {
                ctx.err(format!(
                    "Please specify username(s), e.g. `/{cmd} {num} @octocat`"
                ));
                print_user_hints(ctx, owner, repo).await;
                return false;
            }
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            let res = if cmd == "assign" {
                client.add_assignees(owner, repo, num, &users).await
            } else {
                client.remove_assignees(owner, repo, num, &users).await
            };
            match res {
                Ok(_) => {
                    ctx.ok(format!(
                        "{} {}{} → {}",
                        if cmd == "assign" {
                            "Assigned"
                        } else {
                            "Unassigned"
                        },
                        ctx.repo,
                        format!("#{num}"),
                        users.join(", ")
                    ));
                    true
                }
                Err(e) => {
                    ctx.err(format!("Operation failed: {e}"));
                    false
                }
            }
        }

        "users" | "assignees" | "collaborators" | "team" => run_users(ctx, owner, repo).await,

        "workload" => {
            match ctx
                .state
                .triage
                .storage()
                .list_all_reviewer_workloads(Some(ctx.repo))
            {
                Ok(rows) if rows.is_empty() => {
                    ctx.info(format!("No reviewer workloads tracked for {}", ctx.repo));
                    true
                }
                Ok(rows) => {
                    ctx.ok(format!("Active review workload for {}", ctx.repo));
                    for (username, _repo, active) in rows {
                        ctx.out(format!("  @{username:<24} {active} active reviews"));
                    }
                    true
                }
                Err(e) => {
                    ctx.err(format!("Failed to read workloads: {e}"));
                    false
                }
            }
        }

        "reviewer" | "reviewers" | "review" | "reassign" | "rereview" | "unreviewer"
        | "unreviewers" => run_reviewer(ctx, cmd, args, owner, repo).await,

        "project" | "move" | "duplicate" => run_issue_command(ctx, cmd, args, owner, repo).await,

        "close" | "reopen" => {
            let Some(num) = ctx.need_number(args) else {
                return false;
            };
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            let res = if cmd == "close" {
                // GitHub accepts exactly `completed` and `not_planned`. Anything
                // else is a user mistake and should be reported, not silently
                // downgraded to `completed`.
                let raw = args[1..].join(" ").trim().to_ascii_lowercase();
                let reason = match raw.replace('-', " ").as_str() {
                    "" | "completed" | "complete" | "done" | "fixed" => "completed",
                    "not planned" | "notplanned" | "wontfix" | "invalid" | "duplicate" => {
                        "not_planned"
                    }
                    _ => {
                        ctx.err(format!(
                            "Unknown close reason `{raw}`. Use `completed` or `not planned`."
                        ));
                        return false;
                    }
                };
                client
                    .update_issue_state(owner, repo, num, "closed", Some(reason))
                    .await
            } else {
                client
                    .update_issue_state(owner, repo, num, "open", Some("reopened"))
                    .await
            };
            match res {
                Ok(_) => {
                    ctx.ok(format!(
                        "{} {}{}",
                        if cmd == "close" { "Closed" } else { "Reopened" },
                        ctx.repo,
                        format!("#{num}")
                    ));
                    true
                }
                Err(e) => {
                    ctx.err(format!("Operation failed: {e}"));
                    false
                }
            }
        }

        "comment" => {
            let Some(num) = ctx.need_number(args) else {
                return false;
            };
            let body = args[1..].join(" ");
            if body.trim().is_empty() {
                ctx.err("Comment body cannot be empty, e.g. `/comment 42 Fixed in v0.2.1`");
                return false;
            }
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            match client.create_comment(owner, repo, num, &body).await {
                Ok(c) => {
                    ctx.ok(format!("Commented on {}{}", ctx.repo, format!("#{num}")));
                    if let Some(url) = c.html_url {
                        ctx.out(url);
                    }
                    true
                }
                Err(e) => {
                    ctx.err(format!("Failed to post comment: {e}"));
                    false
                }
            }
        }

        "title" => run_title(ctx, owner, repo, args).await,
        "milestone" => run_milestone(ctx, owner, repo, args).await,
        "lock" => run_lock(ctx, owner, repo, args, true).await,
        "unlock" => run_lock(ctx, owner, repo, args, false).await,

        "retest" => {
            let Some(num) = ctx.need_number(args) else {
                return false;
            };
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            let pr = match client.get_pull_request(owner, repo, num).await {
                Ok(p) => p,
                Err(e) => {
                    ctx.err(format!("Failed to fetch PR: {e}"));
                    return false;
                }
            };
            let suites = match client
                .list_check_suites_for_ref(owner, repo, &pr.head.sha)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    ctx.err(format!("Failed to fetch check suites: {e}"));
                    return false;
                }
            };
            if suites.is_empty() {
                ctx.err(format!(
                    "No check suites available to re-run on {}{}",
                    ctx.repo,
                    format!("#{num}")
                ));
                return false;
            }
            let failed: Vec<&serde_json::Value> = suites
                .iter()
                .filter(|suite| {
                    matches!(
                        suite.get("conclusion").and_then(|v| v.as_str()),
                        Some("failure")
                            | Some("timed_out")
                            | Some("cancelled")
                            | Some("action_required")
                    )
                })
                .collect();
            if failed.is_empty() {
                ctx.info(format!(
                    "All {} check suite(s) on {}{} already passed — nothing to re-run",
                    suites.len(),
                    ctx.repo,
                    format!("#{num}")
                ));
                return true;
            }
            let mut rerun = 0;
            let mut errors = 0;
            for suite in &failed {
                let Some(id) = suite.get("id").and_then(|v| v.as_u64()) else {
                    continue;
                };
                match client.rerequest_check_suite(owner, repo, id).await {
                    Ok(_) => rerun += 1,
                    Err(e) => {
                        errors += 1;
                        ctx.err(format!("Failed to re-run suite {id}: {e}"));
                    }
                }
            }
            if rerun == 0 {
                ctx.err(format!(
                    "Could not re-trigger any of the {} failed check suite(s)",
                    failed.len()
                ));
                return false;
            }
            ctx.ok(format!(
                "Re-triggered {rerun}/{} failed check suite(s) ({} head {}){}",
                failed.len(),
                ctx.repo,
                &pr.head.sha[..7.min(pr.head.sha.len())],
                if errors > 0 {
                    format!(", {errors} failed")
                } else {
                    String::new()
                }
            ));
            true
        }

        "stale" => {
            let rules = ctx.state.config.default_rules.stale.clone();
            if !rules.enabled {
                ctx.err("Stale lifecycle rules are disabled in config (default_rules.stale.enabled = false)");
                return false;
            }
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            let mgr = StaleManager::new(
                rules,
                ctx.state.triage.storage().clone(),
                NotificationDispatcher::new(&ctx.state.config.notifications),
            );
            ctx.info("Starting stale sweep…");
            match mgr.process_repository(&client, owner, repo).await {
                Ok(r) => {
                    ctx.ok(format!(
                        "Stale sweep completed: warned {} · closed {} · locked {}",
                        r.warned_count, r.closed_count, r.locked_count
                    ));
                    true
                }
                Err(e) => {
                    ctx.err(format!("Stale sweep failed: {e}"));
                    false
                }
            }
        }

        "report" => {
            let monthly = args
                .first()
                .map(|a| a.eq_ignore_ascii_case("monthly"))
                .unwrap_or(false);
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            let cfg = ctx.state.config.default_rules.reports.clone();
            let reporter = MetricsReporter::new(
                cfg,
                NotificationDispatcher::new(&ctx.state.config.notifications),
            );
            let days = if monthly { 30 } else { 7 };
            let label = if monthly { "Monthly" } else { "Weekly" };
            ctx.info(format!("Generating {label} report (past {days} days)…"));
            match reporter
                .generate_report(&client, owner, repo, days, label)
                .await
            {
                Ok(data) => {
                    for line in reporter.format_markdown_report(&data).lines() {
                        ctx.out(line.to_string());
                    }
                    true
                }
                Err(e) => {
                    ctx.err(format!("Failed to generate report: {e}"));
                    false
                }
            }
        }

        "config" => {
            let s = ConfigSummary::from_config(&ctx.state.config);
            ctx.out(format!("listen      {}:{}", s.host, s.port));
            ctx.out(format!("db_path     {}", s.db_path));
            ctx.out(format!(
                "channels    {}",
                if s.channels.is_empty() {
                    "(not configured)".to_string()
                } else {
                    s.channels.join(", ")
                }
            ));
            ctx.out(format!(
                "repos       {}",
                if s.allowed_repositories.is_empty() {
                    "(all connected repositories)".to_string()
                } else {
                    s.allowed_repositories.join(", ")
                }
            ));
            ctx.out(format!("super_users {}", s.super_users.join(", ")));
            ctx.out(format!(
                "restricted  /{}",
                s.restricted_commands.join(", /")
            ));
            ctx.out(format!(
                "stale={} reviewers={} labels={}",
                s.stale_enabled, s.reviewers_enabled, s.labels_enabled
            ));
            true
        }

        "reload" => {
            let path = ctx.state.files.toml_path.clone();
            match std::fs::read_to_string(&path) {
                Ok(content) => match ctx.state.files.validate_toml(&content) {
                    Ok(cfg) => {
                        ctx.ok(format!(
                            "{} validation passed (listen {}:{})",
                            path.display(),
                            cfg.host,
                            cfg.port
                        ));
                        ctx.info("Note: Running process uses startup configuration. Restart required to apply changes.");
                        true
                    }
                    Err(e) => {
                        ctx.err(format!("Validation failed: {e}"));
                        false
                    }
                },
                Err(e) => {
                    ctx.err(format!("Failed to read {}: {e}", path.display()));
                    false
                }
            }
        }

        "restart" => {
            if !ctx.user.is_owner() {
                ctx.err("Only owners can restart the service");
                return false;
            }
            ctx.ok("Graceful shutdown signal sent. The service will be restarted by systemd / container runtime.");
            ctx.state.shutdown.notify_waiters();
            true
        }

        other => {
            ctx.err(format!("Unimplemented command: /{other}"));
            false
        }
    }
}

/// `/project <num> <project> [status]`, `/move <num> <owner/repo>`, `/duplicate <num> <#original>`
/// Reuses the same executor as bot / GitHub-comment commands so behaviour stays identical.
async fn run_issue_command(
    ctx: &Ctx<'_>,
    cmd: &str,
    args: &[String],
    owner: &str,
    repo: &str,
) -> bool {
    use crate::github::models::User;
    use crate::triage::commands::{CommandExecutor, SlashCommand};
    use crate::triage::projects::ProjectsService;
    use crate::triage::reviewers::ReviewerLoadBalancer;

    let Some(num) = ctx.need_number(args) else {
        return false;
    };
    let command = match cmd {
        "project" => match args.get(1) {
            Some(target) => SlashCommand::Project {
                target: target.clone(),
                status: if args.len() > 2 {
                    Some(args[2..].join(" "))
                } else {
                    None
                },
            },
            None => {
                ctx.err("Usage: /project <num> <project-number> [status]");
                return false;
            }
        },
        "move" => match args.get(1) {
            Some(target) if target.contains('/') => {
                // Moving an issue *into* a repository is a write to that repo, so
                // it needs a grant for the destination, not just the source.
                if !ctx.user.can_access_repo(target) {
                    ctx.err(format!("No access to target repository {target}"));
                    return false;
                }
                SlashCommand::Move {
                    target_repo: target.clone(),
                }
            }
            _ => {
                ctx.err("Usage: /move <num> <owner/target-repo>");
                return false;
            }
        },
        _ => match args
            .get(1)
            .and_then(|a| a.trim_start_matches('#').parse::<u64>().ok())
        {
            Some(orig) => SlashCommand::Duplicate(orig),
            None => {
                ctx.err("Usage: /duplicate <num> <#original-number>");
                return false;
            }
        },
    };

    let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
        Ok(c) => c,
        Err(e) => {
            ctx.err(format!("Failed to get installation token: {e}"));
            return false;
        }
    };
    let issue = match client.get_issue(owner, repo, num).await {
        Ok(i) => i,
        Err(e) => {
            ctx.err(format!("Could not find issue/PR #{num}: {e}"));
            return false;
        }
    };
    let repo_config = ctx
        .state
        .triage
        .load_repo_config(&client, owner, repo)
        .await;
    let projects = ProjectsService::new(repo_config.projects.clone());
    let balancer = ReviewerLoadBalancer::new(
        repo_config.reviewers.clone(),
        ctx.state.triage.storage().clone(),
    );
    // The global restricted list and the per-user command grants were already
    // checked in `dispatch`, but a repository can narrow them further in its own
    // `.github/sugie.toml`. Honour that too instead of clearing it.
    if !ctx
        .user
        .can_run(cmd, &repo_config.commands.restricted_commands)
    {
        ctx.err(format!(
            "/{cmd} is restricted in this repository (see .github/sugie.toml)"
        ));
        return false;
    }
    // Console permission was already enforced (role + per-user grants), so lift the
    // super-user restriction that applies to GitHub-comment invocations.
    let mut commands_cfg = repo_config.commands.clone();
    commands_cfg.restricted_commands.clear();
    let executor = CommandExecutor::new(
        &client,
        &projects,
        ctx.state.triage.notifier(),
        &commands_cfg,
        Some(balancer),
    );
    let sender = User {
        login: ctx.user.username.clone(),
        id: 0,
        r#type: None,
    };
    match executor
        .execute(&command, owner, repo, &issue, &sender)
        .await
    {
        Ok(()) => {
            ctx.ok(format!("Executed /{cmd} on {}#{num}", ctx.repo));
            true
        }
        Err(e) => {
            ctx.err(format!("/{cmd} failed: {e}"));
            false
        }
    }
}

/// Collects eligible GitHub assignees, configured reviewer pool members, super users,
/// and tracked review workloads for `owner/repo`.
async fn collect_repo_users(
    ctx: &Ctx<'_>,
    owner: &str,
    repo: &str,
) -> Result<Vec<(String, Vec<String>)>, String> {
    let client = ctx
        .state
        .triage
        .get_client_for_repo(ctx.repo)
        .await
        .map_err(|e| format!("Failed to get installation token: {e}"))?;

    let repo_config = ctx
        .state
        .triage
        .load_repo_config(&client, owner, repo)
        .await;
    let gh_assignees: Vec<String> = client
        .list_assignees(owner, repo)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|u| u.login)
        .collect();
    let workloads = ctx
        .state
        .triage
        .storage()
        .list_all_reviewer_workloads(Some(ctx.repo))
        .unwrap_or_default();

    let mut ordered: Vec<String> = Vec::new();
    let mut push_unique = |name: &str| {
        let clean = name.trim().trim_start_matches('@');
        if !clean.is_empty() && !ordered.iter().any(|x| x.eq_ignore_ascii_case(clean)) {
            ordered.push(clean.to_string());
        }
    };

    for u in &repo_config.reviewers.pool {
        push_unique(u);
    }
    for u in &repo_config.commands.super_users {
        push_unique(u);
    }
    for (u, _, _) in &workloads {
        push_unique(u);
    }
    for u in &gh_assignees {
        push_unique(u);
    }

    let out = ordered
        .into_iter()
        .map(|login| {
            let mut tags = Vec::new();
            if repo_config
                .reviewers
                .pool
                .iter()
                .any(|p| p.trim_start_matches('@').eq_ignore_ascii_case(&login))
            {
                tags.push("Reviewer Pool".to_string());
            }
            if repo_config
                .commands
                .super_users
                .iter()
                .any(|s| s.trim_start_matches('@').eq_ignore_ascii_case(&login))
            {
                tags.push("Super User".to_string());
            }
            if gh_assignees.iter().any(|g| g.eq_ignore_ascii_case(&login)) {
                tags.push("GitHub Assignee".to_string());
            }
            if let Some((_, _, active)) = workloads
                .iter()
                .find(|(w, _, _)| w.eq_ignore_ascii_case(&login))
            {
                tags.push(format!("{active} active reviews"));
            }
            (login, tags)
        })
        .collect();

    Ok(out)
}

async fn run_users(ctx: &Ctx<'_>, owner: &str, repo: &str) -> bool {
    ctx.info(format!(
        "Fetching eligible GitHub assignees & reviewer pool for {}…",
        ctx.repo
    ));
    match collect_repo_users(ctx, owner, repo).await {
        Ok(users) if users.is_empty() => {
            ctx.info(format!(
                "No assignees or reviewer pool members found for {}",
                ctx.repo
            ));
            true
        }
        Ok(users) => {
            ctx.ok(format!(
                "Available GitHub users for {} ({} total):",
                ctx.repo,
                users.len()
            ));
            for (login, tags) in &users {
                let tag_str = if tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", tags.join(" · "))
                };
                ctx.out(format!("  @{login:<24}{tag_str}"));
            }
            ctx.info("Tip: Use `/assign <num> @user` or `/reviewer <num> set @user`");
            true
        }
        Err(e) => {
            ctx.err(e);
            false
        }
    }
}

async fn print_user_hints(ctx: &Ctx<'_>, owner: &str, repo: &str) {
    if let Ok(users) = collect_repo_users(ctx, owner, repo).await {
        if !users.is_empty() {
            let formatted: Vec<String> = users
                .iter()
                .take(20)
                .map(|(login, tags)| {
                    if tags.iter().any(|t| t == "Reviewer Pool") {
                        format!("@{login} (pool)")
                    } else {
                        format!("@{login}")
                    }
                })
                .collect();
            ctx.info(format!(
                "Available GitHub users in {}: {}",
                ctx.repo,
                formatted.join(", ")
            ));
        }
    }
}

/// `/reviewer <num> [set|add|remove] [+|-]user...` and `/unreviewer <num> user...`
async fn run_reviewer(ctx: &Ctx<'_>, cmd: &str, args: &[String], owner: &str, repo: &str) -> bool {
    use crate::triage::reviewers::ReviewerLoadBalancer;

    let Some(num) = args
        .first()
        .and_then(|a| a.trim_start_matches('#').parse::<u64>().ok())
    else {
        ctx.err(
            "Usage: /reviewer <pr-num> [set|+|-] <@username...>  (e.g. `/reviewer 12 set @alice`)",
        );
        print_user_hints(ctx, owner, repo).await;
        return false;
    };
    let mut rest: Vec<String> = args[1..].to_vec();

    // Optional sub-command.
    let mut mode = if matches!(cmd, "reassign" | "rereview") {
        "set"
    } else if matches!(cmd, "unreviewer" | "unreviewers") {
        "remove"
    } else {
        "edit"
    };
    if matches!(cmd, "reviewer" | "reviewers" | "review") {
        if let Some(first) = rest.first().map(|s| s.to_ascii_lowercase()) {
            match first.as_str() {
                "set" | "replace" | "change" | "swap" => {
                    mode = "set";
                    rest.remove(0);
                }
                "add" => {
                    rest.remove(0);
                }
                "remove" | "rm" | "del" => {
                    mode = "remove";
                    rest.remove(0);
                }
                _ => {}
            }
        }
    }

    let clean = |s: &str| {
        s.trim_start_matches(|c| c == '+' || c == '-')
            .trim_start_matches('@')
            .to_string()
    };
    let mut add: Vec<String> = Vec::new();
    let mut remove: Vec<String> = Vec::new();
    for tok in &rest {
        let user = clean(tok);
        if user.is_empty() {
            continue;
        }
        match mode {
            "remove" => remove.push(user),
            "set" => add.push(user),
            _ => {
                if tok.starts_with('-') {
                    remove.push(user);
                } else {
                    add.push(user);
                }
            }
        }
    }

    let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
        Ok(c) => c,
        Err(e) => {
            ctx.err(format!("Failed to get installation token: {e}"));
            return false;
        }
    };
    let pr = match client.get_pull_request(owner, repo, num).await {
        Ok(p) => p,
        Err(e) => {
            ctx.err(format!(
                "#{num} is not a pull request or could not be fetched: {e}"
            ));
            return false;
        }
    };
    let current: Vec<String> = pr
        .requested_reviewers
        .iter()
        .map(|u| u.login.clone())
        .collect();

    // No users given: show the current reviewers and available user hints.
    if add.is_empty() && remove.is_empty() {
        if mode == "set" {
            ctx.err(format!(
                "Specify the new reviewer(s), e.g. `/reviewer {num} set @alice @bob`"
            ));
            print_user_hints(ctx, owner, repo).await;
            return false;
        }
        if current.is_empty() {
            ctx.info(format!("{}#{num} has no requested reviewers", ctx.repo));
        } else {
            ctx.ok(format!(
                "{}#{num} requested reviewers: {}",
                ctx.repo,
                current
                    .iter()
                    .map(|u| format!("@{u}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        print_user_hints(ctx, owner, repo).await;
        return true;
    }

    let has = |list: &[String], u: &str| list.iter().any(|x| x.eq_ignore_ascii_case(u));
    if mode == "set" {
        remove = current.iter().filter(|c| !has(&add, c)).cloned().collect();
    }
    add.retain(|u| !has(&current, u) && !u.eq_ignore_ascii_case(&pr.user.login));
    add.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    remove.retain(|u| has(&current, u));

    if add.is_empty() && remove.is_empty() {
        ctx.info("Nothing to change: requested reviewers already match");
        return true;
    }

    let repo_config = ctx
        .state
        .triage
        .load_repo_config(&client, owner, repo)
        .await;
    let balancer = ReviewerLoadBalancer::new(
        repo_config.reviewers.clone(),
        ctx.state.triage.storage().clone(),
    );

    if !remove.is_empty() {
        if let Err(e) = client.remove_reviewers(owner, repo, num, &remove).await {
            ctx.err(format!("Failed to remove reviewers: {e}"));
            return false;
        }
        for r in &remove {
            let _ = balancer.on_review_completed_or_closed(ctx.repo, r);
        }
        ctx.ok(format!("Removed reviewers: {}", remove.join(", ")));
    }
    if !add.is_empty() {
        if let Err(e) = client.request_reviewers(owner, repo, num, &add).await {
            ctx.err(format!("Failed to request reviewers: {e}"));
            return false;
        }
        for r in &add {
            let _ = balancer.on_review_requested(ctx.repo, r);
        }
        ctx.ok(format!("Requested reviewers: {}", add.join(", ")));
    }
    let body = format!(
        "🔄 **Reviewers changed by @{}** (via admin console){}{}",
        ctx.user.username,
        if remove.is_empty() {
            String::new()
        } else {
            format!(
                "\n- Removed: {}",
                remove
                    .iter()
                    .map(|u| format!("@{u}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
        if add.is_empty() {
            String::new()
        } else {
            format!(
                "\n- Requested: {}",
                add.iter()
                    .map(|u| format!("@{u}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
    );
    let _ = client.create_comment(owner, repo, num, &body).await;
    true
}

// ------------------------------------------------------------- inspection

fn fmt_ts(ts: chrono::DateTime<chrono::Utc>) -> String {
    ts.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn join_names(items: &[String]) -> String {
    if items.is_empty() {
        "—".to_string()
    } else {
        items.join(", ")
    }
}

/// Shared client lookup: every command below needs an installation token for the
/// selected repository, and the failure message is the same.
/// Parse `[count] [page]` for `/issues` and `/prs`.
///
/// Rejecting junk is better than silently listing the default page: a typo in
/// `/issues 5O` should say so, not pretend the user asked for ten.
fn parse_list_args(cmd: &str, args: &[String]) -> Result<(usize, usize), String> {
    if args.len() > 2 {
        return Err(format!(
            "Too many arguments. Usage: `/{cmd} [count] [page]`, e.g. `/{cmd} 50 2`"
        ));
    }
    let count = match args.first() {
        None => DEFAULT_LIST_COUNT,
        Some(raw) => match raw.parse::<usize>() {
            Ok(0) => return Err("Count must be at least 1".to_string()),
            Ok(n) => n.min(LIST_PAGE_SIZE as usize),
            Err(_) => {
                return Err(format!(
                    "`{raw}` is not a count. Usage: `/{cmd} [count] [page]`, e.g. `/{cmd} 50 2`"
                ))
            }
        },
    };
    let page = match args.get(1) {
        None => 1,
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => return Err(format!("`{raw}` is not a page number (pages start at 1)")),
        },
    };
    Ok((count, page))
}

/// `/issues [count] [page]` — open issues, newest first.
///
/// The REST issues endpoint returns issues **and** pull requests interleaved, so
/// page N of the raw API is not page N of the issues. The window is filled by
/// walking raw pages until it is covered, bounded by `MAX_RAW_PAGES`.
async fn run_issue_list(ctx: &Ctx<'_>, owner: &str, repo: &str, args: &[String]) -> bool {
    let (count, page) = match parse_list_args("issues", args) {
        Ok(v) => v,
        Err(msg) => {
            ctx.err(msg);
            return false;
        }
    };
    let Some(client) = client_for(ctx).await else {
        return false;
    };

    // The list endpoint is paginated, so its length is not the total: ask for
    // the exact counter instead.
    let total = match tokio::join!(
        client.count_open_issues_and_prs(owner, repo),
        client.count_open_pulls(owner, repo)
    ) {
        (Ok(all), Ok(prs)) => Some(all.saturating_sub(prs)),
        _ => None,
    };

    let offset = (page - 1).saturating_mul(count);
    let want = offset + count;
    let mut issues = Vec::new();
    let mut exhausted = false;
    let mut raw_page = 1;
    while issues.len() < want && raw_page <= MAX_RAW_PAGES {
        match client
            .list_repo_issues_page(owner, repo, "open", LIST_PAGE_SIZE, raw_page)
            .await
        {
            Ok(batch) => {
                // A short page means there is nothing after it.
                let short_page = batch.len() < LIST_PAGE_SIZE as usize;
                issues.extend(batch.into_iter().filter(|i| !i.is_pr()));
                if short_page {
                    exhausted = true;
                    break;
                }
            }
            Err(e) => {
                ctx.err(format!("Failed to fetch issues: {e}"));
                return false;
            }
        }
        raw_page += 1;
    }
    let hit_cap = issues.len() < want && raw_page > MAX_RAW_PAGES;

    let start = offset.min(issues.len());
    let end = (offset + count).min(issues.len());
    if start == end {
        if hit_cap {
            // We stopped walking before the requested window, so "past the end"
            // would be a lie: the items exist, this command just will not reach
            // them in one go.
            ctx.err(format!(
                "Page {page} is past the {MAX_RAW_PAGES} API pages this command walks. Try a closer page: `/issues {count} {}`.",
                (issues.len() / count).max(1)
            ));
        } else {
            match total {
                Some(0) => ctx.ok("No open issues."),
                _ => ctx.err(format!(
                    "Page {page} is past the end — {count} per page, only {} issue(s) in this repository.",
                    issues.len()
                )),
            }
        }
        return true;
    }

    let (first, last) = (start + 1, end);
    match total {
        Some(n) => ctx.ok(format!("{n} open issues · showing {first}–{last}")),
        None => ctx.ok(format!("Open issues · showing {first}–{last}")),
    }
    for i in &issues[start..end] {
        ctx.out(format!(
            "#{:<5} {:<60} @{}  [{}]",
            i.number,
            truncate(&i.title, 58),
            i.user.login,
            i.labels
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let more = match total {
        Some(n) => (last as u64) < n,
        None => !exhausted,
    };
    if more {
        ctx.info(format!("Next page: `/issues {count} {}`", page + 1));
    }
    if hit_cap {
        ctx.info(format!(
            "Stopped after {MAX_RAW_PAGES} API pages; start from a later page or narrow the count to go further."
        ));
    }
    true
}

/// `/prs [count] [page]` — open pull requests, newest first. No issue/PR mixing
/// here, so the requested page maps straight onto the API page.
async fn run_pr_list(ctx: &Ctx<'_>, owner: &str, repo: &str, args: &[String]) -> bool {
    let (count, page) = match parse_list_args("prs", args) {
        Ok(v) => v,
        Err(msg) => {
            ctx.err(msg);
            return false;
        }
    };
    let Some(client) = client_for(ctx).await else {
        return false;
    };

    let total = client.count_open_pulls(owner, repo).await.ok();
    let offset = (page - 1).saturating_mul(count);

    let list = match client
        .list_repo_pulls_page(owner, repo, "open", count as u32, page as u32)
        .await
    {
        Ok(l) => l,
        Err(e) => {
            ctx.err(format!("Failed to fetch PRs: {e}"));
            return false;
        }
    };

    if list.is_empty() {
        match total {
            Some(0) => ctx.ok("No open PRs."),
            _ => ctx.err(format!("Page {page} is past the end — {count} per page.")),
        }
        return true;
    }

    let (first, last) = (offset + 1, offset + list.len());
    match total {
        Some(n) => ctx.ok(format!("{n} open PRs · showing {first}–{last}")),
        None => ctx.ok(format!("Open PRs · showing {first}–{last}")),
    }
    for p in &list {
        ctx.out(format!(
            "#{:<5} {:<58} @{}{}",
            p.number,
            truncate(&p.title, 56),
            p.user.login,
            if p.draft { "  [draft]" } else { "" }
        ));
    }
    if list.len() == count {
        ctx.info(format!("Next page: `/prs {count} {}`", page + 1));
    }
    true
}

async fn client_for<'a>(ctx: &Ctx<'a>) -> Option<crate::github::client::GitHubClient> {
    match ctx.state.triage.get_client_for_repo(ctx.repo).await {
        Ok(c) => Some(c),
        Err(e) => {
            ctx.err(format!("Failed to get installation token: {e}"));
            None
        }
    }
}

/// One issue or PR, with the fields an operator needs before acting on it.
async fn run_issue_detail(ctx: &Ctx<'_>, owner: &str, repo: &str, args: &[String]) -> bool {
    let Some(num) = ctx.need_number(args) else {
        return false;
    };
    let Some(client) = client_for(ctx).await else {
        return false;
    };
    let issue = match client.get_issue(owner, repo, num).await {
        Ok(i) => i,
        Err(e) => {
            ctx.err(format!("Failed to load #{num}: {e}"));
            return false;
        }
    };

    ctx.out(format!("#{} {}", issue.number, issue.title));
    ctx.out(format!(
        "state     {} · author @{} · {} comments",
        issue.state,
        issue.user.login,
        issue.comments.unwrap_or(0)
    ));
    ctx.out(format!(
        "created   {} · updated {}",
        fmt_ts(issue.created_at),
        fmt_ts(issue.updated_at)
    ));
    let labels: Vec<String> = issue.labels.iter().map(|l| l.name.clone()).collect();
    ctx.out(format!("labels    {}", join_names(&labels)));
    let assignees: Vec<String> = issue
        .assignees
        .iter()
        .map(|u| format!("@{}", u.login))
        .collect();
    ctx.out(format!("assignees {}", join_names(&assignees)));
    ctx.out(format!(
        "milestone {}",
        issue
            .milestone
            .as_ref()
            .map(|m| m.title.clone())
            .unwrap_or_else(|| "—".to_string())
    ));

    if issue.is_pr() {
        match client.get_pull_request(owner, repo, num).await {
            Ok(pr) => {
                ctx.out(format!("branch    {} → {}", pr.head.r#ref, pr.base.r#ref));
                ctx.out(format!(
                    "changes   +{} −{} across {} files",
                    pr.additions.unwrap_or(0),
                    pr.deletions.unwrap_or(0),
                    pr.changed_files.unwrap_or(0)
                ));
                let reviewers: Vec<String> = pr
                    .requested_reviewers
                    .iter()
                    .map(|u| format!("@{}", u.login))
                    .collect();
                ctx.out(format!("reviewers {}", join_names(&reviewers)));
                if pr.draft {
                    ctx.out("draft     yes");
                }
                if pr.merged {
                    ctx.out(format!(
                        "merged    {}",
                        pr.merged_at
                            .map(fmt_ts)
                            .unwrap_or_else(|| "yes".to_string())
                    ));
                }
                report_checks(ctx, &client, owner, repo, &pr.head.sha).await;
            }
            Err(e) => ctx.err(format!("Pull request details unavailable: {e}")),
        }
    }
    true
}

/// CI status for a PR head commit. Returns the number of failing suites.
async fn report_checks(
    ctx: &Ctx<'_>,
    client: &crate::github::client::GitHubClient,
    owner: &str,
    repo: &str,
    sha: &str,
) -> usize {
    let suites = match client.list_check_suites_for_ref(owner, repo, sha).await {
        Ok(s) => s,
        Err(e) => {
            ctx.err(format!("Failed to read check suites: {e}"));
            return 0;
        }
    };
    if suites.is_empty() {
        ctx.info("checks    no check suites reported for this commit");
        return 0;
    }
    let mut failing = 0usize;
    let mut pending = 0usize;
    for s in &suites {
        let app = s
            .get("app")
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown");
        let status = s
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let conclusion = s.get("conclusion").and_then(|v| v.as_str()).unwrap_or("—");
        if status != "completed" {
            pending += 1;
        } else if matches!(
            conclusion,
            "failure" | "timed_out" | "cancelled" | "action_required"
        ) {
            failing += 1;
        }
        ctx.out(format!("  {app:<20} {status:<12} {conclusion}"));
    }
    ctx.out(format!(
        "checks    {} suites · {failing} failing · {pending} pending",
        suites.len()
    ));
    failing
}

async fn run_checks(ctx: &Ctx<'_>, owner: &str, repo: &str, args: &[String]) -> bool {
    let Some(num) = ctx.need_number(args) else {
        return false;
    };
    let Some(client) = client_for(ctx).await else {
        return false;
    };
    let pr = match client.get_pull_request(owner, repo, num).await {
        Ok(p) => p,
        Err(e) => {
            ctx.err(format!(
                "#{num} is not a pull request, or could not be read: {e}"
            ));
            return false;
        }
    };
    ctx.info(format!(
        "Checks for #{num} at {} ({} → {})",
        &pr.head.sha[..7.min(pr.head.sha.len())],
        pr.head.r#ref,
        pr.base.r#ref
    ));
    report_checks(ctx, &client, owner, repo, &pr.head.sha).await;
    true
}

async fn run_labels(ctx: &Ctx<'_>, owner: &str, repo: &str) -> bool {
    let Some(client) = client_for(ctx).await else {
        return false;
    };
    match client.list_labels(owner, repo).await {
        Ok(labels) => {
            ctx.ok(format!("{} labels in {}", labels.len(), ctx.repo));
            for l in &labels {
                let desc = l.description.clone().unwrap_or_default();
                if desc.is_empty() {
                    ctx.out(format!("  {}", l.name));
                } else {
                    ctx.out(format!("  {:<28} {}", l.name, truncate(&desc, 60)));
                }
            }
            true
        }
        Err(e) => {
            ctx.err(format!("Failed to list labels: {e}"));
            false
        }
    }
}

async fn run_milestones(ctx: &Ctx<'_>, owner: &str, repo: &str) -> bool {
    let Some(client) = client_for(ctx).await else {
        return false;
    };
    match client.list_milestones(owner, repo).await {
        Ok(list) => {
            if list.is_empty() {
                ctx.info("No open milestones");
                return true;
            }
            ctx.ok(format!("{} open milestones", list.len()));
            for m in &list {
                ctx.out(format!(
                    "  #{:<3} {:<40} {}/{} closed{}",
                    m.number,
                    truncate(&m.title, 38),
                    m.closed_issues.unwrap_or(0),
                    m.closed_issues.unwrap_or(0) + m.open_issues.unwrap_or(0),
                    m.due_on
                        .map(|d| format!(" · due {}", d.format("%Y-%m-%d")))
                        .unwrap_or_default()
                ));
            }
            true
        }
        Err(e) => {
            ctx.err(format!("Failed to list milestones: {e}"));
            false
        }
    }
}

async fn run_rate(ctx: &Ctx<'_>) -> bool {
    let Some(client) = client_for(ctx).await else {
        return false;
    };
    match client.rate_limit().await {
        Ok(v) => {
            let res = v.get("resources").cloned().unwrap_or(serde_json::json!({}));
            let mut any = false;
            for name in ["core", "graphql", "search"] {
                let Some(bucket) = res.get(name) else {
                    continue;
                };
                any = true;
                let remaining = bucket
                    .get("remaining")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let limit = bucket.get("limit").and_then(|n| n.as_u64()).unwrap_or(0);
                let reset = bucket
                    .get("reset")
                    .and_then(|n| n.as_i64())
                    .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                    .map(|d| d.format("%H:%M UTC").to_string())
                    .unwrap_or_else(|| "—".to_string());
                let level = if remaining == 0 { "err" } else { "out" };
                ctx.state.jobs.log(
                    ctx.job,
                    level,
                    format!("{name:<8} {remaining:>6} / {limit:<6} resets {reset}"),
                );
            }
            if !any {
                ctx.err("GitHub returned no rate-limit buckets");
                return false;
            }
            true
        }
        Err(e) => {
            ctx.err(format!("Failed to read rate limit: {e}"));
            false
        }
    }
}

// -------------------------------------------------------------- mutations

async fn run_title(ctx: &Ctx<'_>, owner: &str, repo: &str, args: &[String]) -> bool {
    let Some(num) = ctx.need_number(args) else {
        return false;
    };
    let title = args[1..].join(" ").trim().to_string();
    if title.is_empty() {
        ctx.err("Usage: `/title 42 New title for the issue`");
        return false;
    }
    if title.chars().count() > 256 {
        ctx.err("Titles are limited to 256 characters");
        return false;
    }
    let Some(client) = client_for(ctx).await else {
        return false;
    };
    match client.set_issue_title(owner, repo, num, &title).await {
        Ok(()) => {
            ctx.ok(format!("#{num} renamed to “{title}”"));
            true
        }
        Err(e) => {
            ctx.err(format!("Failed to rename #{num}: {e}"));
            false
        }
    }
}

async fn run_milestone(ctx: &Ctx<'_>, owner: &str, repo: &str, args: &[String]) -> bool {
    let Some(num) = ctx.need_number(args) else {
        return false;
    };
    let Some(wanted) = args.get(1).map(|s| s.trim().to_string()) else {
        ctx.err("Usage: `/milestone 42 v1.4` or `/milestone 42 none` to clear");
        return false;
    };
    let Some(client) = client_for(ctx).await else {
        return false;
    };

    if wanted.eq_ignore_ascii_case("none") || wanted.eq_ignore_ascii_case("clear") || wanted == "-"
    {
        return match client.set_issue_milestone(owner, repo, num, None).await {
            Ok(()) => {
                ctx.ok(format!("Cleared the milestone on #{num}"));
                true
            }
            Err(e) => {
                ctx.err(format!("Failed to clear the milestone: {e}"));
                false
            }
        };
    }

    let milestones = match client.list_milestones(owner, repo).await {
        Ok(m) => m,
        Err(e) => {
            ctx.err(format!("Failed to list milestones: {e}"));
            return false;
        }
    };
    // Accept either the milestone title (case-insensitive) or its number.
    let found = milestones
        .iter()
        .find(|m| m.title.eq_ignore_ascii_case(&wanted))
        .or_else(|| {
            wanted
                .trim_start_matches('#')
                .parse::<u64>()
                .ok()
                .and_then(|n| milestones.iter().find(|m| m.number == n))
        });

    let Some(m) = found else {
        ctx.err(format!("No open milestone matches “{wanted}”. Available:"));
        for m in milestones.iter().take(20) {
            ctx.out(format!("  #{} {}", m.number, m.title));
        }
        return false;
    };

    match client
        .set_issue_milestone(owner, repo, num, Some(m.number))
        .await
    {
        Ok(()) => {
            ctx.ok(format!("#{num} moved to milestone “{}”", m.title));
            true
        }
        Err(e) => {
            ctx.err(format!("Failed to set the milestone: {e}"));
            false
        }
    }
}

/// `lock` and `unlock` share the argument parsing: only `lock` takes a reason.
async fn run_lock(ctx: &Ctx<'_>, owner: &str, repo: &str, args: &[String], lock: bool) -> bool {
    let Some(num) = ctx.need_number(args) else {
        return false;
    };
    // Validate the reason before touching GitHub: a typo should not cost an API
    // round-trip (or an installation-token lookup).
    let reason = match args.get(1).map(|r| r.trim().to_ascii_lowercase()) {
        None => None,
        Some(_) if !lock => {
            ctx.err("/unlock takes no arguments besides the issue number");
            return false;
        }
        Some(r) => match r.as_str() {
            "off-topic" | "offtopic" => Some("off-topic"),
            "too heated" | "heated" | "too-heated" => Some("too heated"),
            "resolved" => Some("resolved"),
            "spam" => Some("spam"),
            other => {
                ctx.err(format!(
                    "Unknown reason “{other}”. Use off-topic, too heated, resolved or spam"
                ));
                return false;
            }
        },
    };
    let Some(client) = client_for(ctx).await else {
        return false;
    };

    if !lock {
        return match client.unlock_issue(owner, repo, num).await {
            Ok(()) => {
                ctx.ok(format!("Unlocked the conversation on #{num}"));
                true
            }
            Err(e) => {
                ctx.err(format!("Failed to unlock #{num}: {e}"));
                false
            }
        };
    }

    match client.lock_issue(owner, repo, num, reason).await {
        Ok(()) => {
            ctx.ok(format!(
                "Locked the conversation on #{num}{}",
                reason.map(|r| format!(" ({r})")).unwrap_or_default()
            ));
            true
        }
        Err(e) => {
            ctx.err(format!("Failed to lock #{num}: {e}"));
            false
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn human_duration(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d{}h", secs / 86400, (secs % 86400) / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_job_is_not_relabelled_by_a_late_finish() {
        let jobs = JobRegistry::default();
        let id = jobs.create("alice", "o/r", "/stale");
        assert!(jobs.cancel(&id).is_some());
        // The aborted task may still reach `finish` before the abort lands.
        jobs.finish(&id, true);
        let job = jobs.snapshot(&id).expect("job");
        assert_eq!(job.status, JobStatus::Cancelled);
        assert!(job.abort.is_none(), "the abort handle is consumed once");
        assert!(jobs.cancel(&id).is_none(), "a second cancel is a no-op");
    }

    #[test]
    fn cancel_leaves_finished_jobs_alone() {
        let jobs = JobRegistry::default();
        let id = jobs.create("bob", "o/r", "/issues");
        jobs.finish(&id, false);
        assert!(jobs.cancel(&id).is_none());
        assert_eq!(jobs.snapshot(&id).expect("job").status, JobStatus::Failed);
    }

    #[test]
    fn list_args_defaults_bounds_and_junk() {
        let none: Vec<String> = Vec::new();
        assert_eq!(parse_list_args("issues", &none), Ok((10, 1)));
        assert_eq!(parse_list_args("issues", &["50".into()]), Ok((50, 1)));
        assert_eq!(
            parse_list_args("issues", &["50".into(), "3".into()]),
            Ok((50, 3))
        );
        // GitHub caps per_page at 100, so a larger request is clamped.
        assert_eq!(parse_list_args("prs", &["500".into()]), Ok((100, 1)));
        // Junk is rejected instead of silently falling back to the default.
        assert!(parse_list_args("issues", &["5O".into()]).is_err());
        assert!(parse_list_args("issues", &["0".into()]).is_err());
        assert!(parse_list_args("issues", &["10".into(), "0".into()]).is_err());
        assert!(parse_list_args("issues", &["10".into(), "2".into(), "x".into()]).is_err());
    }
}

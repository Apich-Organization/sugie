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

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
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
}

#[derive(Default)]
pub struct JobRegistry {
    inner: Mutex<(HashMap<String, Job>, VecDeque<String>)>,
}

impl JobRegistry {
    pub fn create(&self, username: &str, repo: &str, command: &str) -> String {
        let id = crate::web::auth::random_token().chars().take(12).collect::<String>();
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
        };
        if let Ok(mut guard) = self.inner.lock() {
            let (jobs, order) = &mut *guard;
            jobs.insert(id.clone(), job);
            order.push_back(id.clone());
            while order.len() > MAX_JOBS {
                if let Some(old) = order.pop_front() {
                    jobs.remove(&old);
                }
            }
        }
        id
    }

    pub fn log(&self, id: &str, level: &str, text: impl Into<String>) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(job) = guard.0.get_mut(id) {
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
                job.status = if ok { JobStatus::Done } else { JobStatus::Failed };
                job.finished_at = Some(clock());
            }
        }
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
        let mut jobs: Vec<Job> = guard
            .1
            .iter()
            .filter_map(|id| guard.0.get(id))
            .filter(|j| j.username.eq_ignore_ascii_case(username))
            .cloned()
            .collect();
        jobs.reverse();
        jobs.truncate(limit);
        jobs
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
) -> Result<String, String> {
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return Err("Command cannot be empty".to_string());
    }
    let (cmd, args) = split_command(&raw);
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
    tokio::spawn(async move {
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
            "-",
            ok,
        );
    });

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
        match args.first().and_then(|a| a.trim_start_matches('#').parse::<u64>().ok()) {
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
            ctx.out(format!("Configuration: {}", ctx.state.files.toml_path.display()));
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
            ctx.out(format!("open issues {} · open PRs {} · 30d commits {} · 30d merged {}", s.open_issues, s.open_prs, s.commits_30d, s.merged_30d));
            ctx.out(format!("stars {} · forks {} · default branch {}", s.stars, s.forks, s.default_branch));
            s.is_ok()
        }

        "issues" | "prs" => {
            let limit = args
                .first()
                .and_then(|a| a.parse::<usize>().ok())
                .unwrap_or(10)
                .min(50);
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            if cmd == "issues" {
                match client.list_repo_issues(owner, repo, "open", 100).await {
                    Ok(list) => {
                        let issues: Vec<_> = list.into_iter().filter(|i| !i.is_pr()).collect();
                        ctx.ok(format!("{} open issues, showing first {}", issues.len(), limit));
                        for i in issues.iter().take(limit) {
                            ctx.out(format!(
                                "#{:<5} {:<60} @{}  [{}]",
                                i.number,
                                truncate(&i.title, 58),
                                i.user.login,
                                i.labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")
                            ));
                        }
                    }
                    Err(e) => {
                        ctx.err(format!("Failed to fetch issues: {e}"));
                        return false;
                    }
                }
            } else {
                match client.list_repo_pulls(owner, repo, "open", 100).await {
                    Ok(list) => {
                        ctx.ok(format!("{} open PRs, showing first {}", list.len(), limit));
                        for p in list.iter().take(limit) {
                            ctx.out(format!(
                                "#{:<5} {:<58} @{}{}",
                                p.number,
                                truncate(&p.title, 56),
                                p.user.login,
                                if p.draft { "  [draft]" } else { "" }
                            ));
                        }
                    }
                    Err(e) => {
                        ctx.err(format!("Failed to fetch PRs: {e}"));
                        return false;
                    }
                }
            }
            true
        }

        "label" | "unlabel" => {
            let Some(num) = ctx.need_number(args) else { return false };
            let labels: Vec<String> = args[1..].iter().cloned().collect();
            if labels.is_empty() {
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
            let res = if cmd == "label" {
                client.add_labels(owner, repo, num, &labels).await
            } else {
                let mut r = Ok(());
                for l in &labels {
                    r = client.remove_label(owner, repo, num, l).await;
                    if r.is_err() {
                        break;
                    }
                }
                r
            };
            match res {
                Ok(_) => {
                    ctx.ok(format!(
                        "{} labels on {}{}: {}",
                        if cmd == "label" { "Added" } else { "Removed" },
                        ctx.repo,
                        format!("#{num}"),
                        labels.join(", ")
                    ));
                    true
                }
                Err(e) => {
                    ctx.err(format!("Operation failed: {e}"));
                    false
                }
            }
        }

        "assign" | "unassign" => {
            let Some(num) = ctx.need_number(args) else { return false };
            let users: Vec<String> = args[1..]
                .iter()
                .map(|u| u.trim_start_matches('@').to_string())
                .filter(|u| !u.is_empty())
                .collect();
            if users.is_empty() {
                ctx.err("Please specify username(s), e.g. `/assign 42 octocat`");
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
                        if cmd == "assign" { "Assigned" } else { "Unassigned" },
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

        "close" | "reopen" => {
            let Some(num) = ctx.need_number(args) else { return false };
            let client = match ctx.state.triage.get_client_for_repo(ctx.repo).await {
                Ok(c) => c,
                Err(e) => {
                    ctx.err(format!("Failed to get installation token: {e}"));
                    return false;
                }
            };
            let res = if cmd == "close" {
                let reason = match args.get(1).map(|s| s.as_str()) {
                    Some("not_planned") => Some("not_planned"),
                    Some("completed") => Some("completed"),
                    _ => Some("completed"),
                };
                client.update_issue_state(owner, repo, num, "closed", reason).await
            } else {
                client.update_issue_state(owner, repo, num, "open", Some("reopened")).await
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
            let Some(num) = ctx.need_number(args) else { return false };
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

        "retest" => {
            let Some(num) = ctx.need_number(args) else { return false };
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
                ctx.err(format!("No check suites available to re-run on {}{}", ctx.repo, format!("#{num}")));
                return false;
            }
            let mut rerun = 0;
            for suite in &suites {
                let Some(id) = suite.get("id").and_then(|v| v.as_u64()) else {
                    continue;
                };
                if client.rerequest_check_suite(owner, repo, id).await.is_ok() {
                    rerun += 1;
                }
            }
            ctx.ok(format!("Re-triggered {rerun} check suites ({} head {})", ctx.repo, &pr.head.sha[..7.min(pr.head.sha.len())]));
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
            match reporter.generate_report(&client, owner, repo, days, label).await {
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
                        ctx.ok(format!("{} validation passed (listen {}:{})", path.display(), cfg.host, cfg.port));
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

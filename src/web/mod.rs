//! Admin console: Leptos SSR pages mounted at `/admin` inside the bot's own
//! Axum server, plus a small JSON/SSE API for the command console.

pub mod assets;
pub mod auth;
pub mod config_store;
pub mod db;
pub mod exec;
pub mod perms;
pub mod stats;
pub mod ui;

use crate::config::SugieConfig;
use crate::triage::TriageService;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, Sse},
        Html, IntoResponse, Json, Redirect, Response,
    },
    routing::{get, post},
    Form, Router,
};
use chrono::{DateTime, Utc};
use config_store::{ConfigFiles, ConfigSummary};
use db::WebDb;
use exec::{JobRegistry, JobStatus};
use leptos::prelude::*;
use perms::CurrentUser;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use stats::StatsCache;
use ui::pages;

#[derive(Clone)]
pub struct WebState {
    pub config: Arc<SugieConfig>,
    pub triage: TriageService,
    pub db: WebDb,
    pub files: ConfigFiles,
    pub jobs: Arc<JobRegistry>,
    pub stats: Arc<StatsCache>,
    pub shutdown: Arc<tokio::sync::Notify>,
    pub started_at: DateTime<Utc>,
}

impl WebState {
    pub fn new(
        config: Arc<SugieConfig>,
        triage: TriageService,
        files: ConfigFiles,
        shutdown: Arc<tokio::sync::Notify>,
    ) -> anyhow::Result<Self> {
        let db = WebDb::open(&config.db_path)?;
        if let Some(generated) = auth::bootstrap_admin(&db)? {
            tracing::warn!(
                "Admin console bootstrapped an `admin` account. Generated password: {} (also written to data/admin-initial-password.txt)",
                generated
            );
        }
        Ok(Self {
            config,
            triage,
            db,
            files,
            jobs: Arc::new(JobRegistry::default()),
            stats: Arc::new(StatsCache::default()),
            shutdown,
            started_at: Utc::now(),
        })
    }
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/admin", get(dashboard))
        .route("/admin/", get(dashboard))
        .route("/admin/login", get(login_page).post(login_submit))
        .route("/admin/logout", post(logout))
        .route("/admin/repos", get(repos_page))
        .route("/admin/api/repos/sync", post(sync_repos))
        .route("/admin/chat", get(chat_page))
        .route("/admin/config", get(config_page))
        .route("/admin/users", get(users_page))
        .route("/admin/audit", get(audit_page))
        .route("/admin/api/exec", post(api_exec))
        .route("/admin/api/jobs/{id}", get(api_job))
        .route("/admin/api/jobs/{id}/stream", get(api_job_stream))
        .route("/admin/api/stats", get(api_stats))
        .route("/admin/config/structured", post(save_structured))
        .route("/admin/config/toml", post(save_toml))
        .route("/admin/config/env", post(save_env))
        .route("/admin/config/restore", post(restore_backup))
        .route("/admin/users/create", post(create_user))
        .route("/admin/users/update", post(update_user))
        .route("/admin/users/delete", post(delete_user))
        .route("/admin/system/restart", post(restart))
        .route("/admin/assets/app.css", get(css))
        .route("/admin/assets/app.js", get(js))
        .route("/admin/assets/logo.svg", get(logo_svg))
        .route("/admin/assets/sugie-mark.svg", get(logo_svg))
        .route("/favicon.ico", get(logo_svg))
        .with_state(state)
}

// ------------------------------------------------------------------ helpers

fn html(view: impl IntoView + 'static) -> Response {
    Html(ui::render(view)).into_response()
}

fn redirect(to: &str) -> Response {
    Redirect::to(to).into_response()
}

/// Every page handler funnels through this: anonymous callers get bounced to the
/// login form with a `next` hint.
fn gate(state: &WebState, headers: &HeaderMap) -> Result<CurrentUser, Response> {
    match auth::current_user(&state.db, headers) {
        Some(user) => Ok(user),
        None => Err(redirect("/admin/login")),
    }
}

fn gate_admin(state: &WebState, headers: &HeaderMap) -> Result<CurrentUser, Response> {
    let user = gate(state, headers)?;
    if user.can_manage() {
        Ok(user)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Html("<h1>403 Forbidden</h1><p>Admin privileges required.</p>".to_string()),
        )
            .into_response())
    }
}

fn flash_from(query: &HashMap<String, String>) -> Option<(bool, String)> {
    let msg = query.get("msg")?.clone();
    let ok = query.get("ok").map(|v| v == "1").unwrap_or(true);
    Some((ok, msg))
}

// -------------------------------------------------------------------- pages

async fn login_page(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if auth::current_user(&state.db, &headers).is_some() {
        return redirect("/admin");
    }
    html(pages::login_page(None, "/admin".to_string()))
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    #[serde(default)]
    next: String,
}

async fn login_submit(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let ip = auth::client_ip(&headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    match auth::login(&state.db, form.username.trim(), &form.password, &ip, &ua) {
        Ok(Some(token)) => {
            state
                .db
                .audit(&form.username, "login", "-", "Login successful", &ip, true);
            let next = if form.next.starts_with("/admin") && !form.next.starts_with("/admin/login") {
                form.next
            } else {
                "/admin".to_string()
            };
            let mut resp = redirect(&next);
            if let Ok(value) = HeaderValue::from_str(&auth::session_cookie(&token)) {
                resp.headers_mut().append(header::SET_COOKIE, value);
            }
            resp
        }
        Ok(None) => {
            state
                .db
                .audit(&form.username, "login", "-", "Invalid username or password", &ip, false);
            html(pages::login_page(
                Some("Invalid username or password, or account is disabled".to_string()),
                form.next,
            ))
        }
        Err(e) => {
            tracing::error!("admin login failed: {e}");
            html(pages::login_page(Some("Login failed, please check server logs".to_string()), form.next))
        }
    }
}

async fn logout(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::cookie_value(&headers) {
        let _ = state.db.delete_session(&auth::token_hash(&token));
    }
    let mut resp = redirect("/admin/login");
    if let Ok(value) = HeaderValue::from_str(&auth::clear_cookie()) {
        resp.headers_mut().append(header::SET_COOKIE, value);
    }
    resp
}

async fn dashboard(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let user = match gate(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let mut repos = state.triage.storage().list_repositories().unwrap_or_default();
    if repos.is_empty() {
        if let Ok(synced) = state.triage.sync_all_repositories().await {
            repos = synced;
        }
    }
    if let Some(ref allowed) = state.config.notifications.allowed_repositories {
        for r in allowed {
            if !repos.iter().any(|(_, name, _)| name.eq_ignore_ascii_case(r)) {
                repos.push((0, r.clone(), 0));
            }
        }
    }

    let mut set = tokio::task::JoinSet::new();
    for (_, name, inst) in repos.iter() {
        let triage = state.triage.clone();
        let cache = state.stats.clone();
        let name = name.clone();
        let inst = *inst;
        set.spawn(async move {
            let stats = stats::fetch(&triage, &cache, &name, false).await;
            pages::RepoCard {
                full_name: name,
                installation_id: inst,
                stats: Some(stats),
            }
        });
    }

    let mut cards = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(card) = res {
            cards.push(card);
        }
    }
    cards.sort_by(|a, b| a.full_name.cmp(&b.full_name));

    let totals = pages::Totals {
        repos: repos.len(),
        issues: cards
            .iter()
            .filter_map(|c| c.stats.as_ref())
            .map(|s| s.open_issues)
            .sum(),
        prs: cards
            .iter()
            .filter_map(|c| c.stats.as_ref())
            .map(|s| s.open_prs)
            .sum(),
        commits: cards
            .iter()
            .filter_map(|c| c.stats.as_ref())
            .map(|s| s.commits_30d)
            .sum(),
    };

    let summary = ConfigSummary::from_config(&state.config);
    html(pages::dashboard_page(user, cards, totals, summary, None))
}

async fn repos_page(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let user = match gate(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let mut repos = state.triage.storage().list_repositories().unwrap_or_default();
    if repos.is_empty() {
        if let Ok(synced) = state.triage.sync_all_repositories().await {
            repos = synced;
        }
    }
    if let Some(ref allowed) = state.config.notifications.allowed_repositories {
        for r in allowed {
            if !repos.iter().any(|(_, name, _)| name.eq_ignore_ascii_case(r)) {
                repos.push((0, r.clone(), 0));
            }
        }
    }

    let mut set = tokio::task::JoinSet::new();
    // Show all connected repositories without truncation
    for (_, name, inst) in repos.iter() {
        let triage = state.triage.clone();
        let cache = state.stats.clone();
        let name = name.clone();
        let inst = *inst;
        set.spawn(async move {
            let stats = stats::fetch(&triage, &cache, &name, false).await;
            pages::RepoCard {
                full_name: name,
                installation_id: inst,
                stats: Some(stats),
            }
        });
    }
    let mut cards = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(card) = res {
            cards.push(card);
        }
    }
    cards.sort_by(|a, b| a.full_name.cmp(&b.full_name));

    html(pages::repos_page(user, cards))
}

async fn sync_repos(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let user = match gate(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let ip = auth::client_ip(&headers);
    match state.triage.sync_all_repositories().await {
        Ok(synced) => {
            state.db.audit(
                &user.username,
                "repos.sync",
                "-",
                &format!("Synced {} repositories from GitHub App installations", synced.len()),
                &ip,
                true,
            );
        }
        Err(e) => {
            state.db.audit(
                &user.username,
                "repos.sync",
                "-",
                &format!("Sync failed: {e}"),
                &ip,
                false,
            );
        }
    }
    redirect("/admin/repos")
}

#[derive(Deserialize, Default)]
struct ChatQuery {
    #[serde(default)]
    repo: String,
}

async fn chat_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<ChatQuery>,
) -> Response {
    let user = match gate(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let repos = all_repos(&state);

    let selected = if query.repo.is_empty() {
        repos.first().cloned().unwrap_or_default()
    } else {
        query.repo
    };

    let history = state.jobs.recent_for(&user.username, 20);
    html(pages::chat_page(user, repos, selected, history))
}

async fn config_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let flash = flash_from(&query);
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };

    let toml_text = state
        .files
        .read_toml()
        .unwrap_or_else(|e| format!("# Failed to read: {e}\n"));

    // The form reflects what is on disk, falling back to the running config.
    let cfg = state
        .files
        .validate_toml(&toml_text)
        .unwrap_or_else(|_| (*state.config).clone());

    let mut events = Vec::new();
    let ev = &cfg.notifications.events;
    for (name, value) in [
        ("weekly_report", ev.weekly_report),
        ("monthly_report", ev.monthly_report),
        ("assigned", ev.assigned),
        ("unassigned", ev.unassigned),
        ("issue_opened", ev.issue_opened),
        ("issue_closed", ev.issue_closed),
        ("issue_comment", ev.issue_comment),
        ("pr_opened", ev.pr_opened),
        ("pr_merged", ev.pr_merged),
        ("label_changed", ev.label_changed),
        ("stale_warned", ev.stale_warned),
        ("stale_closed", ev.stale_closed),
        ("ci_failure", ev.ci_failure),
        ("duplicate_detected", ev.duplicate_detected),
        ("dco_failed", ev.dco_failed),
        ("growth_milestone", ev.growth_milestone),
        ("cross_repo_move", ev.cross_repo_move),
    ] {
        events.push((name.to_string(), value));
    }

    let form = pages::ConfigForm {
        host: cfg.host.clone(),
        port: cfg.port.to_string(),
        db_path: cfg.db_path.clone(),
        allowed_repositories: cfg
            .notifications
            .allowed_repositories
            .clone()
            .unwrap_or_default()
            .join("\n"),
        super_users: cfg.default_rules.commands.super_users.join("\n"),
        restricted_commands: cfg.default_rules.commands.restricted_commands.join("\n"),
        stale_enabled: cfg.default_rules.stale.enabled,
        stale_days: cfg.default_rules.stale.warn_days.to_string(),
        stale_interval: cfg.default_rules.stale.check_interval_hours.to_string(),
        reviewers_enabled: cfg.default_rules.reviewers.enabled,
        reviewers_per_pr: cfg.default_rules.reviewers.reviewers_per_pr.to_string(),
        labels_enabled: cfg.default_rules.labels.enabled,
        events,
    };

    html(pages::config_page(
        user,
        form,
        toml_text,
        state.files.read_env_entries(),
        state.files.list_backups(),
        flash,
    ))
}

async fn users_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let flash = flash_from(&query);
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let users = build_user_cards(&state);
    let repos = all_repos(&state);
    html(pages::users_page(user, users, repos, flash))
}

#[derive(Deserialize, Default)]
struct PageQuery {
    #[serde(default)]
    page: i64,
}

async fn audit_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let page = query.page.max(0);
    let rows = state
        .db
        .list_audit(50, page * 50)
        .unwrap_or_default()
        .into_iter()
        .map(|r| pages::AuditCard {
            ts: r.ts.replace('T', " ").chars().take(19).collect(),
            username: r.username,
            action: r.action,
            target: r.target,
            detail: r.detail,
            ip: r.ip,
            ok: r.ok,
        })
        .collect();
    let total = state.db.count_audit().unwrap_or(0);
    html(pages::audit_page(user, rows, page, total))
}

// --------------------------------------------------------------- api: exec

#[derive(Deserialize)]
struct ExecRequest {
    repo: String,
    command: String,
}

async fn api_exec(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<ExecRequest>,
) -> Response {
    let user = match gate(&state, &headers) {
        Ok(u) => u,
        Err(_) => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response(),
    };

    match exec::dispatch(state.clone(), user, req.repo, req.command) {
        Ok(job_id) => Json(json!({ "job_id": job_id })).into_response(),
        Err(msg) => (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    }
}

async fn api_job(State(state): State<WebState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if gate(&state, &headers).is_err() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match state.jobs.snapshot(&id) {
        Some(job) => Json(job).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "Job not found"}))).into_response(),
    }
}

async fn api_job_stream(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if gate(&state, &headers).is_err() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let stream = async_stream::stream! {
        let mut sent = 0usize;
        loop {
            match state.jobs.since(&id, sent) {
                Some((lines, status)) => {
                    for line in lines {
                        sent += 1;
                        let payload = json!({"level": line.level, "text": line.text});
                        yield Ok::<Event, Infallible>(Event::default().data(payload.to_string()));
                    }
                    if status != JobStatus::Running {
                        let payload = json!({"done": true, "ok": status == JobStatus::Done});
                        yield Ok(Event::default().data(payload.to_string()));
                        break;
                    }
                }
                None => {
                    let payload = json!({"done": true, "ok": false, "text": "Job expired"});
                    yield Ok(Event::default().data(payload.to_string()));
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };

    Sse::new(stream).into_response()
}

#[derive(Deserialize, Default)]
struct StatsQuery {
    #[serde(default)]
    repo: String,
}

async fn api_stats(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<StatsQuery>,
) -> Response {
    if gate(&state, &headers).is_err() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    if query.repo.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Missing repo parameter"}))).into_response();
    }
    let stats = stats::fetch(&state.triage, &state.stats, &query.repo, true).await;
    Json(stats).into_response()
}

// -------------------------------------------------------------- api: config

async fn save_structured(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };

    let get = |k: &str| form.get(k).cloned().unwrap_or_default();
    let lines = |k: &str| -> Vec<String> {
        form.get(k)
            .map(|v| {
                v.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let int_or = |k: &str, default: i64| -> i64 {
        form.get(k).and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(default)
    };

    let text = match state.files.read_toml() {
        Ok(t) => t,
        Err(e) => return config_flash(false, &format!("Failed to read configuration: {e}")),
    };

    let mut patches: Vec<(String, config_store::Patch)> = vec![
        ("host".into(), config_store::Patch::Str(get("host"))),
        ("port".into(), config_store::Patch::Int(int_or("port", 8800))),
        ("db_path".into(), config_store::Patch::Str(get("db_path"))),
        (
            "default_rules.commands.super_users".into(),
            config_store::Patch::List(lines("super_users")),
        ),
        (
            "default_rules.commands.restricted_commands".into(),
            config_store::Patch::List(lines("restricted_commands")),
        ),
        (
            "default_rules.stale.enabled".into(),
            config_store::Patch::Bool(form.contains_key("stale_enabled")),
        ),
        (
            "default_rules.stale.warn_days".into(),
            config_store::Patch::Int(int_or("stale_days", 60)),
        ),
        (
            "default_rules.stale.check_interval_hours".into(),
            config_store::Patch::Int(int_or("stale_interval", 24)),
        ),
        (
            "default_rules.reviewers.enabled".into(),
            config_store::Patch::Bool(form.contains_key("reviewers_enabled")),
        ),
        (
            "default_rules.reviewers.reviewers_per_pr".into(),
            config_store::Patch::Int(int_or("reviewers_per_pr", 1)),
        ),
        (
            "default_rules.labels.enabled".into(),
            config_store::Patch::Bool(form.contains_key("labels_enabled")),
        ),
    ];

    let repo_filter = lines("allowed_repositories");
    patches.push((
        "notifications.allowed_repositories".into(),
        if repo_filter.is_empty() {
            config_store::Patch::Remove
        } else {
            config_store::Patch::List(repo_filter)
        },
    ));

    for key in form.keys() {
        if let Some(name) = key.strip_prefix("event_") {
            patches.push((
                format!("notifications.events.{name}"),
                config_store::Patch::Bool(true),
            ));
        }
    }
    // Events not submitted are unchecked — flip them off explicitly.
    for name in [
        "weekly_report", "monthly_report", "assigned", "unassigned", "issue_opened",
        "issue_closed", "issue_comment", "pr_opened", "pr_merged", "label_changed",
        "stale_warned", "stale_closed", "ci_failure", "duplicate_detected", "dco_failed",
        "growth_milestone", "cross_repo_move",
    ] {
        if !form.contains_key(&format!("event_{name}")) {
            patches.push((
                format!("notifications.events.{name}"),
                config_store::Patch::Bool(false),
            ));
        }
    }

    match config_store::patch_toml(&text, &patches) {
        Ok(patched) => match state.files.save_toml(&patched) {
            Ok(backup) => {
                state.db.audit(
                    &user.username,
                    "config.save",
                    "sugie.toml",
                    "Saved structured settings",
                    &auth::client_ip(&headers),
                    true,
                );
                config_flash(
                    true,
                    &format!(
                        "Saved sugie.toml{}",
                        if backup.as_os_str().is_empty() {
                            String::new()
                        } else {
                            format!(" (backup {})", backup.display())
                        }
                    ),
                )
            }
            Err(e) => {
                state.db.audit(&user.username, "config.save", "sugie.toml", &e.to_string(), "-", false);
                config_flash(false, &format!("Failed to save: {e}"))
            }
        },
        Err(e) => config_flash(false, &format!("Configuration validation failed: {e}")),
    }
}

async fn save_toml(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let content = form.get("content").cloned().unwrap_or_default();

    match state.files.save_toml(&content) {
        Ok(backup) => {
            state.db.audit(
                &user.username,
                "config.save",
                "sugie.toml",
                "Saved raw TOML",
                &auth::client_ip(&headers),
                true,
            );
            config_flash(
                true,
                &format!(
                    "Saved sugie.toml{}",
                    if backup.as_os_str().is_empty() {
                        String::new()
                    } else {
                        format!(" (backup {})", backup.display())
                    }
                ),
            )
        }
        Err(e) => {
            state.db.audit(&user.username, "config.save", "sugie.toml", &e.to_string(), "-", false);
            config_flash(false, &format!("Failed to save: {e}"))
        }
    }
}

async fn save_env(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };

    let updates: Vec<(String, String)> = form
        .iter()
        .filter(|(k, v)| !v.trim().is_empty() && k.as_str() != "content")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if updates.is_empty() {
        return config_flash(true, "No entries to update");
    }

    match state.files.save_env(&updates) {
        Ok(_) => {
            state.db.audit(
                &user.username,
                "config.save",
                ".env",
                &format!("Updated {} keys", updates.len()),
                &auth::client_ip(&headers),
                true,
            );
            config_flash(true, "Saved .env; changes will take effect after restart")
        }
        Err(e) => {
            state.db.audit(&user.username, "config.save", ".env", &e.to_string(), "-", false);
            config_flash(false, &format!("Failed to save .env: {e}"))
        }
    }
}

async fn restore_backup(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let name = form.get("name").cloned().unwrap_or_default();
    match state.files.restore_backup(&name) {
        Ok(_) => {
            state.db.audit(
                &user.username,
                "config.restore",
                &name,
                "Restore backup",
                &auth::client_ip(&headers),
                true,
            );
            config_flash(true, &format!("Restored to {name}"))
        }
        Err(e) => config_flash(false, &format!("Failed to restore backup: {e}")),
    }
}

// --------------------------------------------------------------- api: users

fn all_repos(state: &WebState) -> Vec<String> {
    let mut repos: Vec<String> = state
        .triage
        .storage()
        .list_repositories()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();
    if let Some(ref allowed) = state.config.notifications.allowed_repositories {
        for r in allowed {
            if !repos.iter().any(|existing| existing.eq_ignore_ascii_case(r)) {
                repos.push(r.clone());
            }
        }
    }
    repos.sort();
    repos
}

fn build_user_cards(state: &WebState) -> Vec<pages::UserCard> {
    state
        .db
        .list_users()
        .unwrap_or_default()
        .into_iter()
        .map(|u| pages::UserCard {
            role_label: perms::Role::parse(&u.role).label().to_string(),
            repos: state.db.repo_perms(u.id).unwrap_or_default(),
            commands: state.db.command_perms(u.id).unwrap_or_default(),
            last_login: u
                .last_login
                .map(|t| t.replace('T', " ").chars().take(16).collect())
                .unwrap_or_default(),
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            role: u.role,
            active: u.active,
        })
        .collect()
}

async fn create_user(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let actor = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let username = form.get("username").map(|s| s.trim().to_string()).unwrap_or_default();
    let password = form.get("password").cloned().unwrap_or_default();
    let role = form.get("role").cloned().unwrap_or_else(|| "viewer".into());
    let display = form.get("display_name").cloned().unwrap_or_default();

    if username.len() < 3 {
        return users_flash(false, "Username must be at least 3 characters");
    }
    if password.len() < 8 {
        return users_flash(false, "Password must be at least 8 characters");
    }
    if role == "owner" && !actor.is_owner() {
        return users_flash(false, "Only owners can create owner accounts");
    }

    let hash = match auth::hash_password(&password) {
        Ok(h) => h,
        Err(e) => return users_flash(false, &format!("Password processing failed: {e}")),
    };

    match state.db.create_user(&username, &hash, &role, &display) {
        Ok(id) => {
            let repos: Vec<String> = form
                .get("repos")
                .map(|v| v.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default();
            if !repos.is_empty() {
                let _ = state.db.set_repo_perms(id, &repos);
            }
            state.db.audit(
                &actor.username,
                "user.create",
                &username,
                &format!("role={role}"),
                &auth::client_ip(&headers),
                true,
            );
            users_flash(true, &format!("Created user {username}"))
        }
        Err(e) => users_flash(false, &format!("Failed to create user: {e}")),
    }
}

async fn update_user(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let actor = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(id) = form.get("id").and_then(|v| v.parse::<i64>().ok()) else {
        return users_flash(false, "Missing user ID");
    };
    let Some(target) = state.db.get_user(id).ok().flatten() else {
        return users_flash(false, "User not found");
    };
    if target.role == "owner" && !actor.is_owner() {
        return users_flash(false, "Only owners can modify owner accounts");
    }

    let role = form.get("role").cloned().unwrap_or_else(|| target.role.clone());
    if role == "owner" && !actor.is_owner() {
        return users_flash(false, "Only owners can grant owner role");
    }
    let display = form
        .get("display_name")
        .cloned()
        .unwrap_or_else(|| target.display_name.clone());
    let active = form.contains_key("active");

    if let Err(e) = state.db.update_user_profile(id, &role, &display, active) {
        return users_flash(false, &format!("Failed to update user: {e}"));
    }

    if let Some(repos) = form.get("repos") {
        let list: Vec<String> = repos
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let _ = state.db.set_repo_perms(id, &list);
    }
    if let Some(cmds) = form.get("commands") {
        let list: Vec<String> = cmds
            .lines()
            .map(|s| s.trim().trim_start_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let _ = state.db.set_command_perms(id, &list);
    }
    if let Some(pw) = form.get("password") {
        if !pw.is_empty() {
            if pw.len() < 8 {
                return users_flash(false, "Password must be at least 8 characters");
            }
            match auth::hash_password(pw) {
                Ok(h) => {
                    let _ = state.db.set_password(id, &h);
                }
                Err(e) => return users_flash(false, &format!("Password processing failed: {e}")),
            }
        }
    }

    state.db.audit(
        &actor.username,
        "user.update",
        &target.username,
        &format!("role={role} active={active}"),
        &auth::client_ip(&headers),
        true,
    );
    users_flash(true, &format!("Updated user {}", target.username))
}

async fn delete_user(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let actor = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(id) = form.get("id").and_then(|v| v.parse::<i64>().ok()) else {
        return users_flash(false, "Missing user ID");
    };
    if id == actor.id {
        return users_flash(false, "Cannot delete currently logged-in account");
    }
    let Some(target) = state.db.get_user(id).ok().flatten() else {
        return users_flash(false, "User not found");
    };
    if target.role == "owner" && !actor.is_owner() {
        return users_flash(false, "Only owners can delete owner accounts");
    }

    match state.db.delete_user(id) {
        Ok(_) => {
            state.db.audit(
                &actor.username,
                "user.delete",
                &target.username,
                "",
                &auth::client_ip(&headers),
                true,
            );
            users_flash(true, &format!("Deleted user {}", target.username))
        }
        Err(e) => users_flash(false, &format!("Failed to delete user: {e}")),
    }
}

async fn restart(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if !user.is_owner() {
        return users_flash(false, "Only owners can restart the service");
    }
    state.db.audit(
        &user.username,
        "system.restart",
        "-",
        "Triggered graceful restart",
        &auth::client_ip(&headers),
        true,
    );
    tracing::warn!("admin console requested a restart; shutting down gracefully");
    state.shutdown.notify_waiters();
    users_flash(true, "Restart signal sent, service will restart in seconds")
}

fn config_flash(ok: bool, msg: &str) -> Response {
    flash_to("/admin/config", ok, msg)
}

fn users_flash(ok: bool, msg: &str) -> Response {
    flash_to("/admin/users", ok, msg)
}

fn flash_to(path: &str, ok: bool, msg: &str) -> Response {
    redirect(&format!(
        "{path}?ok={}&msg={}",
        if ok { 1 } else { 0 },
        urlencode(msg)
    ))
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------- static

async fn css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        assets::CSS,
    )
        .into_response()
}

async fn js() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        assets::JS,
    )
        .into_response()
}

async fn logo_svg() -> Response {
    (
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        assets::LOGO_SVG,
    )
        .into_response()
}

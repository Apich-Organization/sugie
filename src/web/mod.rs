//! Admin console: Leptos SSR pages mounted at `/admin` inside the bot's own
//! Axum server, plus a small JSON/SSE API for the command console.

pub mod assets;
pub mod auth;
pub mod config_store;
pub mod db;
pub mod exec;
pub mod passkey;
pub mod perms;
pub mod stats;
pub mod totp;
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
use stats::StatsCache;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use ui::pages;

#[derive(Clone)]
pub struct WebState {
    pub config: Arc<SugieConfig>,
    pub triage: TriageService,
    pub db: WebDb,
    pub files: ConfigFiles,
    pub jobs: Arc<JobRegistry>,
    pub stats: Arc<StatsCache>,
    pub passkey: passkey::PasskeyManager,
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
        crate::security_log::init(&config.db_path);
        if let Some(generated) = auth::bootstrap_admin(&db)? {
            tracing::warn!(
                "Admin console bootstrapped an `admin` account. Generated password: {} (also written to data/admin-initial-password.txt)",
                generated
            );
        }
        let passkey = passkey::PasskeyManager::from_env(config.port);
        Ok(Self {
            config,
            triage,
            db,
            files,
            jobs: Arc::new(JobRegistry::default()),
            stats: Arc::new(StatsCache::default()),
            passkey,
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
        .route("/admin/login/2fa", post(login_2fa_submit))
        .route("/admin/logout", post(logout))
        .route("/admin/repos", get(repos_page))
        .route("/admin/api/repos/sync", post(sync_repos))
        .route("/admin/chat", get(chat_page))
        .route("/admin/config", get(config_page))
        .route("/admin/users", get(users_page))
        .route("/admin/audit", get(audit_page))
        .route("/admin/notify", get(notify_page))
        .route("/admin/notify/send", post(notify_send))
        .route("/admin/audit/settings", post(save_audit_settings))
        .route("/admin/audit/purge", post(purge_audit_logs))
        .route("/admin/audit/export", get(export_audit_csv))
        .route("/admin/security", get(security_page))
        .route("/admin/security/totp/activate", post(totp_activate))
        .route("/admin/security/totp/disable", post(totp_disable))
        .route("/admin/security/passkey/delete", post(passkey_delete))
        .route(
            "/admin/api/passkey/register/start",
            post(api_passkey_register_start),
        )
        .route(
            "/admin/api/passkey/register/finish",
            post(api_passkey_register_finish),
        )
        .route(
            "/admin/api/passkey/auth/start",
            post(api_passkey_auth_start),
        )
        .route(
            "/admin/api/passkey/auth/finish",
            post(api_passkey_auth_finish),
        )
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
        .route("/admin/users/2fa-global", post(toggle_global_2fa))
        .route("/admin/users/reset-2fa", post(reset_user_2fa))
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

/// Resolves the signed-in user even when first-login 2FA enrollment is pending.
fn gate_allow_setup(state: &WebState, headers: &HeaderMap) -> Result<CurrentUser, Response> {
    match auth::current_user(&state.db, headers) {
        Some(user) => Ok(user),
        None => Err(redirect("/admin/login")),
    }
}

/// Every standard page handler funnels through this: anonymous callers get bounced
/// to `/admin/login`, and users required to enroll 2FA get directed to `/admin/security`.
fn gate(state: &WebState, headers: &HeaderMap) -> Result<CurrentUser, Response> {
    let user = gate_allow_setup(state, headers)?;
    if user.needs_2fa_setup {
        return Err(redirect("/admin/security?setup=1"));
    }
    Ok(user)
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
    if let Some(u) = auth::current_user(&state.db, &headers) {
        if u.needs_2fa_setup {
            return redirect("/admin/security?setup=1");
        }
        return redirect("/admin");
    }
    html(pages::login_page(None, "/admin".to_string(), None))
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
        Ok(auth::LoginOutcome::Session(token)) => {
            state
                .db
                .audit(&form.username, "login", "-", "Login successful", &ip, true);
            let global_2fa = state.db.is_global_2fa_enabled();
            let needs_setup = state
                .db
                .find_user(form.username.trim())
                .ok()
                .flatten()
                .map(|u| u.needs_2fa_setup(global_2fa))
                .unwrap_or(false);
            let next = if needs_setup {
                "/admin/security?setup=1".to_string()
            } else if form.next.starts_with("/admin") && !form.next.starts_with("/admin/login") {
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
        Ok(auth::LoginOutcome::Require2fa {
            pending_token,
            user,
        }) => {
            let challenge = pages::LoginChallenge {
                pending_token,
                username: user.username,
                has_totp: user.totp_enabled,
                has_passkey: user.passkey_count > 0,
            };
            html(pages::login_page(None, form.next, Some(challenge)))
        }
        Ok(auth::LoginOutcome::Invalid) => {
            state.db.audit(
                &form.username,
                "login",
                "-",
                "Invalid username or password",
                &ip,
                false,
            );
            html(pages::login_page(
                Some("Invalid username or password, or account is disabled".to_string()),
                form.next,
                None,
            ))
        }
        Err(e) => {
            tracing::error!("admin login failed: {e}");
            html(pages::login_page(
                Some("Login failed, please check server logs".to_string()),
                form.next,
                None,
            ))
        }
    }
}

#[derive(Deserialize)]
struct Login2faForm {
    pending_token: String,
    totp_code: String,
    #[serde(default)]
    next: String,
}

async fn login_2fa_submit(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<Login2faForm>,
) -> Response {
    let ip = auth::client_ip(&headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let token_hash = auth::token_hash(form.pending_token.trim());
    let Some((user, _)) = state.db.get_pending_2fa(&token_hash).ok().flatten() else {
        return html(pages::login_page(
            Some("2FA session expired. Please sign in again.".to_string()),
            form.next,
            None,
        ));
    };

    let Some((secret, enabled)) = state.db.get_totp_secret(user.id).ok().flatten() else {
        return html(pages::login_page(
            Some("Authenticator OTP is not configured for this account.".to_string()),
            form.next,
            None,
        ));
    };

    if !enabled || !totp::verify_totp(&secret, &form.totp_code) {
        state.db.audit(
            &user.username,
            "login.2fa",
            "totp",
            "Invalid OTP code",
            &ip,
            false,
        );
        let challenge = pages::LoginChallenge {
            pending_token: form.pending_token,
            username: user.username,
            has_totp: user.totp_enabled,
            has_passkey: user.passkey_count > 0,
        };
        return html(pages::login_page(
            Some("Invalid 8-digit authenticator code. Please try again.".to_string()),
            form.next,
            Some(challenge),
        ));
    }

    let _ = state.db.consume_pending_2fa(&token_hash);
    match auth::mint_session_for_user(&state.db, user.id, &ip, &ua) {
        Ok(session_token) => {
            state.db.audit(
                &user.username,
                "login.2fa",
                "totp",
                "2FA OTP verified",
                &ip,
                true,
            );
            let next = if form.next.starts_with("/admin") && !form.next.starts_with("/admin/login")
            {
                form.next
            } else {
                "/admin".to_string()
            };
            let mut resp = redirect(&next);
            if let Ok(value) = HeaderValue::from_str(&auth::session_cookie(&session_token)) {
                resp.headers_mut().append(header::SET_COOKIE, value);
            }
            resp
        }
        Err(e) => {
            tracing::error!("mint session after 2fa failed: {e}");
            html(pages::login_page(
                Some("Failed to create session".to_string()),
                form.next,
                None,
            ))
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
    let mut repos = state
        .triage
        .storage()
        .list_repositories()
        .unwrap_or_default();
    if repos.is_empty() {
        if let Ok(synced) = state.triage.sync_all_repositories().await {
            repos = synced;
        }
    }
    if let Some(ref allowed) = state.config.notifications.allowed_repositories {
        for r in allowed {
            if !repos
                .iter()
                .any(|(_, name, _)| name.eq_ignore_ascii_case(r))
            {
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
    let mut repos = state
        .triage
        .storage()
        .list_repositories()
        .unwrap_or_default();
    if repos.is_empty() {
        if let Ok(synced) = state.triage.sync_all_repositories().await {
            repos = synced;
        }
    }
    if let Some(ref allowed) = state.config.notifications.allowed_repositories {
        for r in allowed {
            if !repos
                .iter()
                .any(|(_, name, _)| name.eq_ignore_ascii_case(r))
            {
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
                &format!(
                    "Synced {} repositories from GitHub App installations",
                    synced.len()
                ),
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
    let global_2fa = state.db.is_global_2fa_enabled();
    html(pages::users_page(user, users, repos, global_2fa, flash))
}

async fn security_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let flash = flash_from(&query);
    let user = match gate_allow_setup(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };

    let (totp_setup_secret, totp_setup_qr, totp_setup_url) = if user.totp_enabled {
        (String::new(), String::new(), String::new())
    } else {
        match totp::generate_totp_setup(state.passkey.rp_name(), &user.username) {
            Ok(setup) => (setup.secret, setup.qr_data_url, setup.otpauth_url),
            Err(e) => {
                tracing::error!("generate_totp_setup failed: {e}");
                (String::new(), String::new(), String::new())
            }
        }
    };

    let passkeys = state
        .db
        .list_user_passkeys(user.id)
        .unwrap_or_default()
        .into_iter()
        .map(|pk| pages::PasskeyItem {
            id: pk.id,
            name: pk.device_name,
            created_at: pk.created_at.replace('T', " ").chars().take(16).collect(),
            last_used_at: pk
                .last_used_at
                .map(|t| t.replace('T', " ").chars().take(16).collect())
                .unwrap_or_default(),
        })
        .collect();

    let data = pages::SecurityPageData {
        rp_id: state.passkey.rp_id().to_string(),
        rp_origin: state.passkey.rp_origin().to_string(),
        rp_name: state.passkey.rp_name().to_string(),
        global_2fa: state.db.is_global_2fa_enabled(),
        totp_enabled: user.totp_enabled,
        totp_setup_secret,
        totp_setup_qr,
        totp_setup_url,
        passkeys,
    };

    html(pages::security_page(user, data, flash))
}

#[derive(Deserialize)]
struct TotpActivateForm {
    secret: String,
    code: String,
}

async fn totp_activate(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<TotpActivateForm>,
) -> Response {
    let user = match gate_allow_setup(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let ip = auth::client_ip(&headers);
    let secret = form.secret.trim();
    let code = form.code.trim();

    if !totp::verify_totp(secret, code) {
        state.db.audit(
            &user.username,
            "2fa.totp.activate",
            "-",
            "Invalid verification code during TOTP setup",
            &ip,
            false,
        );
        return security_flash(
            false,
            "Invalid 8-digit verification code. Please check your authenticator app and try again.",
        );
    }

    match state.db.activate_totp(user.id, secret) {
        Ok(_) => {
            state.db.audit(
                &user.username,
                "2fa.totp.activate",
                "-",
                "Activated 8-digit SHA512 TOTP",
                &ip,
                true,
            );
            security_flash(
                true,
                "Authenticator App (SHA512 · 8-digit OTP) activated successfully",
            )
        }
        Err(e) => security_flash(false, &format!("Failed to save OTP secret: {e}")),
    }
}

#[derive(Deserialize)]
struct TotpDisableForm {
    password: String,
}

async fn totp_disable(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<TotpDisableForm>,
) -> Response {
    let user = match gate_allow_setup(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let ip = auth::client_ip(&headers);

    let Some(hash) = state.db.password_hash(user.id).ok().flatten() else {
        return security_flash(false, "Unable to verify account password");
    };
    if !auth::verify_password(&form.password, &hash) {
        state.db.audit(
            &user.username,
            "2fa.totp.disable",
            "-",
            "Password verification failed",
            &ip,
            false,
        );
        return security_flash(false, "Incorrect password");
    }

    if let Err(e) = state.db.disable_totp(user.id) {
        return security_flash(false, &format!("Failed to disable OTP: {e}"));
    }

    state.db.audit(
        &user.username,
        "2fa.totp.disable",
        "-",
        "Disabled Authenticator OTP",
        &ip,
        true,
    );
    security_flash(true, "Authenticator OTP disabled")
}

#[derive(Deserialize)]
struct PasskeyDeleteForm {
    id: i64,
}

async fn passkey_delete(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<PasskeyDeleteForm>,
) -> Response {
    let user = match gate_allow_setup(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let ip = auth::client_ip(&headers);

    match state.db.delete_passkey(form.id, user.id) {
        Ok(true) => {
            state.db.audit(
                &user.username,
                "2fa.passkey.delete",
                &form.id.to_string(),
                "Removed Passkey",
                &ip,
                true,
            );
            security_flash(true, "Passkey removed")
        }
        Ok(false) => security_flash(false, "Passkey not found"),
        Err(e) => security_flash(false, &format!("Failed to remove Passkey: {e}")),
    }
}

#[derive(Deserialize, Default)]
struct PasskeyRegStartReq {
    #[serde(default)]
    #[allow(dead_code)]
    device_name: String,
}

async fn api_passkey_register_start(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(_req): Json<PasskeyRegStartReq>,
) -> Response {
    let user = match gate_allow_setup(&state, &headers) {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Unauthorized" })),
            )
                .into_response()
        }
    };

    let existing_keys = state.db.list_user_passkeys(user.id).unwrap_or_default();
    let exclude: Vec<webauthn_rs::prelude::CredentialID> = existing_keys
        .iter()
        .filter_map(|row| {
            serde_json::from_str::<webauthn_rs::prelude::Passkey>(&row.passkey_json).ok()
        })
        .map(|pk| pk.cred_id().clone())
        .collect();
    let exclude_opt = if exclude.is_empty() {
        None
    } else {
        Some(exclude)
    };

    match state
        .passkey
        .start_registration(user.id, &user.username, &user.display_name, exclude_opt)
    {
        Ok(ccr) => Json(json!({ "options": ccr })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct PasskeyRegFinishReq {
    #[serde(default)]
    device_name: String,
    credential: webauthn_rs::prelude::RegisterPublicKeyCredential,
}

async fn api_passkey_register_finish(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<PasskeyRegFinishReq>,
) -> Response {
    let user = match gate_allow_setup(&state, &headers) {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Unauthorized" })),
            )
                .into_response()
        }
    };
    let ip = auth::client_ip(&headers);

    let passkey = match state.passkey.finish_registration(user.id, &req.credential) {
        Ok(pk) => pk,
        Err(e) => {
            state
                .db
                .audit(&user.username, "2fa.passkey.register", "-", &e, &ip, false);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
        }
    };

    use base64::Engine;
    let cred_id_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(passkey.cred_id());
    let passkey_json = match serde_json::to_string(&passkey) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to serialize passkey: {e}") })),
            )
                .into_response()
        }
    };
    let dev_name = if req.device_name.trim().is_empty() {
        "Passkey".to_string()
    } else {
        req.device_name.trim().to_string()
    };

    match state
        .db
        .save_passkey(user.id, &cred_id_b64, &dev_name, &passkey_json, 0)
    {
        Ok(_) => {
            state.db.audit(
                &user.username,
                "2fa.passkey.register",
                &dev_name,
                "Registered WebAuthn Passkey",
                &ip,
                true,
            );
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Failed to save passkey: {e}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct PasskeyAuthStartReq {
    #[serde(default)]
    #[allow(dead_code)]
    pending_token: Option<String>,
}

async fn api_passkey_auth_start(
    State(state): State<WebState>,
    Json(_req): Json<PasskeyAuthStartReq>,
) -> Response {
    match state.passkey.start_discoverable_authentication() {
        Ok(rcr) => Json(json!({ "options": rcr })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct PasskeyAuthFinishReq {
    #[serde(default)]
    pending_token: Option<String>,
    #[serde(default)]
    next: Option<String>,
    credential: webauthn_rs::prelude::PublicKeyCredential,
}

async fn api_passkey_auth_finish(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<PasskeyAuthFinishReq>,
) -> Response {
    let ip = auth::client_ip(&headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let (claimed_uid, cred_id_b64) = match state.passkey.identify_credential(&req.credential) {
        Ok(pair) => pair,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
        }
    };

    // If this is a 2nd-factor verification against a pending_token, enforce that the
    // passkey belongs to the same user who completed step 1 (password login).
    let pending_user = if let Some(ref pt) = req.pending_token {
        if !pt.trim().is_empty() {
            let th = auth::token_hash(pt.trim());
            match state.db.get_pending_2fa(&th).ok().flatten() {
                Some((u, _)) => Some((u, th)),
                None => {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": "2FA session expired. Please sign in again." })),
                    )
                        .into_response();
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let target_uid = if let Some((ref pu, _)) = pending_user {
        if claimed_uid > 0 && claimed_uid != pu.id {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Passkey belongs to a different user account" })),
            )
                .into_response();
        }
        pu.id
    } else {
        claimed_uid
    };

    let stored_rows = if target_uid > 0 {
        state.db.list_user_passkeys(target_uid).unwrap_or_default()
    } else {
        state
            .db
            .list_all_passkeys()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.credential_id == cred_id_b64)
            .collect()
    };

    if stored_rows.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Unrecognized Passkey credential" })),
        )
            .into_response();
    }

    let resolved_user_id = stored_rows[0].user_id;
    let Some(user_row) = state.db.get_user(resolved_user_id).ok().flatten() else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Account not found" })),
        )
            .into_response();
    };
    if !user_row.active {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Account is disabled" })),
        )
            .into_response();
    }

    let mut passkeys: Vec<webauthn_rs::prelude::Passkey> = stored_rows
        .iter()
        .filter_map(|r| serde_json::from_str::<webauthn_rs::prelude::Passkey>(&r.passkey_json).ok())
        .collect();
    let discoverable: Vec<webauthn_rs::prelude::DiscoverableKey> =
        passkeys.iter().map(|pk| pk.into()).collect();

    let auth_result = match state
        .passkey
        .finish_discoverable_authentication(&req.credential, &discoverable)
    {
        Ok(res) => res,
        Err(e) => {
            state
                .db
                .audit(&user_row.username, "login.passkey", "-", &e, &ip, false);
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": e }))).into_response();
        }
    };

    // Update credential counter & last_used_at
    for pk in passkeys.iter_mut() {
        if pk.cred_id() == auth_result.cred_id() {
            pk.update_credential(&auth_result);
            if let Ok(updated_json) = serde_json::to_string(pk) {
                let _ = state.db.update_passkey_usage(
                    &cred_id_b64,
                    auth_result.counter() as i64,
                    &updated_json,
                );
            }
            break;
        }
    }

    if let Some((_, ref th)) = pending_user {
        let _ = state.db.consume_pending_2fa(th);
    }

    match auth::mint_session_for_user(&state.db, user_row.id, &ip, &ua) {
        Ok(session_token) => {
            state.db.audit(
                &user_row.username,
                "login.passkey",
                &cred_id_b64,
                "Authenticated via Passkey",
                &ip,
                true,
            );
            let next_raw = req.next.unwrap_or_else(|| "/admin".to_string());
            let redirect_url =
                if next_raw.starts_with("/admin") && !next_raw.starts_with("/admin/login") {
                    next_raw
                } else {
                    "/admin".to_string()
                };
            let mut resp = Json(json!({ "ok": true, "redirect": redirect_url })).into_response();
            if let Ok(value) = HeaderValue::from_str(&auth::session_cookie(&session_token)) {
                resp.headers_mut().append(header::SET_COOKIE, value);
            }
            resp
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to create session: {e}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default, Clone)]
struct AuditPageQuery {
    #[serde(default)]
    page: i64,
    #[serde(default)]
    tab: String,
    #[serde(default)]
    ok: Option<String>,
    #[serde(default)]
    msg: Option<String>,

    #[serde(default)]
    q_ts: String,
    #[serde(default)]
    q_user: String,
    #[serde(default)]
    q_action: String,
    #[serde(default)]
    q_target: String,
    #[serde(default)]
    q_detail: String,
    #[serde(default)]
    q_ip: String,
    #[serde(default)]
    q_result: String,

    #[serde(default)]
    q_dir: String,
    #[serde(default)]
    q_method: String,
    #[serde(default)]
    q_status: String,
    #[serde(default)]
    q_actor: String,
}

async fn audit_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<AuditPageQuery>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let page = query.page.max(0);
    let tab = if query.tab.is_empty() {
        "audit".to_string()
    } else {
        query.tab.clone()
    };
    let flash = query
        .msg
        .map(|m| (query.ok.as_deref().unwrap_or("1") == "1", m));
    let log_settings = state.db.get_security_log_settings();
    let log_file_path = crate::security_log::resolve_log_file_path(&state.config.db_path)
        .display()
        .to_string();

    let audit_filter = crate::web::db::AuditFilter {
        ts: query.q_ts.clone(),
        username: query.q_user.clone(),
        action: query.q_action.clone(),
        target: query.q_target.clone(),
        detail: query.q_detail.clone(),
        ip: query.q_ip.clone(),
        result: query.q_result.clone(),
    };
    let http_filter = crate::web::db::SecurityHttpFilter {
        ts: query.q_ts.clone(),
        direction: query.q_dir.clone(),
        method: query.q_method.clone(),
        target: query.q_target.clone(),
        status: query.q_status.clone(),
        actor: query.q_actor.clone(),
        ip: query.q_ip.clone(),
        detail: query.q_detail.clone(),
    };

    if tab == "inbound" || tab == "github" || tab == "notify" || tab == "http_all" {
        let dir_filter = match tab.as_str() {
            "inbound" => Some("INBOUND_API"),
            "github" => Some("OUTBOUND_GITHUB"),
            "notify" => Some("OUTBOUND_NOTIFY"),
            _ => None,
        };
        let http_rows = state
            .db
            .list_security_http_logs_filtered(dir_filter, &http_filter, 50, page * 50)
            .unwrap_or_default()
            .into_iter()
            .map(|r| pages::SecurityHttpCard {
                ts: r.ts.replace('T', " ").chars().take(19).collect(),
                direction: r.direction,
                method: r.method,
                target: r.target,
                status: r.status,
                duration_ms: r.duration_ms,
                ip: r.ip,
                actor: r.actor,
                detail: r.detail,
            })
            .collect();
        let total = state
            .db
            .count_security_http_logs_filtered(dir_filter, &http_filter)
            .unwrap_or(0);
        html(pages::audit_page(
            user,
            tab,
            Vec::new(),
            http_rows,
            page,
            total,
            log_file_path,
            log_settings,
            flash,
            audit_filter,
            http_filter,
        ))
    } else {
        let rows = state
            .db
            .list_audit_filtered(&audit_filter, 50, page * 50)
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
        let total = state.db.count_audit_filtered(&audit_filter).unwrap_or(0);
        html(pages::audit_page(
            user,
            tab,
            rows,
            Vec::new(),
            page,
            total,
            log_file_path,
            log_settings,
            flash,
            audit_filter,
            http_filter,
        ))
    }
}

async fn save_audit_settings(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let tab = form
        .get("tab")
        .cloned()
        .unwrap_or_else(|| "audit".to_string());
    let settings = crate::security_log::SecurityLogSettings {
        admin_audit: form.contains_key("admin_audit"),
        inbound_webhooks: form.contains_key("inbound_webhooks"),
        inbound_admin: form.contains_key("inbound_admin"),
        outbound_github: form.contains_key("outbound_github"),
        outbound_lark: form.contains_key("outbound_lark"),
        outbound_discord: form.contains_key("outbound_discord"),
        outbound_zulip: form.contains_key("outbound_zulip"),
        retention_days: form
            .get("retention_days")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0)
            .clamp(0, 36500),
    };
    match state.db.set_security_log_settings(&settings) {
        Ok(_) => {
            state.db.audit(
                &user.username,
                "security.log_settings",
                "-",
                &format!(
                    "admin_audit={} inbound_webhooks={} inbound_admin={} github={} lark={} discord={} zulip={} retention_days={}",
                    settings.admin_audit,
                    settings.inbound_webhooks,
                    settings.inbound_admin,
                    settings.outbound_github,
                    settings.outbound_lark,
                    settings.outbound_discord,
                    settings.outbound_zulip,
                    settings.retention_days
                ),
                &auth::client_ip(&headers),
                true,
            );
            redirect(&format!(
                "/admin/audit?tab={}&ok=1&msg={}",
                urlencode(&tab),
                urlencode("Saved security log collection settings")
            ))
        }
        Err(e) => redirect(&format!(
            "/admin/audit?tab={}&ok=0&msg={}",
            urlencode(&tab),
            urlencode(&format!("Failed to save logging settings: {e}"))
        )),
    }
}

async fn notify_page(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let flash = flash_from(&query);
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let n = state.triage.notifier();
    html(pages::notify_page(
        user,
        n.discord().is_some(),
        n.lark().is_some(),
        n.zulip().is_some(),
        flash,
    ))
}

fn notify_flash(ok: bool, msg: &str) -> Response {
    flash_to("/admin/notify", ok, msg)
}

async fn notify_send(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    use crate::notifications::{DiscordEmbedField, NotificationLevel, ZulipTopicCategory};
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let title = form
        .get("title")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let message = form
        .get("message")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if title.is_empty() || message.is_empty() {
        return notify_flash(false, "Title and message are required");
    }
    let url = form
        .get("url")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(ref u) = url {
        if !(u.starts_with("https://") || u.starts_with("http://")) {
            return notify_flash(false, "Link must start with http:// or https://");
        }
    }
    let level = match form.get("level").map(|s| s.as_str()).unwrap_or("info") {
        "success" => NotificationLevel::Success,
        "warning" => NotificationLevel::Warning,
        "error" => NotificationLevel::Error,
        _ => NotificationLevel::Info,
    };
    let want_lark = form.contains_key("ch_lark");
    let want_discord = form.contains_key("ch_discord");
    let want_zulip = form.contains_key("ch_zulip");
    if !(want_lark || want_discord || want_zulip) {
        return notify_flash(false, "Select at least one channel");
    }

    let notifier = state.triage.notifier();
    let mut sent: Vec<&str> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let fields: Vec<DiscordEmbedField> = vec![DiscordEmbedField {
        name: "Sent by".into(),
        value: user.username.clone(),
        inline: true,
    }];

    if want_lark {
        match notifier.lark() {
            Some(l) => match l
                .send_notification(
                    &title,
                    &message,
                    url.as_deref(),
                    level.lark_template(),
                    &[("Sent by", user.username.as_str())],
                )
                .await
            {
                Ok(_) => sent.push("Lark"),
                Err(e) => failed.push(format!("Lark: {e}")),
            },
            None => failed.push("Lark: not configured".into()),
        }
    }
    if want_discord {
        match notifier.discord() {
            Some(d) => match d
                .send_embed(
                    &title,
                    &message,
                    url.as_deref(),
                    level.discord_color(),
                    fields.clone(),
                    false,
                )
                .await
            {
                Ok(_) => sent.push("Discord"),
                Err(e) => failed.push(format!("Discord: {e}")),
            },
            None => failed.push("Discord: not configured".into()),
        }
    }
    if want_zulip {
        match notifier.zulip() {
            Some(z) => {
                let mut text = match url.as_deref() {
                    Some(u) => format!("### [{title}]({u})\n\n{message}"),
                    None => format!("### {title}\n\n{message}"),
                };
                text.push_str(&format!("\n\n* **Sent by**: {}\n", user.username));
                match z
                    .send_message(ZulipTopicCategory::Custom("announcements"), &text)
                    .await
                {
                    Ok(_) => sent.push("Zulip"),
                    Err(e) => failed.push(format!("Zulip: {e}")),
                }
            }
            None => failed.push("Zulip: not configured".into()),
        }
    }

    let ok = failed.is_empty();
    state.db.audit(
        &user.username,
        "notify.send",
        &sent.join(","),
        &format!("title={title} failed={}", failed.join("; ")),
        &auth::client_ip(&headers),
        ok,
    );
    let mut msg = String::new();
    if !sent.is_empty() {
        msg.push_str(&format!("Sent to {}. ", sent.join(", ")));
    }
    if !failed.is_empty() {
        msg.push_str(&format!("Failed: {}", failed.join("; ")));
    }
    notify_flash(ok, msg.trim())
}

async fn purge_audit_logs(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let tab = form
        .get("tab")
        .cloned()
        .unwrap_or_else(|| "http_all".to_string());
    let days = form
        .get("days")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(-1);
    if days < 0 {
        return redirect(&format!(
            "/admin/audit?tab={}&ok=0&msg={}",
            urlencode(&tab),
            urlencode("Please enter a valid number of days (0 or more)")
        ));
    }
    match state.db.purge_security_http_logs(days) {
        Ok(n) => {
            state.db.audit(
                &user.username,
                "security.log_purge",
                "security_http_logs",
                &format!("Deleted {n} rows older than {days} days"),
                &auth::client_ip(&headers),
                true,
            );
            redirect(&format!(
                "/admin/audit?tab={}&ok=1&msg={}",
                urlencode(&tab),
                urlencode(&format!("Deleted {n} log records older than {days} days"))
            ))
        }
        Err(e) => redirect(&format!(
            "/admin/audit?tab={}&ok=0&msg={}",
            urlencode(&tab),
            urlencode(&format!("Failed to purge logs: {e}"))
        )),
    }
}

fn csv_cell(v: &str) -> String {
    let mut s = v.to_string();
    // Neutralise spreadsheet formula injection.
    if matches!(s.chars().next(), Some('=' | '+' | '-' | '@' | '\t' | '\r')) {
        s.insert(0, '\'');
    }
    format!("\"{}\"", s.replace('"', "\"\""))
}

async fn export_audit_csv(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<AuditPageQuery>,
) -> Response {
    let user = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let tab = if query.tab.is_empty() {
        "audit".to_string()
    } else {
        query.tab
    };
    let mut out = String::new();

    let audit_filter = crate::web::db::AuditFilter {
        ts: query.q_ts.clone(),
        username: query.q_user.clone(),
        action: query.q_action.clone(),
        target: query.q_target.clone(),
        detail: query.q_detail.clone(),
        ip: query.q_ip.clone(),
        result: query.q_result.clone(),
    };
    let http_filter = crate::web::db::SecurityHttpFilter {
        ts: query.q_ts.clone(),
        direction: query.q_dir.clone(),
        method: query.q_method.clone(),
        target: query.q_target.clone(),
        status: query.q_status.clone(),
        actor: query.q_actor.clone(),
        ip: query.q_ip.clone(),
        detail: query.q_detail.clone(),
    };

    match tab.as_str() {
        "inbound" | "github" | "notify" | "http_all" => {
            let dir = match tab.as_str() {
                "inbound" => Some("INBOUND_API"),
                "github" => Some("OUTBOUND_GITHUB"),
                "notify" => Some("OUTBOUND_NOTIFY"),
                _ => None,
            };
            out.push_str(
                "id,timestamp,direction,method,target,status,duration_ms,ip,actor,detail\n",
            );
            for r in state
                .db
                .list_security_http_logs_filtered(dir, &http_filter, 100_000, 0)
                .unwrap_or_default()
            {
                out.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{}\n",
                    r.id,
                    csv_cell(&r.ts),
                    csv_cell(&r.direction),
                    csv_cell(&r.method),
                    csv_cell(&r.target),
                    r.status,
                    r.duration_ms,
                    csv_cell(&r.ip),
                    csv_cell(&r.actor),
                    csv_cell(&r.detail)
                ));
            }
        }
        _ => {
            out.push_str("id,timestamp,username,action,target,detail,ip,result\n");
            for r in state
                .db
                .list_audit_filtered(&audit_filter, 100_000, 0)
                .unwrap_or_default()
            {
                out.push_str(&format!(
                    "{},{},{},{},{},{},{},{}\n",
                    r.id,
                    csv_cell(&r.ts),
                    csv_cell(&r.username),
                    csv_cell(&r.action),
                    csv_cell(&r.target),
                    csv_cell(&r.detail),
                    csv_cell(&r.ip),
                    if r.ok { "success" } else { "denied" }
                ));
            }
        }
    }
    state.db.audit(
        &user.username,
        "security.log_export",
        &tab,
        "Exported CSV",
        &auth::client_ip(&headers),
        true,
    );
    let filename = format!("sugie-{}-{}.csv", tab, Utc::now().format("%Y%m%d-%H%M%S"));
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        out,
    )
        .into_response()
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
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
                .into_response()
        }
    };

    match exec::dispatch(state.clone(), user, req.repo, req.command) {
        Ok(job_id) => Json(json!({ "job_id": job_id })).into_response(),
        Err(msg) => (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response(),
    }
}

async fn api_job(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if gate(&state, &headers).is_err() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Unauthorized"})),
        )
            .into_response();
    }
    match state.jobs.snapshot(&id) {
        Some(job) => Json(job).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Job not found"})),
        )
            .into_response(),
    }
}

async fn api_job_stream(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if gate(&state, &headers).is_err() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Unauthorized"})),
        )
            .into_response();
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
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Unauthorized"})),
        )
            .into_response();
    }
    if query.repo.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Missing repo parameter"})),
        )
            .into_response();
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
        form.get(k)
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(default)
    };

    let text = match state.files.read_toml() {
        Ok(t) => t,
        Err(e) => return config_flash(false, &format!("Failed to read configuration: {e}")),
    };

    let mut patches: Vec<(String, config_store::Patch)> = vec![
        ("host".into(), config_store::Patch::Str(get("host"))),
        (
            "port".into(),
            config_store::Patch::Int(int_or("port", 8800)),
        ),
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
        "weekly_report",
        "monthly_report",
        "assigned",
        "unassigned",
        "issue_opened",
        "issue_closed",
        "issue_comment",
        "pr_opened",
        "pr_merged",
        "label_changed",
        "stale_warned",
        "stale_closed",
        "ci_failure",
        "duplicate_detected",
        "dco_failed",
        "growth_milestone",
        "cross_repo_move",
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
                state.db.audit(
                    &user.username,
                    "config.save",
                    "sugie.toml",
                    &e.to_string(),
                    "-",
                    false,
                );
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
            state.db.audit(
                &user.username,
                "config.save",
                "sugie.toml",
                &e.to_string(),
                "-",
                false,
            );
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
            state.db.audit(
                &user.username,
                "config.save",
                ".env",
                &e.to_string(),
                "-",
                false,
            );
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
            if !repos
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(r))
            {
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
            totp_enabled: u.totp_enabled,
            passkey_count: u.passkey_count,
            two_fa_policy: u.two_fa_policy,
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
    let username = form
        .get("username")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let password = form.get("password").cloned().unwrap_or_default();
    let role = form.get("role").cloned().unwrap_or_else(|| "viewer".into());
    let display = form.get("display_name").cloned().unwrap_or_default();
    let two_fa_policy = if actor.is_owner() {
        form.get("two_fa_policy")
            .cloned()
            .unwrap_or_else(|| "default".into())
    } else {
        "default".into()
    };

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
            if actor.is_owner() && two_fa_policy != "default" {
                let _ = state.db.set_user_two_fa_policy(id, &two_fa_policy);
            }
            let repos: Vec<String> = form
                .get("repos")
                .map(|v| {
                    v.lines()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if !repos.is_empty() {
                let _ = state.db.set_repo_perms(id, &repos);
            }
            state.db.audit(
                &actor.username,
                "user.create",
                &username,
                &format!("role={role} 2fa_policy={two_fa_policy}"),
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

    let role = form
        .get("role")
        .cloned()
        .unwrap_or_else(|| target.role.clone());
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

    if actor.is_owner() {
        if let Some(policy) = form.get("two_fa_policy") {
            let _ = state.db.set_user_two_fa_policy(id, policy);
        }
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

async fn toggle_global_2fa(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let actor = match gate_admin(&state, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if !actor.is_owner() {
        return users_flash(false, "Only owners can change the global 2FA policy");
    }
    let enabled = form
        .get("enabled")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    match state.db.set_global_2fa_enabled(enabled) {
        Ok(_) => {
            state.db.audit(
                &actor.username,
                "2fa.global_policy",
                "-",
                &format!("global_2fa_required={enabled}"),
                &auth::client_ip(&headers),
                true,
            );
            users_flash(
                true,
                if enabled {
                    "Global 2FA requirement enabled for all users"
                } else {
                    "Global 2FA requirement disabled"
                },
            )
        }
        Err(e) => users_flash(false, &format!("Failed to update global 2FA policy: {e}")),
    }
}

async fn reset_user_2fa(
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
        return users_flash(false, "Only owners can reset 2FA for owner accounts");
    }

    match state.db.reset_user_2fa(id) {
        Ok(_) => {
            state.db.audit(
                &actor.username,
                "user.reset_2fa",
                &target.username,
                "Cleared TOTP and Passkeys",
                &auth::client_ip(&headers),
                true,
            );
            users_flash(
                true,
                &format!("Reset 2FA credentials for {}", target.username),
            )
        }
        Err(e) => users_flash(false, &format!("Failed to reset 2FA: {e}")),
    }
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

fn security_flash(ok: bool, msg: &str) -> Response {
    flash_to("/admin/security", ok, msg)
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
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
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

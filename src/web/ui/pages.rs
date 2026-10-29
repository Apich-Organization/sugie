//! Page-level Leptos components. Every page is a plain SSR component tree —
//! no hydration, no client-side router.

use crate::web::config_store::{BackupInfo, EnvEntry};
use crate::web::exec::Job;
use crate::web::perms::{CurrentUser, COMMANDS};
use crate::web::stats::RepoStats;
use crate::web::ui::{number, Flash, Shell, StatCard};
use leptos::prelude::*;

#[derive(Clone, Default)]
pub struct RepoCard {
    pub full_name: String,
    pub installation_id: u64,
    pub stats: Option<RepoStats>,
}

#[derive(Clone, Default)]
pub struct Totals {
    pub repos: usize,
    pub issues: u64,
    pub prs: u64,
    pub commits: u64,
}

#[derive(Clone, Default)]
pub struct ConfigForm {
    pub host: String,
    pub port: String,
    pub db_path: String,
    pub allowed_repositories: String,
    pub super_users: String,
    pub restricted_commands: String,
    pub stale_enabled: bool,
    pub stale_days: String,
    pub stale_interval: String,
    pub reviewers_enabled: bool,
    pub reviewers_per_pr: String,
    pub labels_enabled: bool,
    pub events: Vec<(String, bool)>,
}

#[derive(Clone, Default)]
pub struct UserCard {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub role_label: String,
    pub active: bool,
    pub last_login: String,
    pub repos: Vec<String>,
    pub commands: Vec<String>,
    pub totp_enabled: bool,
    pub passkey_count: i64,
    pub two_fa_policy: String,
}

#[derive(Clone, Default)]
pub struct LoginChallenge {
    pub pending_token: String,
    pub username: String,
    pub has_totp: bool,
    pub has_passkey: bool,
}

#[derive(Clone, Default)]
pub struct PasskeyItem {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub last_used_at: String,
}

#[derive(Clone, Default)]
pub struct SecurityPageData {
    pub rp_id: String,
    pub rp_origin: String,
    pub rp_name: String,
    pub global_2fa: bool,
    pub totp_enabled: bool,
    pub totp_setup_secret: String,
    pub totp_setup_qr: String,
    pub totp_setup_url: String,
    pub passkeys: Vec<PasskeyItem>,
}

#[derive(Clone, Default)]
pub struct AuditCard {
    pub ts: String,
    pub username: String,
    pub action: String,
    pub target: String,
    pub detail: String,
    pub ip: String,
    pub ok: bool,
}

#[derive(Clone, Default)]
pub struct SecurityHttpCard {
    pub ts: String,
    pub direction: String,
    pub method: String,
    pub target: String,
    pub status: i64,
    pub duration_ms: i64,
    pub ip: String,
    pub actor: String,
    pub detail: String,
}

// --------------------------------------------------------------------- login

pub fn login_page(
    error: Option<String>,
    next: String,
    challenge: Option<LoginChallenge>,
) -> impl IntoView {
    let err_view = match error {
        Some(e) if !e.is_empty() => view! { <div class="alert err">{e}</div> }.into_any(),
        _ => view! { <span></span> }.into_any(),
    };

    let body_content = match challenge {
        Some(ch) => {
            let totp_form = if ch.has_totp {
                let pending = ch.pending_token.clone();
                let next_val = next.clone();
                view! {
                    <form method="post" action="/admin/login/2fa">
                        <input type="hidden" name="pending_token" value=pending/>
                        <input type="hidden" name="next" value=next_val/>
                        <label class="field">
                            <span>"8-Digit Authenticator Code (SHA512)"</span>
                            <input
                                type="text"
                                name="totp_code"
                                inputmode="numeric"
                                pattern="[0-9]{6,8}"
                                maxlength="8"
                                placeholder="00000000"
                                autocomplete="one-time-code"
                                autofocus=""
                                required=""
                                class="mono"
                                style="text-align:center;font-size:20px;letter-spacing:.28em;padding:12px"
                            />
                        </label>
                        <button class="btn primary" type="submit" style="width:100%">"Verify OTP Code"</button>
                    </form>
                }
                .into_any()
            } else {
                view! { <span></span> }.into_any()
            };

            let passkey_btn = if ch.has_passkey {
                let pending = ch.pending_token.clone();
                let next_val = next.clone();
                view! {
                    <div style=if ch.has_totp { "margin-top:12px" } else { "" }>
                        <button
                            type="button"
                            class="btn"
                            id="btn-passkey-login"
                            data-pending-token=pending
                            data-next=next_val
                            style="width:100%"
                        >
                            "Verify with Passkey / Security Key"
                        </button>
                    </div>
                }
                .into_any()
            } else {
                view! { <span></span> }.into_any()
            };

            view! {
                <h1>"Two-Factor Verification"</h1>
                <p class="sub">{format!("Complete 2FA verification for @{}", ch.username)}</p>
                {err_view}
                <div id="passkey-err" class="alert err" style="display:none"></div>
                {totp_form}
                {passkey_btn}
                <div class="auth-foot" style="margin-top:18px">
                    <a href="/admin/login">"← Back to Sign In"</a>
                </div>
            }
            .into_any()
        }
        None => {
            let next_for_pk = next.clone();
            view! {
                <h1>"Sign In"</h1>
                <p class="sub">"Enter your credentials or use a Passkey to access the admin console"</p>
                {err_view}
                <div id="passkey-err" class="alert err" style="display:none"></div>
                <form method="post" action="/admin/login">
                    <input type="hidden" name="next" value=next/>
                    <label class="field">
                        <span>"Username"</span>
                        <input type="text" name="username" autocomplete="username webauthn" autofocus="" required=""/>
                    </label>
                    <label class="field">
                        <span>"Password"</span>
                        <input type="password" name="password" autocomplete="current-password" required=""/>
                    </label>
                    <button class="btn primary" type="submit" style="width:100%">"Sign In"</button>
                </form>
                <div style="display:flex;align-items:center;gap:10px;margin:16px 0">
                    <div style="flex:1;height:1px;background:var(--border)"></div>
                    <span class="hint">"or"</span>
                    <div style="flex:1;height:1px;background:var(--border)"></div>
                </div>
                <button
                    type="button"
                    class="btn"
                    id="btn-passkey-login"
                    data-next=next_for_pk
                    style="width:100%"
                >
                    "Sign in with Passkey / Security Key"
                </button>
            }
            .into_any()
        }
    };

    view! {
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>"Sign In · Sugie Admin"</title>
                <link rel="icon" type="image/svg+xml" href="/admin/assets/logo.svg"/>
                <link rel="stylesheet" href="/admin/assets/app.css"/>
            </head>
            <body>
                <div class="auth-wrap">
                    <div class="card auth-card">
                        <div class="auth-brand">
                            <img class="logo" src="/admin/assets/logo.svg" alt="Sugie" style="width:38px;height:38px;border-radius:10px;object-fit:contain"/>
                            <div>
                                <div style="font-weight:650;letter-spacing:-.3px">"Sugie Admin"</div>
                                <div class="hint">"GitHub Triage Bot Admin Console"</div>
                            </div>
                        </div>
                        {body_content}
                    </div>
                </div>
                <script src="/admin/assets/app.js"></script>
            </body>
        </html>
    }
}

// ----------------------------------------------------------------- dashboard

pub fn dashboard_page(
    user: CurrentUser,
    cards: Vec<RepoCard>,
    totals: Totals,
    summary: crate::web::config_store::ConfigSummary,
    flash: Option<(bool, String)>,
) -> impl IntoView {
    let rows = cards
        .iter()
        .map(|card| {
            let name = card.full_name.clone();
            let (issues, prs, commits, stars, status_cls, status_txt, err_hint, is_ok) = match &card.stats {
                Some(s) if s.is_ok() => (
                    number(s.open_issues),
                    number(s.open_prs),
                    number(s.commits_30d),
                    number(s.stars),
                    "badge ok",
                    "Healthy",
                    String::new(),
                    true,
                ),
                Some(s) => {
                    let err_msg = s
                        .error
                        .clone()
                        .unwrap_or_else(|| "Failed to fetch repository status".to_string());
                    let (cls, txt) = if is_suspended_error(&err_msg) {
                        ("badge warn", "Suspended")
                    } else {
                        ("badge bad", "Error")
                    };
                    (
                        "-".to_string(),
                        "-".to_string(),
                        "-".to_string(),
                        "-".to_string(),
                        cls,
                        txt,
                        err_msg,
                        false,
                    )
                }
                None => (
                    "…".to_string(),
                    "…".to_string(),
                    "…".to_string(),
                    "…".to_string(),
                    "badge muted",
                    "Pending",
                    String::new(),
                    false,
                ),
            };
            let link = format!("/admin/chat?repo={}", urlencode(&card.full_name));
            let dot_cls = if is_ok { "dot" } else { "dot bad" };
            let row_status = if is_ok {
                "ok"
            } else if status_txt == "Pending" {
                "pending"
            } else {
                "error"
            };
            view! {
                <tr data-status=row_status>
                    <td><div class="repo-name"><span class=dot_cls></span>{name.clone()}</div></td>
                    <td class="num">{issues}</td>
                    <td class="num">{prs}</td>
                    <td class="num">{commits}</td>
                    <td class="num">{stars}</td>
                    <td><span class=status_cls title=err_hint>{status_txt}</span></td>
                    <td><a class="btn sm ghost" href=link>"Console"</a></td>
                </tr>
            }
        })
        .collect_view();

    let rows = if cards.is_empty() {
        view! {
            <tr><td colspan="7" class="empty">"No repositories onboarded yet. Please install the GitHub App or click Sync on the Repositories page."</td></tr>
        }
        .into_any()
    } else {
        rows.into_any()
    };

    let flash_view = match flash {
        Some((ok, text)) => view! { <Flash ok=ok text=text/> }.into_any(),
        None => view! { <span></span> }.into_any(),
    };

    let channels = if summary.channels.is_empty() {
        "None".to_string()
    } else {
        summary.channels.join(" · ")
    };
    let allowed = if summary.allowed_repositories.is_empty() {
        "All onboarded repositories".to_string()
    } else {
        summary.allowed_repositories.join(", ")
    };

    view! {
        <Shell title="Dashboard".to_string() active="dashboard".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Overview"</div>
                    <h1>"Dashboard Overview"</h1>
                    <p>"Real-time overview of onboarded repositories. Metrics cached for 60s."</p>
                </div>
                <div class="row">
                    <a class="btn ghost" href="/admin/repos">"All Repositories"</a>
                    <a class="btn primary" href="/admin/chat">"Open Console"</a>
                </div>
            </div>
            {flash_view}
            <div class="grid cols-4" style="margin-bottom:16px">
                <StatCard label="Repositories".to_string() value=number(totals.repos as u64) sub="GitHub App installations".to_string() accent=true/>
                <StatCard label="Open Issues".to_string() value=number(totals.issues) sub="Unresolved issues".to_string() accent=false/>
                <StatCard label="Open PRs".to_string() value=number(totals.prs) sub="Pending pull requests".to_string() accent=false/>
                <StatCard label="30d Commits".to_string() value=number(totals.commits) sub="Commits on default branch".to_string() accent=false/>
            </div>
            <div class="grid split-19">
                <div class="card tight">
                    <div class="card-head" style="padding:18px 20px 0">
                        <h2>"Repositories"</h2>
                        <span class="hint">{format!("{} repositories", cards.len())}</span>
                    </div>
                    <div class="toolbar">
                        <input type="search" placeholder="Search repositories…" data-filter-table="dash-table" data-filter-kind="text" />
                        <label class="pill-check"><input type="checkbox" data-filter-table="dash-table" data-filter-kind="errors" /><span>"Show only errors"</span></label>
                    </div>
                    <table id="dash-table" style="margin-top:10px">
                        <thead>
                            <tr>
                                <th>"Repository"</th><th>"Issues"</th><th>"PRs"</th><th>"Commits"</th><th>"Stars"</th><th>"Status"</th><th>""</th>
                            </tr>
                        </thead>
                        <tbody>{rows}</tbody>
                    </table>
                </div>
                <div class="stack">
                    <div class="card">
                        <div class="card-head"><h2>"Active Configuration"</h2><a class="hint" href="/admin/config">"Edit"</a></div>
                        <dl class="kv">
                            <dt>"Listen"</dt><dd>{format!("{}:{}", summary.host, summary.port)}</dd>
                            <dt>"Database"</dt><dd class="mono">{summary.db_path.clone()}</dd>
                            <dt>"Notifications"</dt><dd>{channels}</dd>
                            <dt>"Repo Filter"</dt><dd>{allowed}</dd>
                            <dt>"Super Users"</dt><dd>{summary.super_users.join(", ")}</dd>
                            <dt>"Restricted"</dt><dd class="mono">{format!("/{}", summary.restricted_commands.join(", /"))}</dd>
                            <dt>"Stale Sweep"</dt><dd>{if summary.stale_enabled { "Enabled" } else { "Disabled" }}</dd>
                        </dl>
                    </div>
                    <div class="card">
                        <div class="card-head"><h2>"Quick Commands"</h2></div>
                        <div class="perm-list">
                            <a class="btn sm ghost" href="/admin/chat">"/status"</a>
                            <a class="btn sm ghost" href="/admin/chat">"/stats"</a>
                            <a class="btn sm ghost" href="/admin/chat">"/issues"</a>
                            <a class="btn sm ghost" href="/admin/chat">"/users"</a>
                            <a class="btn sm ghost" href="/admin/chat">"/report"</a>
                        </div>
                    </div>
                </div>
            </div>
        </Shell>
    }
}

// --------------------------------------------------------------------- repos

pub fn repos_page(user: CurrentUser, cards: Vec<RepoCard>) -> impl IntoView {
    let rows = cards
        .iter()
        .map(|card| {
            let name = card.full_name.clone();
            let (issues, prs, commits, merged, branch, updated, err) = match &card.stats {
                Some(s) => (
                    number(s.open_issues),
                    number(s.open_prs),
                    number(s.commits_30d),
                    number(s.merged_30d),
                    s.default_branch.clone(),
                    s.fetched_at.clone(),
                    s.error.clone().unwrap_or_default(),
                ),
                None => (
                    "…".to_string(),
                    "…".to_string(),
                    "…".to_string(),
                    "…".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    String::new(),
                ),
            };
            let link = format!("/admin/chat?repo={}", urlencode(&card.full_name));
            let stats_link = format!("/admin/api/stats?repo={}", urlencode(&card.full_name));
            let fetched_cell = if err.is_empty() {
                view! { <span>{updated}</span> }.into_any()
            } else if is_suspended_error(&err) {
                view! { <span class="badge warn" title=err.clone()>"Suspended"</span> }.into_any()
            } else {
                view! { <span class="badge bad" title=err.clone()>"Error"</span> }.into_any()
            };
            let row_status = if err.is_empty() {
                if card.stats.is_some() { "ok" } else { "pending" }
            } else if is_suspended_error(&err) {
                "suspended"
            } else {
                "error"
            };
            let dot_cls = if err.is_empty() { "dot" } else { "dot bad" };
            view! {
                <tr data-status=row_status>
                    <td>
                        <div class="repo-name"><span class=dot_cls></span>{name.clone()}</div>
                        <div class="hint mono">{format!("installation {}", card.installation_id)}</div>
                    </td>
                    <td class="num">{issues}</td>
                    <td class="num">{prs}</td>
                    <td class="num">{commits}</td>
                    <td class="num">{merged}</td>
                    <td class="mono">{branch}</td>
                    <td class="hint">{fetched_cell}</td>
                    <td>
                        <div class="row">
                            <a class="btn sm ghost" href=link>"Console"</a>
                            <a class="btn sm ghost" href=stats_link>"Refresh"</a>
                        </div>
                    </td>
                </tr>
            }
        })
        .collect_view();

    let rows = if cards.is_empty() {
        view! {
            <tr><td colspan="8" class="empty">"No connected repositories yet"</td></tr>
        }
        .into_any()
    } else {
        rows.into_any()
    };

    view! {
        <Shell title="Repositories".to_string() active="repos".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Repositories"</div>
                    <h1>"Connected Repositories"</h1>
                    <p>"All repositories within the GitHub App installation scope recorded by the bot"</p>
                </div>
                <div class="row">
                    <form method="post" action="/admin/api/repos/sync" style="margin:0">
                        <button class="btn ghost" type="submit">"Sync Repositories"</button>
                    </form>
                    <a class="btn primary" href="/admin/chat">"Open Console"</a>
                </div>
            </div>
            <div class="card tight">
                <div class="toolbar">
                    <input type="search" placeholder="Search repositories or installation id…" data-filter-table="repos-table" data-filter-kind="text" />
                    <select data-filter-table="repos-table" data-filter-kind="status">
                        <option value="">"All statuses"</option>
                        <option value="ok">"Healthy"</option>
                        <option value="suspended">"Suspended"</option>
                        <option value="error">"Error"</option>
                        <option value="pending">"Pending"</option>
                    </select>
                </div>
                <table id="repos-table">
                    <thead>
                        <tr>
                            <th>"Repository"</th><th>"Issues"</th><th>"PRs"</th><th>"30d Commits"</th><th>"30d Merged"</th><th>"Default Branch"</th><th>"Last Fetched"</th><th>""</th>
                        </tr>
                    </thead>
                    <tbody>{rows}</tbody>
                </table>
            </div>
        </Shell>
    }
}

// ---------------------------------------------------------------------- chat

pub fn chat_page(
    user: CurrentUser,
    repos: Vec<String>,
    selected: String,
    history: Vec<Job>,
) -> impl IntoView {
    let repo_links = repos
        .iter()
        .map(|r| {
            let cls = if r == &selected { "active" } else { "" };
            let href = format!("/admin/chat?repo={}", urlencode(r));
            view! { <a href=href class=cls><span>{r.clone()}</span></a> }
        })
        .collect_view();

    let has_repo = !selected.is_empty();
    let head_badge = if has_repo {
        selected.clone()
    } else {
        "No repository selected".to_string()
    };
    let placeholder = if has_repo {
        "Enter command, e.g. /issues 5 or /label 42 bug (Enter to send, Shift+Enter for newline)".to_string()
    } else {
        "Please select a repository on the left".to_string()
    };

    let messages = history
        .iter()
        .rev()
        .map(|job| {
            let cmd = job.command.clone();
            let meta = format!("@{} · {}", job.username, job.started_at);
            let status = match job.status {
                crate::web::exec::JobStatus::Running => "running",
                crate::web::exec::JobStatus::Done => "done",
                crate::web::exec::JobStatus::Failed => "failed",
            };
            let bot_meta = format!("sugie · {} · {}", job.repo, status);
            let lines = job
                .lines
                .iter()
                .skip(1)
                .map(|l| {
                    let cls = format!("line-{}", l.level);
                    let text = format!("{}\n", l.text);
                    view! { <span class=cls>{text}</span> }
                })
                .collect_view();
            view! {
                <div class="msg user">
                    <div class="meta">{meta}</div>
                    <pre>{cmd}</pre>
                </div>
                <div class="msg bot">
                    <div class="meta">{bot_meta}</div>
                    <pre>{lines}</pre>
                </div>
            }
        })
        .collect_view();

    let quick = COMMANDS
        .iter()
        .take(10)
        .map(|spec| {
            let cmd = spec.name.to_string();
            view! { <button type="button" data-cmd=cmd.clone()>{format!("/{cmd}")}</button> }
        })
        .collect_view();

    view! {
        <Shell title="Console".to_string() active="chat".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Console"</div>
                    <h1>"Command Console"</h1>
                    <p>"Select a repository and dispatch triage commands executed directly via GitHub API"</p>
                </div>
                <span class="badge">{format!("Role: {}", user.role.label())}</span>
            </div>
            <div class="console">
                <div class="card tight">
                    <div class="card-head" style="padding:14px 16px 8px;margin:0"><h2>"Repositories"</h2></div>
                    <div class="repo-picker">
                        {if repos.is_empty() {
                            view! { <div class="empty">"No repositories"</div> }.into_any()
                        } else {
                            repo_links.into_any()
                        }}
                    </div>
                </div>
                <div class="card chat">
                    <div class="chat-head">
                        <span class="badge">{head_badge}</span>
                        <span class="hint">"Prefix / is optional"</span>
                        <span class="spacer"></span>
                        <a class="hint" href="/admin/chat">"Clear view"</a>
                    </div>
                    <div class="chat-log" id="chat-log">{messages}</div>
                    <div class="quick">{quick}</div>
                    <form class="chat-input" id="cmd-form" onsubmit="return false;">
                        <input type="hidden" id="cmd-repo" value=selected/>
                        <textarea id="cmd-input" rows="1" placeholder=placeholder disabled=!has_repo></textarea>
                        <button class="btn primary" type="submit" disabled=!has_repo>"Send"</button>
                    </form>
                </div>
            </div>
        </Shell>
    }
}

// -------------------------------------------------------------------- config

pub fn config_page(
    user: CurrentUser,
    form: ConfigForm,
    toml_text: String,
    env_entries: Vec<EnvEntry>,
    backups: Vec<BackupInfo>,
    flash: Option<(bool, String)>,
) -> impl IntoView {
    let flash_view = match flash {
        Some((ok, text)) => view! { <Flash ok=ok text=text/> }.into_any(),
        None => view! { <span></span> }.into_any(),
    };

    let event_checks = form
        .events
        .iter()
        .map(|(name, enabled)| {
            let field = format!("event_{name}");
            view! {
                <label class="pill-check">
                    <input type="checkbox" name=field value="1" checked=*enabled/>
                    <span>{name.clone()}</span>
                </label>
            }
        })
        .collect_view();

    let env_rows = env_entries
        .iter()
        .map(|entry| {
            let key = entry.key.clone();
            let shown = if entry.secret {
                crate::web::config_store::mask(&entry.value)
            } else {
                entry.value.clone()
            };
            let input_type = if entry.secret { "password" } else { "text" };
            let placeholder = if entry.secret {
                "Leave blank to keep current value".to_string()
            } else {
                entry.value.clone()
            };
            view! {
                <tr>
                    <td class="mono">{key.clone()}</td>
                    <td class="secret-note">{shown}</td>
                    <td>
                        <input type=input_type name=key.clone() placeholder=placeholder autocomplete="off"/>
                    </td>
                </tr>
            }
        })
        .collect_view();

    let backup_rows = backups
        .iter()
        .map(|b| {
            let name = b.name.clone();
            view! {
                <tr>
                    <td class="mono">{name.clone()}</td>
                    <td class="num">{format!("{} B", b.size)}</td>
                    <td class="hint">{b.modified.clone()}</td>
                    <td>
                        <form method="post" action="/admin/config/restore" data-confirm="Restore this backup? Current configuration will be backed up first.">
                            <input type="hidden" name="name" value=name.clone()/>
                            <button class="btn sm ghost" type="submit">"Restore"</button>
                        </form>
                    </td>
                </tr>
            }
        })
        .collect_view();

    view! {
        <Shell title="Configuration".to_string() active="config".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Configuration"</div>
                    <h1>"Configuration Center"</h1>
                    <p>"Manage sugie.toml and .env online with schema validation, automatic backups, and atomic writes"</p>
                </div>
                <form method="post" action="/admin/system/restart" data-confirm="Are you sure you want to restart the service? The process will exit gracefully and be restarted by systemd / container runtime.">
                    <button class="btn" type="submit">"Restart Service"</button>
                </form>
            </div>
            {flash_view}
            <div class="alert warn">"Note: Saving writes changes to disk. The running process uses the configuration loaded at startup; restart the service to apply changes."</div>

            <div class="tabs">
                <button type="button" data-tab="basic" class="active">"General Settings"</button>
                <button type="button" data-tab="toml">"sugie.toml Source"</button>
                <button type="button" data-tab="env">".env Secrets"</button>
                <button type="button" data-tab="backup">"Backups"</button>
            </div>

            <div data-tab-panel="basic" class="active">
                <form method="post" action="/admin/config/structured">
                    <div class="grid cols-2">
                        <div class="card">
                            <div class="card-head"><h2>"Server"</h2></div>
                            <label class="field"><span>"Host"</span><input type="text" name="host" value=form.host.clone()/></label>
                            <label class="field"><span>"Port"</span><input type="number" name="port" value=form.port.clone()/></label>
                            <label class="field"><span>"Database Path"</span><input type="text" name="db_path" value=form.db_path.clone()/></label>
                            <label class="field">
                                <span>"Allowed Repositories (one per line, leave empty for all)"</span>
                                <textarea name="allowed_repositories" rows="4">{form.allowed_repositories.clone()}</textarea>
                            </label>
                        </div>
                        <div class="card">
                            <div class="card-head"><h2>"Access Control"</h2></div>
                            <label class="field">
                                <span>"Super Users (one per line)"</span>
                                <textarea name="super_users" rows="4">{form.super_users.clone()}</textarea>
                            </label>
                            <label class="field">
                                <span>"Restricted Commands (one per line)"</span>
                                <textarea name="restricted_commands" rows="8">{form.restricted_commands.clone()}</textarea>
                            </label>
                        </div>
                    </div>
                    <div class="grid cols-2" style="margin-top:16px">
                        <div class="card">
                            <div class="card-head"><h2>"Triage Rules"</h2></div>
                            <label class="pill-check" style="margin-bottom:10px">
                                <input type="checkbox" name="stale_enabled" value="1" checked=form.stale_enabled/>
                                <span>"Stale Lifecycle"</span>
                            </label>
                            <div class="row">
                                <label class="field" style="flex:1"><span>"Days Until Stale"</span><input type="number" name="stale_days" value=form.stale_days.clone()/></label>
                                <label class="field" style="flex:1"><span>"Check Interval (hours)"</span><input type="number" name="stale_interval" value=form.stale_interval.clone()/></label>
                            </div>
                            <label class="pill-check" style="margin-bottom:10px">
                                <input type="checkbox" name="reviewers_enabled" value="1" checked=form.reviewers_enabled/>
                                <span>"Auto-Assign Reviewers"</span>
                            </label>
                            <label class="field"><span>"Reviewers Per PR"</span><input type="number" name="reviewers_per_pr" value=form.reviewers_per_pr.clone()/></label>
                            <label class="pill-check">
                                <input type="checkbox" name="labels_enabled" value="1" checked=form.labels_enabled/>
                                <span>"Auto Labeling"</span>
                            </label>
                        </div>
                        <div class="card">
                            <div class="card-head"><h2>"Notification Events"</h2></div>
                            <div class="perm-list">{event_checks}</div>
                        </div>
                    </div>
                    <div class="row" style="margin-top:16px">
                        <button class="btn primary" type="submit">"Save Settings"</button>
                        <span class="hint">"Comments and formatting are preserved when saving"</span>
                    </div>
                </form>
            </div>

            <div data-tab-panel="toml">
                <form method="post" action="/admin/config/toml">
                    <div class="card">
                        <div class="card-head">
                            <h2>"sugie.toml"</h2>
                            <span class="hint">"Validated as SugieConfig before saving"</span>
                        </div>
                        <textarea class="code" name="content" spellcheck="false">{toml_text}</textarea>
                        <div class="row" style="margin-top:14px">
                            <button class="btn primary" type="submit">"Validate & Save"</button>
                        </div>
                    </div>
                </form>
            </div>

            <div data-tab-panel="env">
                <form method="post" action="/admin/config/env">
                    <div class="card tight">
                        <div class="card-head" style="padding:18px 20px 0">
                            <h2>".env Secrets"</h2>
                            <span class="hint">"Sensitive values are masked; leave blank to keep unchanged"</span>
                        </div>
                        <table style="margin-top:10px">
                            <thead><tr><th>"Key"</th><th>"Current Value"</th><th>"New Value"</th></tr></thead>
                            <tbody>{env_rows}</tbody>
                        </table>
                        <div class="row" style="padding:16px 20px">
                            <button class="btn primary" type="submit">"Save .env"</button>
                        </div>
                    </div>
                </form>
            </div>

            <div data-tab-panel="backup">
                <div class="card tight">
                    <div class="card-head" style="padding:18px 20px 0"><h2>"Configuration Backups"</h2></div>
                    <table style="margin-top:10px">
                        <thead><tr><th>"File"</th><th>"Size"</th><th>"Date"</th><th>""</th></tr></thead>
                        <tbody>{backup_rows}</tbody>
                    </table>
                </div>
            </div>
        </Shell>
    }
}

// --------------------------------------------------------------------- users

pub fn users_page(
    user: CurrentUser,
    users: Vec<UserCard>,
    repos: Vec<String>,
    global_2fa: bool,
    flash: Option<(bool, String)>,
) -> impl IntoView {
    let flash_view = match flash {
        Some((ok, text)) => view! { <Flash ok=ok text=text/> }.into_any(),
        None => view! { <span></span> }.into_any(),
    };

    let repo_hint = if repos.is_empty() {
        "(No connected repositories yet)".to_string()
    } else {
        repos.join("\n")
    };

    let is_owner = user.is_owner();

    let rows = users
        .iter()
        .map(|u| {
            let modal_id = format!("edit-modal-{}", u.id);
            let badge_cls = match u.role.as_str() {
                "owner" => "badge warn",
                "admin" => "badge ok",
                "maintainer" => "badge",
                _ => "badge muted",
            };
            let repos_txt = if u.repos.is_empty() {
                "(All)".to_string()
            } else {
                u.repos.join(", ")
            };
            let cmds_txt = if u.commands.is_empty() {
                "—".to_string()
            } else {
                format!("/{}", u.commands.join(", /"))
            };
            let status = if u.active { "Active" } else { "Disabled" };
            let last = if u.last_login.is_empty() { "Never".to_string() } else { u.last_login.clone() };
            let name = u.username.clone();
            let display = u.display_name.clone();
            let onclick = format!("document.getElementById('{}').showModal()", modal_id);

            let mut factors = Vec::new();
            if u.totp_enabled {
                factors.push("OTP".to_string());
            }
            if u.passkey_count > 0 {
                factors.push(format!("Passkey ×{}", u.passkey_count));
            }
            let enrolled_txt = if factors.is_empty() {
                "Not Enrolled".to_string()
            } else {
                factors.join(" + ")
            };
            let enrolled_cls = if factors.is_empty() {
                let effective_req = u.two_fa_policy == "required" || (u.two_fa_policy != "disabled" && global_2fa);
                if effective_req { "badge warn" } else { "badge muted" }
            } else {
                "badge ok"
            };
            let policy_label = match u.two_fa_policy.as_str() {
                "required" => "Enforced",
                "disabled" => "Disabled",
                _ => if global_2fa { "Global (Req)" } else { "Global (Opt)" },
            };

            let row_role = u.role.clone();
            let row_state = if u.active { "active" } else { "disabled" };
            let row_2fa = if u.totp_enabled || u.passkey_count > 0 { "enrolled" } else { "none" };
            view! {
                <tr data-role=row_role data-state=row_state data-twofa=row_2fa>
                    <td>
                        <div class="repo-name">{name.clone()}</div>
                        <div class="hint">{display.clone()}</div>
                    </td>
                    <td><span class=badge_cls>{u.role_label.clone()}</span></td>
                    <td>
                        <div><span class=enrolled_cls>{enrolled_txt}</span></div>
                        <div class="hint" style="margin-top:2px">{policy_label}</div>
                    </td>
                    <td class="hint mono">{repos_txt}</td>
                    <td class="hint mono">{cmds_txt}</td>
                    <td><span class=if u.active { "badge ok" } else { "badge muted" }>{status}</span></td>
                    <td class="hint">{last}</td>
                    <td>
                        <button type="button" class="btn sm" onclick=onclick>"Edit"</button>
                    </td>
                </tr>
            }
        })
        .collect_view();

    let dialogs = users
        .iter()
        .map(|u| {
            let id = u.id.to_string();
            let modal_id = format!("edit-modal-{}", u.id);
            let repos_value = u.repos.join("\n");
            let cmds_value = u.commands.join("\n");
            let name = u.username.clone();
            let display = u.display_name.clone();
            let close_click = format!("document.getElementById('{}').close()", modal_id);
            let has_enrolled = u.totp_enabled || u.passkey_count > 0;
            let enrolled_summary = format!(
                "OTP: {} · Passkeys: {}",
                if u.totp_enabled { "Enabled" } else { "Not set" },
                u.passkey_count
            );

            let reset_2fa_form = if has_enrolled {
                let id_for_reset = id.clone();
                view! {
                    <form method="post" action="/admin/users/reset-2fa" data-confirm="Reset all 2FA credentials (OTP & Passkeys) for this user? If 2FA is required, they will be prompted to register 2FA again on their next login.">
                        <input type="hidden" name="id" value=id_for_reset/>
                        <button class="btn sm" type="submit">"Reset User 2FA"</button>
                    </form>
                }
                .into_any()
            } else {
                view! { <span></span> }.into_any()
            };

            view! {
                <dialog id=modal_id.clone() class="modal-dialog" onclick="if(event.target===this)this.close()">
                    <div class="modal-body">
                        <div class="modal-head">
                            <div>
                                <h3>{format!("Edit User — {}", name)}</h3>
                                <span class="hint">{format!("User ID: #{} · {}", id, enrolled_summary)}</span>
                            </div>
                            <button type="button" class="icon-btn" onclick=close_click.clone() title="Close">"✕"</button>
                        </div>
                        <form method="post" action="/admin/users/update" class="stack">
                            <input type="hidden" name="id" value=id.clone()/>
                            <div class="grid cols-2">
                                <label class="field"><span>"Role"</span>
                                    <select name="role">
                                        <option value="viewer" selected=u.role == "viewer">"Viewer"</option>
                                        <option value="maintainer" selected=u.role == "maintainer">"Maintainer"</option>
                                        <option value="admin" selected=u.role == "admin">"Admin"</option>
                                        <option value="owner" selected=u.role == "owner">"Owner"</option>
                                    </select>
                                </label>
                                <label class="field"><span>"Display Name"</span><input type="text" name="display_name" value=display.clone()/></label>
                            </div>
                            <div class="grid cols-2">
                                <label class="field"><span>"Two-Factor Auth (2FA) Policy"</span>
                                    <select name="two_fa_policy" disabled=!is_owner>
                                        <option value="default" selected=u.two_fa_policy == "default">"Follow Global Policy"</option>
                                        <option value="required" selected=u.two_fa_policy == "required">"Enforce 2FA (Required)"</option>
                                        <option value="disabled" selected=u.two_fa_policy == "disabled">"Disable / Exempt 2FA"</option>
                                    </select>
                                </label>
                                <div style="display:flex;align-items:center;padding-top:14px">
                                    <label class="pill-check">
                                        <input type="checkbox" name="active" value="1" checked=u.active/>
                                        <span>"Active Account"</span>
                                    </label>
                                </div>
                            </div>
                            <label class="field"><span>"Allowed Repositories (one per line, empty for all)"</span>
                                <textarea name="repos" rows="3" placeholder=repo_hint.clone()>{repos_value}</textarea>
                            </label>
                            <label class="field"><span>"Additional Command Grants (one per line)"</span>
                                <textarea name="commands" rows="2" placeholder="retest\nstale">{cmds_value}</textarea>
                            </label>
                            <label class="field"><span>"Reset Password (leave blank to keep unchanged)"</span>
                                <input type="password" name="password" autocomplete="new-password" placeholder="••••••••"/>
                            </label>
                            <div class="row" style="margin-top:6px">
                                <button class="btn primary" type="submit">"Save Changes"</button>
                                <button type="button" class="btn ghost" onclick=close_click.clone()>"Cancel"</button>
                            </div>
                        </form>
                        <div style="margin-top:16px;padding-top:14px;border-top:1px dashed var(--border-mid);display:flex;justify-content:space-between;align-items:center;gap:10px">
                            <div>{reset_2fa_form}</div>
                            <form method="post" action="/admin/users/delete" data-confirm="Are you sure you want to delete this account? This action cannot be undone.">
                                <input type="hidden" name="id" value=id.clone()/>
                                <button class="btn sm danger" type="submit">"Delete Account"</button>
                            </form>
                        </div>
                    </div>
                </dialog>
            }
        })
        .collect_view();

    let global_2fa_card = {
        let status_badge = if global_2fa {
            view! { <span class="badge warn">"Global 2FA Enforced"</span> }.into_any()
        } else {
            view! { <span class="badge muted">"Global 2FA Optional"</span> }.into_any()
        };
        let toggle_controls = if is_owner {
            let next_val = if global_2fa { "0" } else { "1" };
            let btn_label = if global_2fa { "Disable Global 2FA Requirement" } else { "Enable Global 2FA for All Users" };
            let btn_cls = if global_2fa { "btn sm" } else { "btn sm primary" };
            view! {
                <form method="post" action="/admin/users/2fa-global" style="margin:0">
                    <input type="hidden" name="enabled" value=next_val/>
                    <button class=btn_cls type="submit">{btn_label}</button>
                </form>
            }
            .into_any()
        } else {
            view! { <span class="hint">"Only Owners can change the global 2FA policy"</span> }.into_any()
        };

        view! {
            <div class="card" style="margin-bottom:16px;display:flex;align-items:center;justify-content:space-between;gap:16px;flex-wrap:wrap">
                <div>
                    <div class="row" style="gap:10px;margin-bottom:4px">
                        <strong style="font-size:14px">"Global Two-Factor Authentication (2FA) Policy"</strong>
                        {status_badge}
                    </div>
                    <div class="hint">
                        "When enabled globally, all users (unless individually exempted) must register and verify 2FA via 8-digit SHA512 OTP or a Passkey upon sign-in."
                    </div>
                </div>
                {toggle_controls}
            </div>
        }
    };

    view! {
        <Shell title="Users".to_string() active="users".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Access"</div>
                    <h1>"Users & Permissions"</h1>
                    <p>"Roles determine baseline permissions; repository ACLs, command grants, and 2FA policies define security scope."</p>
                </div>
            </div>
            {flash_view}
            {global_2fa_card}
            <div class="stack">
                <div class="card tight">
                    <div class="toolbar">
                        <input type="search" placeholder="Search users…" data-filter-table="users-table" data-filter-kind="text" />
                        <select data-filter-table="users-table" data-filter-kind="role">
                            <option value="">"All roles"</option>
                            <option value="owner">"Owner"</option>
                            <option value="admin">"Admin"</option>
                            <option value="maintainer">"Maintainer"</option>
                            <option value="viewer">"Viewer"</option>
                        </select>
                        <select data-filter-table="users-table" data-filter-kind="state">
                            <option value="">"Any status"</option>
                            <option value="active">"Active"</option>
                            <option value="disabled">"Disabled"</option>
                        </select>
                        <select data-filter-table="users-table" data-filter-kind="twofa">
                            <option value="">"Any 2FA"</option>
                            <option value="enrolled">"2FA enrolled"</option>
                            <option value="none">"Not enrolled"</option>
                        </select>
                    </div>
                    <table id="users-table">
                        <thead><tr><th>"User"</th><th>"Role"</th><th>"2FA"</th><th>"Repositories"</th><th>"Commands"</th><th>"Status"</th><th>"Last Login"</th><th>""</th></tr></thead>
                        <tbody>{rows}</tbody>
                    </table>
                    {dialogs}
                </div>
                <div class="card" style="max-width:760px">
                    <div class="card-head"><h2>"Create User"</h2></div>
                    <form method="post" action="/admin/users/create" class="form-grid">
                        <label class="field"><span>"Username"</span><input type="text" name="username" required=""/></label>
                        <label class="field"><span>"Display Name"</span><input type="text" name="display_name"/></label>
                        <label class="field"><span>"Initial Password (min 8 chars)"</span><input type="password" name="password" required=""/></label>
                        <label class="field"><span>"Role"</span>
                            <select name="role">
                                <option value="viewer">"Viewer"</option>
                                <option value="maintainer">"Maintainer"</option>
                                <option value="admin">"Admin"</option>
                            </select>
                        </label>
                        <label class="field"><span>"Two-Factor Auth (2FA) Policy"</span>
                            <select name="two_fa_policy" disabled=!is_owner>
                                <option value="default">"Follow Global Policy"</option>
                                <option value="required">"Enforce 2FA (Required)"</option>
                                <option value="disabled">"Disable / Exempt 2FA"</option>
                            </select>
                        </label>
                        <label class="field span-2">
                            <span>"Allowed Repositories (one per line, leave empty for all)"</span>
                            <textarea name="repos" rows="3" placeholder=repo_hint></textarea>
                        </label>
                        <div class="span-2"><button class="btn primary" type="submit">"Create Account"</button></div>
                    </form>
                </div>
            </div>
        </Shell>
    }
}

// ------------------------------------------------------------------ security

pub fn security_page(
    user: CurrentUser,
    data: SecurityPageData,
    flash: Option<(bool, String)>,
) -> impl IntoView {
    let flash_view = match flash {
        Some((ok, text)) => view! { <Flash ok=ok text=text/> }.into_any(),
        None => view! { <span></span> }.into_any(),
    };

    let setup_banner = if user.needs_2fa_setup {
        view! {
            <div class="alert warn" style="border-width:1.5px;font-weight:500">
                "Two-Factor Authentication Setup Required: Your account policy requires 2FA before you can access the rest of the console. Please register either an 8-digit SHA512 Authenticator App (OTP) or a Passkey / Security Key below."
            </div>
        }
        .into_any()
    } else {
        view! { <span></span> }.into_any()
    };

    let formatted_secret = data
        .totp_setup_secret
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");

    let totp_card_body = if data.totp_enabled {
        view! {
            <div class="stack">
                <div class="alert ok" style="margin:0">
                    "Authenticator App (TOTP · SHA512 · 8 digits) is active on your account."
                </div>
                <p class="hint" style="margin:0">
                    "When signing in with your password, you can enter the 8-digit code generated by your authenticator app."
                </p>
                <form
                    method="post"
                    action="/admin/security/totp/disable"
                    data-confirm="Disable Authenticator App (OTP) on your account?"
                    style="border-top:1px solid var(--border);padding-top:14px"
                >
                    <label class="field">
                        <span>"Confirm Current Password to Disable OTP"</span>
                        <input type="password" name="password" autocomplete="current-password" required="" placeholder="Enter your password"/>
                    </label>
                    <button class="btn danger sm" type="submit">"Disable Authenticator OTP"</button>
                </form>
            </div>
        }
        .into_any()
    } else {
        let qr_src = if data.totp_setup_qr.starts_with("data:") {
            data.totp_setup_qr.clone()
        } else {
            format!("data:image/png;base64,{}", data.totp_setup_qr)
        };
        let raw_secret = data.totp_setup_secret.clone();
        let otpauth_uri = data.totp_setup_url.clone();
        view! {
            <div class="stack">
                <p class="hint" style="margin:0">
                    "Scan the QR code below with an authenticator app that supports SHA512 & 8-digit codes (such as FreeOTP+, Aegis, Ente Auth, 1Password, or Bitwarden), then enter the 8-digit code to activate."
                </p>
                <div style="display:flex;gap:18px;align-items:center;flex-wrap:wrap;background:var(--surface-alt);padding:16px;border-radius:16px;border:1px solid var(--border)">
                    <img
                        src=qr_src
                        alt="TOTP QR Code"
                        style="width:164px;height:164px;border-radius:12px;background:#fff;padding:8px;border:1px solid var(--border)"
                    />
                    <div style="flex:1;min-width:200px" class="stack">
                        <div>
                            <div class="eyebrow" style="margin-bottom:4px">"Manual Setup Key (Base32)"</div>
                            <div class="mono" style="font-size:13px;font-weight:600;word-break:break-all;padding:8px 10px;border-radius:10px;background:var(--surface-hi);border:1px solid var(--border)">
                                {formatted_secret}
                            </div>
                        </div>
                        <dl class="kv" style="grid-template-columns:95px 1fr;gap:4px 10px;font-size:12px">
                            <dt>"Issuer"</dt><dd class="mono">{data.rp_name.clone()}</dd>
                            <dt>"Algorithm"</dt><dd class="mono">"SHA512"</dd>
                            <dt>"Digits"</dt><dd class="mono">"8"</dd>
                            <dt>"Period"</dt><dd class="mono">"30s"</dd>
                        </dl>
                    </div>
                </div>
                <div>
                    <div class="eyebrow" style="margin-bottom:4px">"OTPAuth URI"</div>
                    <div class="mono" style="font-size:12px;word-break:break-all;padding:8px 10px;border-radius:10px;background:var(--surface-hi);border:1px solid var(--border);user-select:all">
                        {otpauth_uri}
                    </div>
                </div>
                <form method="post" action="/admin/security/totp/activate">
                    <input type="hidden" name="secret" value=raw_secret/>
                    <label class="field">
                        <span>"Verification Code (8 digits)"</span>
                        <input
                            type="text"
                            name="code"
                            inputmode="numeric"
                            pattern="[0-9]{8}"
                            maxlength="8"
                            placeholder="00000000"
                            autocomplete="one-time-code"
                            required=""
                            class="mono"
                            style="letter-spacing:.25em;font-size:16px"
                        />
                    </label>
                    <button class="btn primary" type="submit">"Verify & Activate OTP"</button>
                </form>
            </div>
        }
        .into_any()
    };

    let passkey_rows = if data.passkeys.is_empty() {
        view! {
            <tr><td colspan="4" class="empty" style="padding:20px">"No Passkeys or Security Keys registered yet"</td></tr>
        }
        .into_any()
    } else {
        data.passkeys
            .iter()
            .map(|pk| {
                let id_str = pk.id.to_string();
                let name = pk.name.clone();
                let created = pk.created_at.clone();
                let last_used = if pk.last_used_at.is_empty() {
                    "Never".to_string()
                } else {
                    pk.last_used_at.clone()
                };
                view! {
                    <tr>
                        <td style="font-weight:560">{name}</td>
                        <td class="hint mono">{created}</td>
                        <td class="hint mono">{last_used}</td>
                        <td style="text-align:right">
                            <form method="post" action="/admin/security/passkey/delete" data-confirm="Remove this Passkey from your account?" style="margin:0">
                                <input type="hidden" name="id" value=id_str/>
                                <button class="btn sm danger" type="submit">"Remove"</button>
                            </form>
                        </td>
                    </tr>
                }
            })
            .collect_view()
            .into_any()
    };

    let policy_badge = match user.two_fa_policy.as_str() {
        "required" => "Enforced by Owner",
        "disabled" => "Exempt / Disabled",
        _ => if data.global_2fa { "Required (Global Policy)" } else { "Optional (Global Policy)" },
    };

    view! {
        <Shell title="2FA & Passkeys".to_string() active="security".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Account Security"</div>
                    <h1>"Two-Factor Authentication & Passkeys"</h1>
                    <p>"Configure 8-digit SHA512 Time-based OTP and WebAuthn Passkeys / FIDO2 Security Keys."</p>
                </div>
                <span class="badge">{format!("2FA Policy: {policy_badge}")}</span>
            </div>
            {flash_view}
            {setup_banner}

            <div class="card" style="margin-bottom:16px">
                <div class="card-head" style="margin-bottom:10px">
                    <h2>"Relying Party (RP) Configuration"</h2>
                    <span class="hint">"Loaded from environment variables (.env)"</span>
                </div>
                <dl class="kv" style="grid-template-columns:130px 1fr">
                    <dt>"RP Name (Issuer)"</dt><dd class="mono">{data.rp_name.clone()}</dd>
                    <dt>"RP ID (Domain)"</dt><dd class="mono">{data.rp_id.clone()}</dd>
                    <dt>"RP Origin"</dt><dd class="mono">{data.rp_origin.clone()}</dd>
                </dl>
            </div>

            <div class="grid cols-2" style="align-items:start">
                <div class="card">
                    <div class="card-head">
                        <h2>"Authenticator App (OTP)"</h2>
                        <span class=if data.totp_enabled { "badge ok" } else { "badge muted" }>
                            {if data.totp_enabled { "Enabled" } else { "Not Configured" }}
                        </span>
                    </div>
                    {totp_card_body}
                </div>

                <div class="card">
                    <div class="card-head">
                        <h2>"Passkeys & Security Keys"</h2>
                        <span class=if data.passkeys.is_empty() { "badge muted" } else { "badge ok" }>
                            {format!("{} registered", data.passkeys.len())}
                        </span>
                    </div>
                    <p class="hint" style="margin-top:0">
                        "Register a Passkey (Touch ID, Face ID, Windows Hello, or a FIDO2 hardware key like YubiKey) to sign in without a password or complete 2FA verification in one tap."
                    </p>
                    <div id="passkey-reg-msg" class="alert" style="display:none"></div>
                    <div class="row" style="align-items:flex-end;margin-bottom:16px">
                        <label class="field" style="flex:1;margin-bottom:0">
                            <span>"Key / Device Nickname"</span>
                            <input type="text" id="passkey-device-name" placeholder="e.g. MacBook Touch ID, YubiKey 5C"/>
                        </label>
                        <button type="button" class="btn primary" id="btn-passkey-register">"Register Passkey"</button>
                    </div>
                    <div style="border:1px solid var(--border);border-radius:14px;overflow:hidden">
                        <table>
                            <thead>
                                <tr><th>"Name"</th><th>"Added"</th><th>"Last Used"</th><th>""</th></tr>
                            </thead>
                            <tbody>{passkey_rows}</tbody>
                        </table>
                    </div>
                </div>
            </div>
        </Shell>
    }
}

// --------------------------------------------------------------------- audit

pub fn audit_page(
    user: CurrentUser,
    tab: String,
    rows: Vec<AuditCard>,
    http_rows: Vec<SecurityHttpCard>,
    page: i64,
    total: i64,
    log_file_path: String,
    log_settings: crate::security_log::SecurityLogSettings,
    flash: Option<(bool, String)>,
) -> impl IntoView {
    let is_http_tab = tab == "inbound" || tab == "github" || tab == "notify" || tab == "http_all";
    let flash_view = flash.map(|(ok, msg)| {
        let cls = if ok { "alert ok" } else { "alert err" };
        view! { <div class=cls>{msg}</div> }
    });

    let prev = format!("/admin/audit?tab={}&page={}", urlencode(&tab), (page - 1).max(0));
    let next = format!("/admin/audit?tab={}&page={}", urlencode(&tab), page + 1);
    let pages = (total + 49) / 50;

    let table_view = if is_http_tab {
        let http_body = if http_rows.is_empty() {
            view! { <tr><td colspan="8" class="empty">"No HTTP security logs recorded yet"</td></tr> }.into_any()
        } else {
            http_rows
                .iter()
                .map(|r| {
                    let dir_cls = if r.direction == "OUTBOUND_GITHUB" {
                        "badge"
                    } else if r.direction == "OUTBOUND_NOTIFY" {
                        "badge warn"
                    } else {
                        "badge muted"
                    };
                    let dir_label = if r.direction == "OUTBOUND_GITHUB" {
                        "GitHub API"
                    } else if r.direction == "OUTBOUND_NOTIFY" {
                        "Notification"
                    } else {
                        "Inbound API"
                    };
                    let status_cls = if (200..400).contains(&r.status) {
                        "badge ok"
                    } else if (400..500).contains(&r.status) {
                        "badge warn"
                    } else {
                        "badge bad"
                    };
                    let status_txt = if r.status == 0 {
                        "ERR".to_string()
                    } else {
                        r.status.to_string()
                    };
                    view! {
                        <tr>
                            <td class="hint mono nowrap">{r.ts.clone()}</td>
                            <td><span class=dir_cls>{dir_label}</span></td>
                            <td class="mono" style="font-weight:600">{r.method.clone()}</td>
                            <td class="mono" style="word-break:break-all;max-width:360px">{r.target.clone()}</td>
                            <td><span class=status_cls>{status_txt}</span></td>
                            <td class="num">{format!("{} ms", r.duration_ms)}</td>
                            <td class="hint mono">{format!("{} ({})", r.actor, r.ip)}</td>
                            <td class="hint mono" style="word-break:break-word;max-width:260px">{r.detail.clone()}</td>
                        </tr>
                    }
                })
                .collect_view()
                .into_any()
        };

        view! {
            <table>
                <thead>
                    <tr>
                        <th>"Timestamp"</th>
                        <th>"Direction"</th>
                        <th>"Method"</th>
                        <th>"URL / Path"</th>
                        <th>"Status"</th>
                        <th>"Latency"</th>
                        <th>"Actor / IP"</th>
                        <th>"Details"</th>
                    </tr>
                </thead>
                <tbody>{http_body}</tbody>
            </table>
        }
        .into_any()
    } else {
        let body = if rows.is_empty() {
            view! { <tr><td colspan="7" class="empty">"No audit logs recorded yet"</td></tr> }.into_any()
        } else {
            rows.iter()
                .map(|r| {
                    let cls = if r.ok { "badge ok" } else { "badge bad" };
                    let txt = if r.ok { "Success" } else { "Denied" };
                    view! {
                        <tr>
                            <td class="hint mono nowrap">{r.ts.clone()}</td>
                            <td class="nowrap">{r.username.clone()}</td>
                            <td class="nowrap"><span class="badge muted">{r.action.clone()}</span></td>
                            <td class="mono">{r.target.clone()}</td>
                            <td class="hint">{r.detail.clone()}</td>
                            <td class="hint mono">{r.ip.clone()}</td>
                            <td><span class=cls>{txt}</span></td>
                        </tr>
                    }
                })
                .collect_view()
                .into_any()
        };

        view! {
            <table>
                <thead>
                    <tr>
                        <th>"Timestamp"</th>
                        <th>"User"</th>
                        <th>"Action"</th>
                        <th>"Target"</th>
                        <th>"Details"</th>
                        <th>"IP"</th>
                        <th>"Result"</th>
                    </tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        }
        .into_any()
    };

    let tab_audit_cls = if tab == "audit" { "btn sm primary" } else { "btn sm ghost" };
    let tab_inbound_cls = if tab == "inbound" { "btn sm primary" } else { "btn sm ghost" };
    let tab_github_cls = if tab == "github" { "btn sm primary" } else { "btn sm ghost" };
    let tab_notify_cls = if tab == "notify" { "btn sm primary" } else { "btn sm ghost" };
    let tab_all_cls = if tab == "http_all" { "btn sm primary" } else { "btn sm ghost" };
    let tab_hidden = tab.clone();
    let tab_hidden2 = tab.clone();
    let export_href = format!("/admin/audit/export?tab={}", urlencode(&tab));

    view! {
        <Shell title="Audit".to_string() active="audit".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Security & Audit"</div>
                    <h1>"Security Review & Audit Logs"</h1>
                    <p>
                        {format!("{total} total records · Page {} of {} · Persistent HTTP log file: ", page + 1, pages.max(1))}
                        <code class="mono">{log_file_path}</code>
                    </p>
                </div>
                <div class="row">
                    <a class="btn sm ghost" href=prev>"Previous"</a>
                    <a class="btn sm ghost" href=next>"Next"</a>
                </div>
            </div>
            {flash_view}
            <div class="card" style="margin-bottom:20px">
                <form method="post" action="/admin/audit/settings">
                    <input type="hidden" name="tab" value=tab_hidden />
                    <div class="card-head" style="margin-bottom:12px">
                        <div>
                            <div class="card-title">"Security Log Collection Settings"</div>
                            <div class="card-sub">"Configure which log categories are recorded to SQLite and security_http.log (all enabled by default). Changes take effect immediately."</div>
                        </div>
                        <button type="submit" class="btn sm primary">"Save Logging Settings"</button>
                    </div>
                    <div class="check-grid">
                        <label class="check">
                            <input type="checkbox" name="admin_audit" checked=log_settings.admin_audit />
                            <span>"Admin Portal Action Audit (Logins, Users, 2FA, Config)"</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="inbound_webhooks" checked=log_settings.inbound_webhooks />
                            <span>"Inbound Webhook & Bot API Requests (/webhook, /api/chat/*)"</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="inbound_admin" checked=log_settings.inbound_admin />
                            <span>"Inbound Admin Console HTTP Requests (/admin/*)"</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="outbound_github" checked=log_settings.outbound_github />
                            <span>"Outbound GitHub REST & GraphQL Requests"</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="outbound_lark" checked=log_settings.outbound_lark />
                            <span>"Outbound Lark (Feishu) Notifications"</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="outbound_discord" checked=log_settings.outbound_discord />
                            <span>"Outbound Discord Notifications"</span>
                        </label>
                        <label class="check">
                            <input type="checkbox" name="outbound_zulip" checked=log_settings.outbound_zulip />
                            <span>"Outbound Zulip Notifications"</span>
                        </label>
                    </div>
                    <label class="field" style="margin:14px 0 0;max-width:420px">
                        <span>"Auto-delete HTTP security logs older than N days (0 = keep forever)"</span>
                        <input type="number" name="retention_days" min="0" max="36500" value=log_settings.retention_days.to_string() />
                    </label>
                </form>
                <div class="row" style="margin-top:14px;padding-top:14px;border-top:1px dashed var(--border-mid);justify-content:space-between">
                    <form method="post" action="/admin/audit/purge" class="row" data-confirm="Permanently delete HTTP security log records older than the specified number of days? (The security_http.log file on disk is not modified.)" style="margin:0">
                        <input type="hidden" name="tab" value=tab_hidden2 />
                        <span class="hint">"Clear logs older than"</span>
                        <input type="number" name="days" min="0" value="30" style="width:90px" />
                        <span class="hint">"days"</span>
                        <button type="submit" class="btn sm danger">"Clear Now"</button>
                    </form>
                    <a class="btn sm" href=export_href>"Export CSV"</a>
                </div>
            </div>
            <div class="row" style="margin-bottom:16px">
                <a class=tab_audit_cls href="/admin/audit?tab=audit">"Admin Action Audit"</a>
                <a class=tab_inbound_cls href="/admin/audit?tab=inbound">"Inbound API & Webhook Requests"</a>
                <a class=tab_github_cls href="/admin/audit?tab=github">"Outbound GitHub API Requests"</a>
                <a class=tab_notify_cls href="/admin/audit?tab=notify">"Outbound Notifications (Lark / Discord / Zulip)"</a>
                <a class=tab_all_cls href="/admin/audit?tab=http_all">"All HTTP Traffic"</a>
            </div>
            <div class="card tight">
                {table_view}
            </div>
        </Shell>
    }
}

/// GitHub returns HTTP 403 "This installation has been suspended" for suspended app installations.
fn is_suspended_error(err: &str) -> bool {
    err.to_ascii_lowercase().contains("suspended")
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

// --------------------------------------------------------------------- notify

pub fn notify_page(
    user: CurrentUser,
    discord_ok: bool,
    lark_ok: bool,
    zulip_ok: bool,
    flash: Option<(bool, String)>,
) -> impl IntoView {
    let flash_view = match flash {
        Some((ok, text)) => view! { <Flash ok=ok text=text/> }.into_any(),
        None => view! { <span></span> }.into_any(),
    };
    let status = |ok: bool| if ok { "Configured" } else { "Not configured" };
    let any_ok = discord_ok || lark_ok || zulip_ok;

    view! {
        <Shell title="Notify".to_string() active="notify".to_string() user=Some(user.clone())>
            <div class="page-head">
                <div>
                    <div class="eyebrow">"Broadcast"</div>
                    <h1>"Send Notification"</h1>
                    <p>"Send a manual announcement to Lark, Discord and/or Zulip. Bypasses event toggles and repository filters."</p>
                </div>
            </div>
            {flash_view}
            <div class="card" style="max-width:760px">
                <form method="post" action="/admin/notify/send" class="form-grid">
                    <label class="field span-2">
                        <span>"Title"</span>
                        <input type="text" name="title" required="" maxlength="200" placeholder="Maintenance window tonight"/>
                    </label>
                    <label class="field span-2">
                        <span>"Message"</span>
                        <textarea name="message" rows="5" required="" maxlength="4000" placeholder="Details of the announcement…"></textarea>
                    </label>
                    <label class="field">
                        <span>"Link (optional)"</span>
                        <input type="text" name="url" placeholder="https://github.com/org/repo/issues/1"/>
                    </label>
                    <label class="field">
                        <span>"Severity"</span>
                        <select name="level">
                            <option value="info">"Info"</option>
                            <option value="success">"Success"</option>
                            <option value="warning">"Warning"</option>
                            <option value="error">"Error"</option>
                        </select>
                    </label>
                    <div class="span-2" style="margin-bottom:16px">
                        <div class="hint" style="margin-bottom:8px;font-weight:560">"Send to"</div>
                        <div class="check-grid">
                            <label class="check">
                                <input type="checkbox" name="ch_lark" checked=lark_ok disabled=!lark_ok/>
                                <span>{format!("Lark (Feishu) — {}", status(lark_ok))}</span>
                            </label>
                            <label class="check">
                                <input type="checkbox" name="ch_discord" checked=discord_ok disabled=!discord_ok/>
                                <span>{format!("Discord — {}", status(discord_ok))}</span>
                            </label>
                            <label class="check">
                                <input type="checkbox" name="ch_zulip" checked=zulip_ok disabled=!zulip_ok/>
                                <span>{format!("Zulip — {}", status(zulip_ok))}</span>
                            </label>
                        </div>
                    </div>
                    <div class="span-2">
                        <button class="btn primary" type="submit" disabled=!any_ok>"Send Notification"</button>
                    </div>
                </form>
            </div>
        </Shell>
    }
}

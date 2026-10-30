//! Security review logger for all inbound HTTP API requests and all outbound
//! GitHub API requests.
//!
//! Records every event to:
//! 1. A persistent JSONL log file on disk (`data/security_http.log`, or
//!    `SUGIE_SECURITY_LOG_PATH` if set in `.env`),
//! 2. The `security_http_logs` table in SQLite (`data/sugie.db`) for inspection
//!    in the Admin Console (`/admin/audit`),
//! 3. Structured `tracing` output (`target: "sugie::security_audit"`).

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Arc, Mutex, OnceLock,
};

pub const KEY_SECURITY_LOG_SETTINGS: &str = "security_log_settings";

/// Configurable toggles controlling which security & audit logs are recorded.
/// By default, all log categories are enabled (`true`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SecurityLogSettings {
    #[serde(default = "default_true")]
    pub admin_audit: bool,
    #[serde(default = "default_true")]
    pub inbound_webhooks: bool,
    #[serde(default = "default_true")]
    pub inbound_admin: bool,
    #[serde(default = "default_true")]
    pub outbound_github: bool,
    #[serde(default = "default_true")]
    pub outbound_lark: bool,
    #[serde(default = "default_true")]
    pub outbound_discord: bool,
    #[serde(default = "default_true")]
    pub outbound_zulip: bool,
    /// Automatically delete `security_http_logs` rows older than this many days (0 = keep forever).
    #[serde(default)]
    pub retention_days: i64,
}

fn default_true() -> bool {
    true
}

impl Default for SecurityLogSettings {
    fn default() -> Self {
        Self {
            admin_audit: true,
            inbound_webhooks: true,
            inbound_admin: true,
            outbound_github: true,
            outbound_lark: true,
            outbound_discord: true,
            outbound_zulip: true,
            retention_days: 0,
        }
    }
}

static LOG_ADMIN_AUDIT: AtomicBool = AtomicBool::new(true);
static LOG_INBOUND_WEBHOOKS: AtomicBool = AtomicBool::new(true);
static LOG_INBOUND_ADMIN: AtomicBool = AtomicBool::new(true);
static LOG_OUTBOUND_GITHUB: AtomicBool = AtomicBool::new(true);
static LOG_OUTBOUND_LARK: AtomicBool = AtomicBool::new(true);
static LOG_OUTBOUND_DISCORD: AtomicBool = AtomicBool::new(true);
static LOG_OUTBOUND_ZULIP: AtomicBool = AtomicBool::new(true);
static RETENTION_DAYS: AtomicI64 = AtomicI64::new(0);
static LAST_PURGE_TS: AtomicI64 = AtomicI64::new(0);

pub fn current_settings() -> SecurityLogSettings {
    SecurityLogSettings {
        admin_audit: LOG_ADMIN_AUDIT.load(Ordering::Relaxed),
        inbound_webhooks: LOG_INBOUND_WEBHOOKS.load(Ordering::Relaxed),
        inbound_admin: LOG_INBOUND_ADMIN.load(Ordering::Relaxed),
        outbound_github: LOG_OUTBOUND_GITHUB.load(Ordering::Relaxed),
        outbound_lark: LOG_OUTBOUND_LARK.load(Ordering::Relaxed),
        outbound_discord: LOG_OUTBOUND_DISCORD.load(Ordering::Relaxed),
        outbound_zulip: LOG_OUTBOUND_ZULIP.load(Ordering::Relaxed),
        retention_days: RETENTION_DAYS.load(Ordering::Relaxed),
    }
}

pub fn apply_settings(s: SecurityLogSettings) {
    LOG_ADMIN_AUDIT.store(s.admin_audit, Ordering::Relaxed);
    LOG_INBOUND_WEBHOOKS.store(s.inbound_webhooks, Ordering::Relaxed);
    LOG_INBOUND_ADMIN.store(s.inbound_admin, Ordering::Relaxed);
    LOG_OUTBOUND_GITHUB.store(s.outbound_github, Ordering::Relaxed);
    LOG_OUTBOUND_LARK.store(s.outbound_lark, Ordering::Relaxed);
    LOG_OUTBOUND_DISCORD.store(s.outbound_discord, Ordering::Relaxed);
    LOG_OUTBOUND_ZULIP.store(s.outbound_zulip, Ordering::Relaxed);
    RETENTION_DAYS.store(s.retention_days.max(0), Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize)]
pub struct SecurityHttpLogEntry {
    pub ts: String,
    pub direction: String, // "INBOUND_API" | "OUTBOUND_GITHUB"
    pub method: String,
    pub target: String,
    pub status: u16,
    pub duration_ms: u64,
    pub ip: String,
    pub actor: String,
    pub detail: String,
}

struct SecurityLogger {
    conn: Mutex<Connection>,
    log_file_path: PathBuf,
    file_lock: Mutex<()>,
}

static LOGGER: OnceLock<Mutex<Option<Arc<SecurityLogger>>>> = OnceLock::new();

fn global_slot() -> &'static Mutex<Option<Arc<SecurityLogger>>> {
    LOGGER.get_or_init(|| Mutex::new(None))
}

/// Resolve the log file path on disk (`SUGIE_SECURITY_LOG_PATH` or `<db_dir>/security_http.log`).
pub fn resolve_log_file_path(db_path: &str) -> PathBuf {
    if let Ok(custom) = std::env::var("SUGIE_SECURITY_LOG_PATH") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let db_p = Path::new(db_path);
    if let Some(parent) = db_p.parent() {
        if !parent.as_os_str().is_empty() {
            return parent.join("security_http.log");
        }
    }
    PathBuf::from("data/security_http.log")
}

/// Initialize the security HTTP logger backed by the SQLite database at `db_path`
/// and the JSONL log file on disk.
pub fn init(db_path: &str) {
    if db_path == ":memory:" {
        return;
    }
    let log_file_path = resolve_log_file_path(db_path);
    if let Some(parent) = log_file_path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Some(parent) = Path::new(db_path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let Ok(conn) = Connection::open(db_path) else {
        return;
    };
    let _ = conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA busy_timeout = 5000;
        CREATE TABLE IF NOT EXISTS security_http_logs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            ts          TEXT NOT NULL,
            direction   TEXT NOT NULL,
            method      TEXT NOT NULL,
            target      TEXT NOT NULL,
            status      INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            ip          TEXT NOT NULL DEFAULT '-',
            actor       TEXT NOT NULL DEFAULT '-',
            detail      TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_sec_http_logs_ts ON security_http_logs (id DESC);
        CREATE INDEX IF NOT EXISTS idx_sec_http_logs_dir ON security_http_logs (direction, id DESC);
        CREATE TABLE IF NOT EXISTS web_settings (
            key        TEXT PRIMARY KEY,
            value      TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        "#,
    );

    // Load persisted logging settings (or seed defaults with all categories enabled)
    let loaded = conn
        .query_row(
            "SELECT value FROM web_settings WHERE key = ?1",
            params![KEY_SECURITY_LOG_SETTINGS],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<SecurityLogSettings>(&raw).ok());

    match loaded {
        Some(s) => apply_settings(s),
        None => {
            let defaults = SecurityLogSettings::default();
            apply_settings(defaults);
            if let Ok(json_str) = serde_json::to_string(&defaults) {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO web_settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
                    params![KEY_SECURITY_LOG_SETTINGS, json_str, Utc::now().to_rfc3339()],
                );
            }
        }
    }

    tracing::info!(
        log_file = %log_file_path.display(),
        db_path = %db_path,
        "Security HTTP audit logger initialized"
    );

    if let Ok(mut guard) = global_slot().lock() {
        *guard = Some(Arc::new(SecurityLogger {
            conn: Mutex::new(conn),
            log_file_path,
            file_lock: Mutex::new(()),
        }));
    }
}

pub fn record(entry: SecurityHttpLogEntry) {
    tracing::info!(
        target: "sugie::security_audit",
        direction = %entry.direction,
        method = %entry.method,
        url = %entry.target,
        status = entry.status,
        duration_ms = entry.duration_ms,
        ip = %entry.ip,
        actor = %entry.actor,
        detail = %entry.detail,
        "security_http_log"
    );

    let logger = {
        let Ok(guard) = global_slot().lock() else {
            return;
        };
        let Some(ref logger) = *guard else {
            return;
        };
        Arc::clone(logger)
    };

    // 1. Append JSON line to security_http.log
    if let Ok(_fguard) = logger.file_lock.lock() {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&logger.log_file_path)
        {
            if let Ok(line) = serde_json::to_string(&entry) {
                let _ = writeln!(file, "{line}");
            }
        }
    }

    // 2. Insert into SQLite security_http_logs table (and opportunistically apply retention hourly)
    if let Ok(conn) = logger.conn.lock() {
        let days = RETENTION_DAYS.load(Ordering::Relaxed);
        if days > 0 {
            let now = Utc::now().timestamp();
            let last = LAST_PURGE_TS.load(Ordering::Relaxed);
            if now - last >= 3600 {
                LAST_PURGE_TS.store(now, Ordering::Relaxed);
                let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
                let _ = conn.execute(
                    "DELETE FROM security_http_logs WHERE ts < ?1",
                    params![cutoff],
                );
            }
        }
        let _ = conn.execute(
            "INSERT INTO security_http_logs (ts, direction, method, target, status, duration_ms, ip, actor, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.ts,
                entry.direction,
                entry.method,
                entry.target,
                entry.status as i64,
                entry.duration_ms as i64,
                entry.ip,
                entry.actor,
                entry.detail,
            ],
        );
    };
}

/// Record an inbound HTTP request to our server / API / webhook / admin console.
pub fn log_inbound_http(
    method: &str,
    uri: &str,
    status: u16,
    duration_ms: u64,
    ip: &str,
    actor: &str,
    detail: &str,
) {
    let is_admin_route = uri == "/"
        || uri.starts_with("/admin")
        || uri.starts_with("/login")
        || uri.starts_with("/logout");
    if is_admin_route {
        if !LOG_INBOUND_ADMIN.load(Ordering::Relaxed) {
            return;
        }
    } else if !LOG_INBOUND_WEBHOOKS.load(Ordering::Relaxed) {
        return;
    }

    record(SecurityHttpLogEntry {
        ts: Utc::now().to_rfc3339(),
        direction: "INBOUND_API".to_string(),
        method: method.to_string(),
        target: uri.to_string(),
        status,
        duration_ms,
        ip: ip.to_string(),
        actor: actor.to_string(),
        detail: truncate_detail(detail, 1000),
    });
}

/// Record an outbound HTTP request to the GitHub REST or GraphQL API.
pub fn log_github_request(method: &str, url: &str, status: u16, duration_ms: u64, detail: &str) {
    if !LOG_OUTBOUND_GITHUB.load(Ordering::Relaxed) {
        return;
    }
    record(SecurityHttpLogEntry {
        ts: Utc::now().to_rfc3339(),
        direction: "OUTBOUND_GITHUB".to_string(),
        method: method.to_string(),
        target: url.to_string(),
        status,
        duration_ms,
        ip: "-".to_string(),
        actor: "sugie-bot".to_string(),
        detail: truncate_detail(detail, 1000),
    });
}

/// Record an outbound notification request to Lark, Discord, or Zulip.
pub fn log_notification_request(
    platform: &str,
    method: &str,
    url: &str,
    status: u16,
    duration_ms: u64,
    detail: &str,
) {
    let enabled = match platform {
        "lark" => LOG_OUTBOUND_LARK.load(Ordering::Relaxed),
        "discord" => LOG_OUTBOUND_DISCORD.load(Ordering::Relaxed),
        "zulip" => LOG_OUTBOUND_ZULIP.load(Ordering::Relaxed),
        _ => true,
    };
    if !enabled {
        return;
    }
    record(SecurityHttpLogEntry {
        ts: Utc::now().to_rfc3339(),
        direction: "OUTBOUND_NOTIFY".to_string(),
        method: method.to_string(),
        target: url.to_string(),
        status,
        duration_ms,
        ip: "-".to_string(),
        actor: format!("{platform}-notifier"),
        detail: truncate_detail(detail, 1000),
    });
}

fn truncate_detail(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let head: String = s.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

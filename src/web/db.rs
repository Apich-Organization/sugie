//! Persistent storage for the admin console: users, sessions, per-repo ACLs,
//! per-command grants, 2FA (TOTP + WebAuthn passkeys), and the audit trail.
//!
//! Tables are namespaced with `web_` and live in the same SQLite file as the
//! bot's own storage, so operations only ever have to back up one file.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const KEY_GLOBAL_2FA_ENABLED: &str = "global_2fa_enabled";
/// `web_settings` key holding how many bot exchanges to keep. `0` keeps all.
pub const KEY_CHAT_HISTORY_KEEP: &str = "chat_history_keep";
/// `web_settings` key holding how many console turns to keep **per user**.
/// `0` keeps all.
pub const KEY_CONSOLE_HISTORY_KEEP: &str = "console_history_keep";

/// Default for both history-retention settings: keep everything.
///
/// Deleting a conversation is the operator's call, not a side effect of hitting
/// an arbitrary row count. Both console pages expose a knob for installs that
/// would rather cap it — chat history in total, console history per user.
const HISTORY_KEEP_DEFAULT: i64 = 0;

#[derive(Clone)]
pub struct WebDb {
    conn: Arc<Mutex<Connection>>,
    /// Cached copy of `web_settings.global_2fa_enabled` so resolving a principal
    /// does not need a second round-trip on every request.
    global_2fa: Arc<std::sync::atomic::AtomicBool>,
    /// Cached copies of the two history-retention settings. Both tables are
    /// written per request, so neither limit is re-queried on insert.
    chat_keep: Arc<std::sync::atomic::AtomicI64>,
    console_keep: Arc<std::sync::atomic::AtomicI64>,
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub active: bool,
    pub created_at: String,
    pub last_login: Option<String>,
    pub totp_enabled: bool,
    /// `"default"` (follow global setting), `"required"` (always enforce 2FA), or `"disabled"` (exempt from 2FA).
    pub two_fa_policy: String,
    pub passkey_count: i64,
}

impl UserRow {
    pub fn has_2fa_enrolled(&self) -> bool {
        self.totp_enabled || self.passkey_count > 0
    }

    pub fn is_2fa_disabled(&self) -> bool {
        self.two_fa_policy == "disabled"
    }

    pub fn is_2fa_required(&self, global_2fa: bool) -> bool {
        match self.two_fa_policy.as_str() {
            "required" => true,
            "disabled" => false,
            _ => global_2fa,
        }
    }

    pub fn needs_2fa_setup(&self, global_2fa: bool) -> bool {
        self.is_2fa_required(global_2fa) && !self.has_2fa_enrolled()
    }
}

/// A resolved console session: the account plus the repository and command
/// grants it holds. Named because the tuple otherwise shows up in every
/// signature that carries it.
pub type SessionPrincipal = (UserRow, Vec<String>, Vec<String>);

#[derive(Debug, Clone, Serialize)]
pub struct PasskeyRow {
    pub id: i64,
    pub user_id: i64,
    pub credential_id: String,
    pub device_name: String,
    pub passkey_json: String,
    pub counter: i64,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: i64,
    pub ts: String,
    pub username: String,
    pub action: String,
    pub target: String,
    pub detail: String,
    pub ip: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct AuditFilter {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub result: String,
}

impl AuditFilter {
    pub fn is_empty(&self) -> bool {
        self.ts.trim().is_empty()
            && self.username.trim().is_empty()
            && self.action.trim().is_empty()
            && self.target.trim().is_empty()
            && self.detail.trim().is_empty()
            && self.ip.trim().is_empty()
            && self.result.trim().is_empty()
    }
}

/// One turn of the console conversation: the command a user submitted and the
/// output the server streamed back.
///
/// The in-memory `JobRegistry` only keeps a short, volatile window of this, so
/// the same exchange is also written to SQLite. That is what lets `/admin/chat`
/// reopen an older conversation after a restart.
#[derive(Debug, Clone, Serialize)]
pub struct ConsoleHistoryRow {
    pub id: i64,
    pub job_id: String,
    pub username: String,
    pub repo: String,
    pub command: String,
    /// `running`, `done`, `failed`, `cancelled` or `interrupted`.
    pub status: String,
    pub lines: Vec<crate::web::exec::LogLine>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl ConsoleHistoryRow {
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }
}

/// One exchange with the chat bot: what an external platform sent and what the
/// bot answered.
///
/// Unlike the console's own history, these messages originate outside the admin
/// console (Zulip / Lark / Discord / the generic API), so they are kept in their
/// own table and surfaced on `/admin/chats`.
#[derive(Debug, Clone, Serialize)]
pub struct ChatLogRow {
    pub id: i64,
    pub ts: String,
    /// `zulip`, `lark`, `discord` or `api`.
    pub platform: String,
    pub username: String,
    pub display_name: String,
    /// Stream / group / channel the message arrived in, when the platform says.
    pub channel: String,
    pub command: String,
    pub reply: String,
    /// `ok`, `warn` or `error` — see [`crate::chat::reply_status`].
    pub status: String,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ChatLogFilter {
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub ts: String,
    /// Matches either the incoming command or the bot's reply.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub status: String,
}

impl ChatLogFilter {
    pub fn is_empty(&self) -> bool {
        self.platform.trim().is_empty()
            && self.username.trim().is_empty()
            && self.ts.trim().is_empty()
            && self.text.trim().is_empty()
            && self.status.trim().is_empty()
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct SecurityHttpFilter {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub detail: String,
}

impl SecurityHttpFilter {
    pub fn is_empty(&self) -> bool {
        self.ts.trim().is_empty()
            && self.direction.trim().is_empty()
            && self.method.trim().is_empty()
            && self.target.trim().is_empty()
            && self.status.trim().is_empty()
            && self.actor.trim().is_empty()
            && self.ip.trim().is_empty()
            && self.detail.trim().is_empty()
    }
}

const USER_SELECT_SQL: &str = "
    SELECT u.id, u.username, u.display_name, u.role, u.active, u.created_at, u.last_login,
           u.totp_enabled, u.two_fa_policy,
           (SELECT COUNT(*) FROM web_passkeys p WHERE p.user_id = u.id) AS passkey_count
    FROM web_users u
";

impl WebDb {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            global_2fa: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            chat_keep: Arc::new(std::sync::atomic::AtomicI64::new(HISTORY_KEEP_DEFAULT)),
            console_keep: Arc::new(std::sync::atomic::AtomicI64::new(HISTORY_KEEP_DEFAULT)),
        };
        db.init()?;
        db.refresh_global_2fa();
        db.refresh_history_keep();
        Ok(db)
    }

    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            global_2fa: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            chat_keep: Arc::new(std::sync::atomic::AtomicI64::new(HISTORY_KEEP_DEFAULT)),
            console_keep: Arc::new(std::sync::atomic::AtomicI64::new(HISTORY_KEEP_DEFAULT)),
        };
        db.init()?;
        db.refresh_global_2fa();
        db.refresh_history_keep();
        Ok(db)
    }

    /// Take the connection lock without propagating mutex poisoning: a panic in
    /// one handler must not permanently brick every later request.
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn refresh_global_2fa(&self) {
        let enabled = self.read_global_2fa();
        self.global_2fa
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    fn read_global_2fa(&self) -> bool {
        let conn = self.lock_conn();
        conn.query_row(
            "SELECT value FROM web_settings WHERE key = ?1",
            params![KEY_GLOBAL_2FA_ENABLED],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|v| v == "true" || v == "1")
        // Default off. Forcing 2FA on a brand-new install bounced every page to
        // /admin/security, which locks the first owner out of the console they
        // just bootstrapped. The dashboard nudges instead.
        .unwrap_or(false)
    }

    fn init(&self) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS web_users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT UNIQUE NOT NULL,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                last_login TEXT,
                totp_secret TEXT NOT NULL DEFAULT '',
                totp_enabled INTEGER NOT NULL DEFAULT 0,
                two_fa_policy TEXT NOT NULL DEFAULT 'default',
                failed_attempts INTEGER NOT NULL DEFAULT 0,
                locked_until TEXT
            );

            CREATE TABLE IF NOT EXISTS web_sessions (
                token_hash TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                ip TEXT NOT NULL DEFAULT '',
                user_agent TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS web_pending_2fa (
                token_hash TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL,
                next_url TEXT NOT NULL DEFAULT '/admin',
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                ip TEXT NOT NULL DEFAULT '',
                user_agent TEXT NOT NULL DEFAULT '',
                attempts INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS web_passkeys (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                credential_id TEXT UNIQUE NOT NULL,
                device_name TEXT NOT NULL DEFAULT 'Passkey',
                passkey_json TEXT NOT NULL,
                counter INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                last_used_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_web_passkeys_user ON web_passkeys(user_id);

            CREATE TABLE IF NOT EXISTS web_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS web_repo_perms (
                user_id INTEGER NOT NULL,
                repo TEXT NOT NULL,
                PRIMARY KEY (user_id, repo)
            );

            CREATE TABLE IF NOT EXISTS web_command_perms (
                user_id INTEGER NOT NULL,
                command TEXT NOT NULL,
                PRIMARY KEY (user_id, command)
            );

            CREATE TABLE IF NOT EXISTS web_console_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL,
                username TEXT NOT NULL,
                repo TEXT NOT NULL DEFAULT '',
                command TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',
                lines_json TEXT NOT NULL DEFAULT '[]',
                started_at TEXT NOT NULL,
                finished_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_web_console_history_user
                ON web_console_history(username, id DESC);

            CREATE UNIQUE INDEX IF NOT EXISTS idx_web_console_history_job
                ON web_console_history(job_id);

            CREATE TABLE IF NOT EXISTS web_chat_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                platform TEXT NOT NULL,
                sender_id TEXT NOT NULL DEFAULT '',
                username TEXT NOT NULL DEFAULT '',
                display_name TEXT NOT NULL DEFAULT '',
                channel TEXT NOT NULL DEFAULT '',
                message_id TEXT NOT NULL DEFAULT '',
                command TEXT NOT NULL,
                reply TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'ok',
                duration_ms INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_web_chat_history_ts
                ON web_chat_history(id DESC);
            CREATE INDEX IF NOT EXISTS idx_web_chat_history_platform
                ON web_chat_history(platform, id DESC);
            CREATE INDEX IF NOT EXISTS idx_web_chat_history_user
                ON web_chat_history(username, id DESC);

            CREATE TABLE IF NOT EXISTS web_audit (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                username TEXT NOT NULL,
                action TEXT NOT NULL,
                target TEXT NOT NULL DEFAULT '',
                detail TEXT NOT NULL DEFAULT '',
                ip TEXT NOT NULL DEFAULT '',
                ok INTEGER NOT NULL DEFAULT 1
            );

            CREATE INDEX IF NOT EXISTS idx_web_audit_ts ON web_audit(ts DESC);

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
            "#,
        )?;

        // Idempotent column migrations for existing databases
        let _ = conn.execute(
            "ALTER TABLE web_users ADD COLUMN totp_secret TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE web_users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE web_users ADD COLUMN two_fa_policy TEXT NOT NULL DEFAULT 'default'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE web_users ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE web_users ADD COLUMN locked_until TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE web_pending_2fa ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
            [],
        );

        // Usernames are looked up with COLLATE NOCASE, so the uniqueness
        // constraint must be case-insensitive too — otherwise `Admin` and
        // `admin` can coexist and login picks whichever row SQLite returns first.
        let _ = conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_web_users_username_nocase
             ON web_users(username COLLATE NOCASE)",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sec_http_logs_ts_asc ON security_http_logs (ts)",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_web_audit_action ON web_audit(action, id DESC)",
            [],
        );
        // Actor lookups are a hot filter in /admin/audit and had no index. The
        // `ts` indexes for both tables already exist above.
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_web_audit_username ON web_audit(username, id DESC)",
            [],
        );

        // Seed the global 2FA policy. Off by default: a fresh install must be
        // usable before the owner has enrolled a second factor. Owners are
        // nudged on the dashboard and can turn it on in one click.
        let _ = conn.execute(
            "INSERT OR IGNORE INTO web_settings (key, value, updated_at) VALUES (?1, 'false', ?2)",
            params![KEY_GLOBAL_2FA_ENABLED, Utc::now().to_rfc3339()],
        );

        // Auto-migrate legacy Chinese defaults from early database initializations
        let _ = conn.execute(
            "UPDATE web_users SET display_name = 'System Administrator' WHERE display_name = '系统管理员'",
            [],
        );
        let _ = conn.execute(
            "UPDATE web_audit SET detail = 'Successful login' WHERE detail = '登录成功'",
            [],
        );

        Ok(())
    }

    // ------------------------------------------------------------- settings

    pub fn is_global_2fa_enabled(&self) -> bool {
        self.global_2fa.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_global_2fa_enabled(&self, enabled: bool) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO web_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![
                KEY_GLOBAL_2FA_ENABLED,
                if enabled { "true" } else { "false" },
                Utc::now().to_rfc3339()
            ],
        )?;
        self.global_2fa
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Bot exchanges to keep, across all platforms. `0` keeps every one.
    pub fn chat_history_keep(&self) -> i64 {
        self.chat_keep.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Console turns to keep **per user**. `0` keeps every one.
    pub fn console_history_keep(&self) -> i64 {
        self.console_keep.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set the chat-history limit and apply it immediately, so lowering the
    /// value takes visible effect instead of waiting for the next bot message.
    pub fn set_chat_history_keep(&self, keep: i64) -> Result<()> {
        let keep = self.save_keep(KEY_CHAT_HISTORY_KEEP, &self.chat_keep, keep)?;
        if keep > 0 {
            let conn = self.lock_conn();
            prune_chat_history(&conn, keep);
        }
        Ok(())
    }

    /// Set the per-user console limit and trim every account immediately.
    pub fn set_console_history_keep(&self, keep: i64) -> Result<()> {
        let keep = self.save_keep(KEY_CONSOLE_HISTORY_KEEP, &self.console_keep, keep)?;
        if keep > 0 {
            let conn = self.lock_conn();
            prune_console_history(&conn, keep);
        }
        Ok(())
    }

    /// Persist one retention value and refresh its in-memory copy. Returns the
    /// normalised value (`0` for "keep everything") so the caller can decide
    /// whether a prune is needed.
    fn save_keep(&self, key: &str, slot: &std::sync::atomic::AtomicI64, keep: i64) -> Result<i64> {
        let keep = keep.max(0);
        {
            let conn = self.lock_conn();
            conn.execute(
                "INSERT INTO web_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, keep.to_string(), Utc::now().to_rfc3339()],
            )?;
        }
        slot.store(keep, std::sync::atomic::Ordering::Relaxed);
        Ok(keep)
    }

    fn refresh_history_keep(&self) {
        let chat = self.read_keep(KEY_CHAT_HISTORY_KEEP);
        let console = self.read_keep(KEY_CONSOLE_HISTORY_KEEP);
        self.chat_keep
            .store(chat, std::sync::atomic::Ordering::Relaxed);
        self.console_keep
            .store(console, std::sync::atomic::Ordering::Relaxed);
    }

    fn read_keep(&self, key: &str) -> i64 {
        let conn = self.lock_conn();
        conn.query_row(
            "SELECT value FROM web_settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(HISTORY_KEEP_DEFAULT)
        .max(0)
    }

    pub fn get_security_log_settings(&self) -> crate::security_log::SecurityLogSettings {
        let conn = self.lock_conn();
        conn.query_row(
            "SELECT value FROM web_settings WHERE key = ?1",
            params![crate::security_log::KEY_SECURITY_LOG_SETTINGS],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<crate::security_log::SecurityLogSettings>(&v).ok())
        .unwrap_or_default()
    }

    pub fn set_security_log_settings(
        &self,
        settings: &crate::security_log::SecurityLogSettings,
    ) -> Result<()> {
        let json_str = serde_json::to_string(settings)?;
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO web_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![
                crate::security_log::KEY_SECURITY_LOG_SETTINGS,
                json_str,
                Utc::now().to_rfc3339()
            ],
        )?;
        crate::security_log::apply_settings(*settings);
        Ok(())
    }

    // ---------------------------------------------------------------- users

    pub fn count_users(&self) -> Result<i64> {
        let conn = self.lock_conn();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM web_users", [], |r| r.get(0))?;
        Ok(n)
    }

    pub fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: &str,
        display_name: &str,
    ) -> Result<i64> {
        self.create_user_with_policy(username, password_hash, role, display_name, "default")
    }

    pub fn create_user_with_policy(
        &self,
        username: &str,
        password_hash: &str,
        role: &str,
        display_name: &str,
        two_fa_policy: &str,
    ) -> Result<i64> {
        let policy = normalize_policy(two_fa_policy);
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO web_users (username, password_hash, role, display_name, active, created_at, two_fa_policy)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
            params![username, password_hash, role, display_name, Utc::now().to_rfc3339(), policy],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn find_user(&self, username: &str) -> Result<Option<UserRow>> {
        let conn = self.lock_conn();
        let sql = format!("{USER_SELECT_SQL} WHERE u.username = ?1 COLLATE NOCASE");
        let row = conn
            .query_row(&sql, params![username], map_user)
            .optional()?;
        Ok(row)
    }

    pub fn get_user(&self, id: i64) -> Result<Option<UserRow>> {
        let conn = self.lock_conn();
        let sql = format!("{USER_SELECT_SQL} WHERE u.id = ?1");
        let row = conn.query_row(&sql, params![id], map_user).optional()?;
        Ok(row)
    }

    pub fn password_hash(&self, user_id: i64) -> Result<Option<String>> {
        let conn = self.lock_conn();
        let hash = conn
            .query_row(
                "SELECT password_hash FROM web_users WHERE id = ?1",
                params![user_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(hash)
    }

    pub fn list_users(&self) -> Result<Vec<UserRow>> {
        let conn = self.lock_conn();
        let sql = format!("{USER_SELECT_SQL} ORDER BY u.id ASC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([], map_user)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn update_user_profile(
        &self,
        id: i64,
        role: &str,
        display_name: &str,
        active: bool,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_users SET role = ?2, display_name = ?3, active = ?4 WHERE id = ?1",
            params![id, role, display_name, active as i64],
        )?;
        Ok(())
    }

    pub fn set_user_two_fa_policy(&self, id: i64, policy: &str) -> Result<()> {
        let p = normalize_policy(policy);
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_users SET two_fa_policy = ?2 WHERE id = ?1",
            params![id, p],
        )?;
        Ok(())
    }

    pub fn set_password(&self, id: i64, hash: &str) -> Result<()> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE web_users SET password_hash = ?2, failed_attempts = 0, locked_until = NULL WHERE id = ?1",
            params![id, hash],
        )?;
        // A password rotation must invalidate every other session, otherwise a
        // stolen cookie keeps working after the user "secures" the account.
        tx.execute("DELETE FROM web_sessions WHERE user_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM web_pending_2fa WHERE user_id = ?1",
            params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_user(&self, id: i64) -> Result<()> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM web_repo_perms WHERE user_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM web_command_perms WHERE user_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM web_sessions WHERE user_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM web_pending_2fa WHERE user_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM web_passkeys WHERE user_id = ?1", params![id])?;
        tx.execute("DELETE FROM web_users WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn touch_login(&self, id: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_users SET last_login = ?2 WHERE id = ?1",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Drop every live session for a user. Called whenever credentials, role or
    /// 2FA enrolment change so a stolen cookie cannot outlive the change.
    pub fn delete_user_sessions(&self, user_id: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "DELETE FROM web_sessions WHERE user_id = ?1",
            params![user_id],
        )?;
        conn.execute(
            "DELETE FROM web_pending_2fa WHERE user_id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    pub fn count_owners(&self) -> Result<i64> {
        let conn = self.lock_conn();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM web_users WHERE role = 'owner' AND active = 1",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    // ------------------------------------------------------ login throttling

    /// Seconds until the account may try again, or `None` when it is not locked.
    pub fn login_lockout_remaining(&self, username: &str) -> Result<Option<i64>> {
        let conn = self.lock_conn();
        let locked: Option<Option<String>> = conn
            .query_row(
                "SELECT locked_until FROM web_users WHERE username = ?1 COLLATE NOCASE",
                params![username],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?;
        let Some(Some(raw)) = locked else {
            return Ok(None);
        };
        let until = chrono::DateTime::parse_from_rfc3339(&raw)
            .map(|d| d.with_timezone(&Utc))
            .ok();
        match until {
            Some(t) if t > Utc::now() => Ok(Some((t - Utc::now()).num_seconds().max(1))),
            _ => Ok(None),
        }
    }

    /// Increment the failure counter and lock the account once `threshold` is
    /// reached. Returns the lock duration in seconds when the account just got
    /// locked.
    pub fn record_login_failure(
        &self,
        username: &str,
        threshold: i64,
        lock_secs: i64,
    ) -> Result<Option<i64>> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_users SET failed_attempts = failed_attempts + 1
             WHERE username = ?1 COLLATE NOCASE",
            params![username],
        )?;
        let attempts: Option<i64> = conn
            .query_row(
                "SELECT failed_attempts FROM web_users WHERE username = ?1 COLLATE NOCASE",
                params![username],
                |r| r.get(0),
            )
            .optional()?;
        if attempts.unwrap_or(0) >= threshold {
            let until = Utc::now() + chrono::Duration::seconds(lock_secs);
            conn.execute(
                "UPDATE web_users SET locked_until = ?2, failed_attempts = 0
                 WHERE username = ?1 COLLATE NOCASE",
                params![username, until.to_rfc3339()],
            )?;
            return Ok(Some(lock_secs));
        }
        Ok(None)
    }

    pub fn clear_login_failures(&self, user_id: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_users SET failed_attempts = 0, locked_until = NULL WHERE id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    /// Count a wrong OTP for a pending challenge. Returns the running total so
    /// the caller can invalidate the challenge after `max` attempts.
    pub fn bump_pending_2fa_attempts(&self, token_hash: &str) -> Result<i64> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_pending_2fa SET attempts = attempts + 1 WHERE token_hash = ?1",
            params![token_hash],
        )?;
        let n: Option<i64> = conn
            .query_row(
                "SELECT attempts FROM web_pending_2fa WHERE token_hash = ?1",
                params![token_hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(n.unwrap_or(0))
    }

    /// Resolve session → user → grants in a single round-trip. This is the hot
    /// path for every admin request, so it must not fan out into four queries.
    pub fn session_principal(&self, token_hash: &str) -> Result<Option<SessionPrincipal>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                "SELECT u.id, u.username, u.display_name, u.role, u.active, u.created_at,
                        u.last_login, u.totp_enabled, u.two_fa_policy,
                        (SELECT COUNT(*) FROM web_passkeys p WHERE p.user_id = u.id),
                        (SELECT GROUP_CONCAT(repo, char(31)) FROM web_repo_perms r WHERE r.user_id = u.id),
                        (SELECT GROUP_CONCAT(command, char(31)) FROM web_command_perms c WHERE c.user_id = u.id),
                        s.expires_at
                 FROM web_sessions s JOIN web_users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1",
                params![token_hash],
                |r| {
                    let user = UserRow {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        display_name: r.get(2)?,
                        role: r.get(3)?,
                        active: r.get::<_, i64>(4)? == 1,
                        created_at: r.get(5)?,
                        last_login: r.get(6)?,
                        totp_enabled: r.get::<_, i64>(7)? == 1,
                        two_fa_policy: r.get(8)?,
                        passkey_count: r.get(9)?,
                    };
                    let repos: Option<String> = r.get(10)?;
                    let commands: Option<String> = r.get(11)?;
                    let expires_at: String = r.get(12)?;
                    Ok((user, split_grants(repos), split_grants(commands), expires_at))
                },
            )
            .optional()?;

        let Some((user, repos, commands, expires_at)) = row else {
            return Ok(None);
        };

        let expired = chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map(|d| d.with_timezone(&Utc) < Utc::now())
            .unwrap_or(true);
        if expired {
            conn.execute(
                "DELETE FROM web_sessions WHERE token_hash = ?1",
                params![token_hash],
            )?;
            return Ok(None);
        }
        if !user.active {
            return Ok(None);
        }
        Ok(Some((user, repos, commands)))
    }

    /// All repo grants in one query, keyed by user id — avoids the N+1 on the
    /// users page.
    pub fn repo_perms_all(&self) -> Result<HashMap<i64, Vec<String>>> {
        let conn = self.lock_conn();
        let mut stmt =
            conn.prepare("SELECT user_id, repo FROM web_repo_perms ORDER BY user_id, repo")?;
        let mut map: HashMap<i64, Vec<String>> = HashMap::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (uid, repo) = row?;
            map.entry(uid).or_default().push(repo);
        }
        Ok(map)
    }

    pub fn command_perms_all(&self) -> Result<HashMap<i64, Vec<String>>> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare("SELECT user_id, command FROM web_command_perms ORDER BY user_id, command")?;
        let mut map: HashMap<i64, Vec<String>> = HashMap::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (uid, cmd) = row?;
            map.entry(uid).or_default().push(cmd);
        }
        Ok(map)
    }

    // --------------------------------------------------------- 2FA: TOTP

    pub fn get_totp_secret(&self, user_id: i64) -> Result<Option<(String, bool)>> {
        let conn = self.lock_conn();
        let row = conn
            .query_row(
                "SELECT totp_secret, totp_enabled FROM web_users WHERE id = ?1",
                params![user_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? == 1)),
            )
            .optional()?;
        Ok(row)
    }

    pub fn activate_totp(&self, user_id: i64, secret: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_users SET totp_secret = ?2, totp_enabled = 1 WHERE id = ?1",
            params![user_id, secret],
        )?;
        Ok(())
    }

    pub fn disable_totp(&self, user_id: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_users SET totp_secret = '', totp_enabled = 0 WHERE id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    pub fn reset_user_2fa(&self, user_id: i64) -> Result<()> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE web_users SET totp_secret = '', totp_enabled = 0 WHERE id = ?1",
            params![user_id],
        )?;
        tx.execute(
            "DELETE FROM web_passkeys WHERE user_id = ?1",
            params![user_id],
        )?;
        // Reset means "the old factor is gone" — force a fresh login so a
        // session created with the removed factor cannot linger.
        tx.execute(
            "DELETE FROM web_sessions WHERE user_id = ?1",
            params![user_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    // --------------------------------------------------------- 2FA: Passkeys

    pub fn list_user_passkeys(&self, user_id: i64) -> Result<Vec<PasskeyRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, credential_id, device_name, passkey_json, counter, created_at, last_used_at
             FROM web_passkeys WHERE user_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt
            .query_map(params![user_id], map_passkey)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn list_all_passkeys(&self) -> Result<Vec<PasskeyRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, credential_id, device_name, passkey_json, counter, created_at, last_used_at
             FROM web_passkeys ORDER BY id DESC",
        )?;
        let rows = stmt
            .query_map([], map_passkey)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn save_passkey(
        &self,
        user_id: i64,
        credential_id: &str,
        device_name: &str,
        passkey_json: &str,
        counter: i64,
    ) -> Result<i64> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO web_passkeys (user_id, credential_id, device_name, passkey_json, counter, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                user_id,
                credential_id,
                device_name,
                passkey_json,
                counter,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_passkey_usage(
        &self,
        credential_id: &str,
        counter: i64,
        passkey_json: &str,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE web_passkeys SET counter = ?2, passkey_json = ?3, last_used_at = ?4
             WHERE credential_id = ?1",
            params![
                credential_id,
                counter,
                passkey_json,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn delete_passkey(&self, id: i64, user_id: i64) -> Result<bool> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "DELETE FROM web_passkeys WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        Ok(affected > 0)
    }

    // ------------------------------------------------------------- sessions

    pub fn create_session(
        &self,
        token_hash: &str,
        user_id: i64,
        ttl_hours: i64,
        ip: &str,
        user_agent: &str,
    ) -> Result<()> {
        let conn = self.lock_conn();
        let now = Utc::now();
        let expires = now + chrono::Duration::hours(ttl_hours);
        conn.execute(
            "INSERT INTO web_sessions (token_hash, user_id, created_at, expires_at, ip, user_agent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                token_hash,
                user_id,
                now.to_rfc3339(),
                expires.to_rfc3339(),
                ip,
                user_agent
            ],
        )?;
        Ok(())
    }

    /// Returns the user for a live session token hash, purging it when expired.
    pub fn session_user(&self, token_hash: &str) -> Result<Option<UserRow>> {
        let conn = self.lock_conn();
        let found: Option<(String, i64)> = conn
            .query_row(
                "SELECT expires_at, user_id FROM web_sessions WHERE token_hash = ?1",
                params![token_hash],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;

        let Some((expires_at, user_id)) = found else {
            return Ok(None);
        };

        let expired = chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map(|d| d.with_timezone(&Utc) < Utc::now())
            .unwrap_or(true);

        if expired {
            conn.execute(
                "DELETE FROM web_sessions WHERE token_hash = ?1",
                params![token_hash],
            )?;
            return Ok(None);
        }

        let sql = format!("{USER_SELECT_SQL} WHERE u.id = ?1");
        let row = conn
            .query_row(&sql, params![user_id], map_user)
            .optional()?;
        Ok(row.filter(|u| u.active))
    }

    pub fn delete_session(&self, token_hash: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "DELETE FROM web_sessions WHERE token_hash = ?1",
            params![token_hash],
        )?;
        Ok(())
    }

    pub fn purge_sessions(&self) -> Result<()> {
        let conn = self.lock_conn();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "DELETE FROM web_sessions WHERE expires_at < ?1",
            params![now],
        )?;
        conn.execute(
            "DELETE FROM web_pending_2fa WHERE expires_at < ?1",
            params![now],
        )?;
        Ok(())
    }

    // --------------------------------------------------------- pending 2FA

    pub fn create_pending_2fa(
        &self,
        token_hash: &str,
        user_id: i64,
        next_url: &str,
        ip: &str,
        user_agent: &str,
    ) -> Result<()> {
        let conn = self.lock_conn();
        let now = Utc::now();
        let expires = now + chrono::Duration::minutes(5);
        conn.execute(
            "DELETE FROM web_pending_2fa WHERE expires_at < ?1",
            params![now.to_rfc3339()],
        )?;
        conn.execute(
            "INSERT INTO web_pending_2fa (token_hash, user_id, next_url, created_at, expires_at, ip, user_agent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                token_hash,
                user_id,
                next_url,
                now.to_rfc3339(),
                expires.to_rfc3339(),
                ip,
                user_agent
            ],
        )?;
        Ok(())
    }

    pub fn get_pending_2fa(&self, token_hash: &str) -> Result<Option<(UserRow, String)>> {
        let conn = self.lock_conn();
        let found: Option<(String, i64, String)> = conn
            .query_row(
                "SELECT expires_at, user_id, next_url FROM web_pending_2fa WHERE token_hash = ?1",
                params![token_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let Some((expires_at, user_id, next_url)) = found else {
            return Ok(None);
        };

        let expired = chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map(|d| d.with_timezone(&Utc) < Utc::now())
            .unwrap_or(true);

        if expired {
            conn.execute(
                "DELETE FROM web_pending_2fa WHERE token_hash = ?1",
                params![token_hash],
            )?;
            return Ok(None);
        }

        let sql = format!("{USER_SELECT_SQL} WHERE u.id = ?1");
        let row = conn
            .query_row(&sql, params![user_id], map_user)
            .optional()?;
        Ok(row.filter(|u| u.active).map(|u| (u, next_url)))
    }

    pub fn consume_pending_2fa(&self, token_hash: &str) -> Result<Option<(UserRow, String)>> {
        let conn = self.lock_conn();
        // Consume the token in one statement: the old check-then-delete let two
        // concurrent OTP submissions both read the row and both mint a session.
        let consumed: Option<(i64, String, String)> = conn
            .query_row(
                "DELETE FROM web_pending_2fa WHERE token_hash = ?1
                 RETURNING user_id, next_url, expires_at",
                params![token_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let Some((user_id, next_url, expires_at)) = consumed else {
            return Ok(None);
        };

        let expired = chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map(|d| d.with_timezone(&Utc) < Utc::now())
            .unwrap_or(true);
        if expired {
            return Ok(None);
        }

        let sql = format!("{USER_SELECT_SQL} WHERE u.id = ?1");
        let row = conn
            .query_row(&sql, params![user_id], map_user)
            .optional()?;
        Ok(row.filter(|u| u.active).map(|u| (u, next_url)))
    }

    // --------------------------------------------------------------- perms

    pub fn set_repo_perms(&self, user_id: i64, repos: &[String]) -> Result<()> {
        let conn = self.lock_conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM web_repo_perms WHERE user_id = ?1",
            params![user_id],
        )?;
        for repo in repos {
            tx.execute(
                "INSERT OR IGNORE INTO web_repo_perms (user_id, repo) VALUES (?1, ?2)",
                params![user_id, repo],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn repo_perms(&self, user_id: i64) -> Result<Vec<String>> {
        let conn = self.lock_conn();
        let mut stmt =
            conn.prepare("SELECT repo FROM web_repo_perms WHERE user_id = ?1 ORDER BY repo")?;
        let rows = stmt
            .query_map(params![user_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn set_command_perms(&self, user_id: i64, commands: &[String]) -> Result<()> {
        let conn = self.lock_conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM web_command_perms WHERE user_id = ?1",
            params![user_id],
        )?;
        for cmd in commands {
            tx.execute(
                "INSERT OR IGNORE INTO web_command_perms (user_id, command) VALUES (?1, ?2)",
                params![user_id, cmd],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn command_perms(&self, user_id: i64) -> Result<Vec<String>> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare("SELECT command FROM web_command_perms WHERE user_id = ?1 ORDER BY command")?;
        let rows = stmt
            .query_map(params![user_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ----------------------------------------------------- console history

    /// Record a command as soon as it is dispatched, so a run that is stopped or
    /// dies with the process still leaves a visible turn in the conversation.
    pub fn console_history_start(
        &self,
        job_id: &str,
        username: &str,
        repo: &str,
        command: &str,
        started_at: &str,
    ) {
        // History is a convenience, never a reason to fail a command: every
        // failure here is swallowed, exactly like `audit`.
        let conn = self.lock_conn();
        let _ = conn.execute(
            "INSERT OR IGNORE INTO web_console_history
                 (job_id, username, repo, command, status, lines_json, started_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, 'running', '[]', ?5, NULL)",
            params![job_id, username, repo, command, started_at],
        );
        // Retention is operator-controlled and off by default, so nothing is
        // deleted unless an admin asked for a cap.
        let keep = self.console_history_keep();
        if keep > 0 {
            // The subquery resolves to NULL while the user has fewer rows than
            // the cap, and `id <= NULL` matches nothing, so no delete happens
            // until the account is actually full.
            let _ = conn.execute(
                "DELETE FROM web_console_history
                  WHERE username = ?1
                    AND id <= (SELECT id FROM web_console_history
                                WHERE username = ?1
                                ORDER BY id DESC LIMIT 1 OFFSET ?2)",
                params![username, keep],
            );
        }
    }

    /// Store the reply for a job, replacing whatever placeholder the start wrote.
    pub fn console_history_finish(
        &self,
        job_id: &str,
        status: &str,
        lines: &[crate::web::exec::LogLine],
        finished_at: &str,
    ) {
        let json = serde_json::to_string(lines).unwrap_or_else(|_| "[]".to_string());
        let conn = self.lock_conn();
        let _ = conn.execute(
            "UPDATE web_console_history
                SET status = ?2, lines_json = ?3, finished_at = ?4
              WHERE job_id = ?1",
            params![job_id, status, json, finished_at],
        );
    }

    /// Close out turns that were still marked `running` when the process exited.
    ///
    /// The job registry lives in memory, so after a restart no turn can
    /// legitimately still be running: any row left open belongs to a run the
    /// server died in the middle of, and leaving it `running` would show a Stop
    /// button that can only ever answer 409.
    pub fn console_history_mark_interrupted(&self) {
        let conn = self.lock_conn();
        let _ = conn.execute(
            "UPDATE web_console_history
                SET status = 'interrupted', finished_at = COALESCE(finished_at, ?1)
              WHERE status = 'running'",
            params![Utc::now().to_rfc3339()],
        );
    }

    /// One page of a user's conversation, newest turn first.
    ///
    /// `before` is the cursor: pass the `id` of the oldest turn currently on
    /// screen to fetch the page behind it. An `id` cursor rather than an offset
    /// keeps paging stable while new commands are being appended.
    pub fn console_history_page(
        &self,
        username: &str,
        before: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ConsoleHistoryRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, job_id, username, repo, command, status, lines_json, started_at,
                    finished_at
               FROM web_console_history
              WHERE username = ?1 AND (?2 IS NULL OR id < ?2)
              ORDER BY id DESC
              LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![username, before, limit as i64], |r| {
                let lines_json: String = r.get(6)?;
                Ok(ConsoleHistoryRow {
                    id: r.get(0)?,
                    job_id: r.get(1)?,
                    username: r.get(2)?,
                    repo: r.get(3)?,
                    command: r.get(4)?,
                    status: r.get(5)?,
                    // A row written by an older build, or truncated by a crash,
                    // must degrade to "no output" rather than blank the page.
                    lines: serde_json::from_str(&lines_json).unwrap_or_default(),
                    started_at: r.get(7)?,
                    finished_at: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // -------------------------------------------------------- chat history

    /// Record one inbound bot message together with the reply it produced.
    ///
    /// Deliberately best-effort: an external chat platform is waiting on the
    /// answer, so a storage problem must never turn into a failed webhook.
    pub fn record_chat_exchange(
        &self,
        msg: &crate::chat::InboundChatMessage,
        reply: &str,
        duration_ms: i64,
    ) {
        let conn = self.lock_conn();
        let _ = conn.execute(
            "INSERT INTO web_chat_history
                 (ts, platform, sender_id, username, display_name, channel, message_id,
                  command, reply, status, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                Utc::now().to_rfc3339(),
                msg.platform.as_str(),
                msg.sender.id,
                msg.sender.username,
                msg.sender.display_name.as_deref().unwrap_or(""),
                msg.stream_or_channel.as_deref().unwrap_or(""),
                msg.message_id.as_deref().unwrap_or(""),
                // A single chat message can be arbitrarily long; cap both sides
                // so one paste cannot dominate the table.
                truncate(&msg.raw_content, 2000),
                truncate(reply, 8000),
                crate::chat::reply_status(reply),
                duration_ms,
            ],
        );
        // Retention is operator-controlled and off by default, so a fresh
        // install never silently loses a conversation.
        let keep = self.chat_history_keep();
        if keep > 0 {
            prune_chat_history(&conn, keep);
        }
    }

    pub fn list_chat_logs_filtered(
        &self,
        filter: &ChatLogFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ChatLogRow>> {
        let conn = self.lock_conn();
        let mut sql = "SELECT id, ts, platform, username, display_name, channel, command, reply,
                              status, duration_ms
                         FROM web_chat_history WHERE 1=1"
            .to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        chat_filter_clauses(&mut sql, &mut params_vec, filter);
        sql.push_str(" ORDER BY id DESC LIMIT ? OFFSET ?");
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(&params_refs[..], |r| {
                Ok(ChatLogRow {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    platform: r.get(2)?,
                    username: r.get(3)?,
                    display_name: r.get(4)?,
                    channel: r.get(5)?,
                    command: r.get(6)?,
                    reply: r.get(7)?,
                    status: r.get(8)?,
                    duration_ms: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn count_chat_logs_filtered(&self, filter: &ChatLogFilter) -> Result<i64> {
        let conn = self.lock_conn();
        let mut sql = "SELECT COUNT(*) FROM web_chat_history WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        chat_filter_clauses(&mut sql, &mut params_vec, filter);
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let count = conn.query_row(&sql, &params_refs[..], |r| r.get::<_, i64>(0))?;
        Ok(count)
    }

    // --------------------------------------------------------------- audit

    pub fn audit(
        &self,
        username: &str,
        action: &str,
        target: &str,
        detail: &str,
        ip: &str,
        ok: bool,
    ) {
        // Authentication and privilege changes are always recorded: letting the
        // toggle silence them would be an anti-forensics switch for an attacker
        // who already holds an admin account. Configuration writes, dispatched
        // commands, repository syncs and audit-settings changes are in the same
        // class — they explain *why* the system changed, not just who signed in.
        let always = action.starts_with("login")
            || action.starts_with("2fa.")
            || action.starts_with("user.")
            || action.starts_with("security.")
            || action.starts_with("config.")
            || action.starts_with("command")
            || action.starts_with("repos.sync")
            || action.starts_with("audit.");
        if !always && !crate::security_log::current_settings().admin_audit {
            return;
        }
        // Audit must never take down a request path.
        let conn = self.lock_conn();
        let _ = conn.execute(
            "INSERT INTO web_audit (ts, username, action, target, detail, ip, ok)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                Utc::now().to_rfc3339(),
                username,
                action,
                target,
                truncate(detail, 500),
                ip,
                ok as i64
            ],
        );
    }

    pub fn list_audit(&self, limit: i64, offset: i64) -> Result<Vec<AuditRow>> {
        self.list_audit_filtered(&AuditFilter::default(), limit, offset)
    }

    pub fn count_audit(&self) -> Result<i64> {
        self.count_audit_filtered(&AuditFilter::default())
    }

    pub fn list_audit_filtered(
        &self,
        filter: &AuditFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<AuditRow>> {
        let conn = self.lock_conn();
        let mut sql =
            "SELECT id, ts, username, action, target, detail, ip, ok FROM web_audit WHERE 1=1"
                .to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if !filter.ts.trim().is_empty() {
            let (clause, value) = ts_filter(filter.ts.trim());
            sql.push_str(clause);
            params_vec.push(Box::new(value));
        }
        if !filter.username.trim().is_empty() {
            sql.push_str(" AND username LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.username.trim())));
        }
        if !filter.action.trim().is_empty() {
            sql.push_str(" AND action LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.action.trim())));
        }
        if !filter.target.trim().is_empty() {
            sql.push_str(" AND target LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.target.trim())));
        }
        if !filter.detail.trim().is_empty() {
            sql.push_str(" AND detail LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.detail.trim())));
        }
        if !filter.ip.trim().is_empty() {
            sql.push_str(" AND ip LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ip.trim())));
        }
        if !filter.result.trim().is_empty() {
            let res = filter.result.trim().to_ascii_lowercase();
            if res == "ok" || res == "success" || res == "true" || res == "1" {
                sql.push_str(" AND ok = 1");
            } else if res == "fail"
                || res == "failed"
                || res == "denied"
                || res == "false"
                || res == "0"
            {
                sql.push_str(" AND ok = 0");
            }
        }

        sql.push_str(" ORDER BY id DESC LIMIT ? OFFSET ?");
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(&params_refs[..], |r| {
                Ok(AuditRow {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    username: r.get(2)?,
                    action: r.get(3)?,
                    target: r.get(4)?,
                    detail: r.get(5)?,
                    ip: r.get(6)?,
                    ok: r.get::<_, i64>(7)? == 1,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn count_audit_filtered(&self, filter: &AuditFilter) -> Result<i64> {
        let conn = self.lock_conn();
        let mut sql = "SELECT COUNT(*) FROM web_audit WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if !filter.ts.trim().is_empty() {
            let (clause, value) = ts_filter(filter.ts.trim());
            sql.push_str(clause);
            params_vec.push(Box::new(value));
        }
        if !filter.username.trim().is_empty() {
            sql.push_str(" AND username LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.username.trim())));
        }
        if !filter.action.trim().is_empty() {
            sql.push_str(" AND action LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.action.trim())));
        }
        if !filter.target.trim().is_empty() {
            sql.push_str(" AND target LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.target.trim())));
        }
        if !filter.detail.trim().is_empty() {
            sql.push_str(" AND detail LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.detail.trim())));
        }
        if !filter.ip.trim().is_empty() {
            sql.push_str(" AND ip LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ip.trim())));
        }
        if !filter.result.trim().is_empty() {
            let res = filter.result.trim().to_ascii_lowercase();
            if res == "ok" || res == "success" || res == "true" || res == "1" {
                sql.push_str(" AND ok = 1");
            } else if res == "fail"
                || res == "failed"
                || res == "denied"
                || res == "false"
                || res == "0"
            {
                sql.push_str(" AND ok = 0");
            }
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let n: i64 = stmt.query_row(&params_refs[..], |r| r.get(0))?;
        Ok(n)
    }

    pub fn list_security_http_logs(
        &self,
        direction: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SecurityHttpRow>> {
        self.list_security_http_logs_filtered(
            direction,
            &SecurityHttpFilter::default(),
            limit,
            offset,
        )
    }

    pub fn count_security_http_logs(&self, direction: Option<&str>) -> Result<i64> {
        self.count_security_http_logs_filtered(direction, &SecurityHttpFilter::default())
    }

    pub fn list_security_http_logs_filtered(
        &self,
        direction: Option<&str>,
        filter: &SecurityHttpFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SecurityHttpRow>> {
        let conn = self.lock_conn();
        let mut sql = "SELECT id, ts, direction, method, target, status, duration_ms, ip, actor, detail FROM security_http_logs WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        let eff_dir = direction.or_else(|| {
            if !filter.direction.trim().is_empty() {
                Some(filter.direction.trim())
            } else {
                None
            }
        });

        if let Some(dir) = eff_dir {
            sql.push_str(" AND direction = ?");
            params_vec.push(Box::new(dir.to_string()));
        }

        if !filter.ts.trim().is_empty() {
            let (clause, value) = ts_filter(filter.ts.trim());
            sql.push_str(clause);
            params_vec.push(Box::new(value));
        }
        if !filter.method.trim().is_empty() {
            sql.push_str(" AND method LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.method.trim())));
        }
        if !filter.target.trim().is_empty() {
            sql.push_str(" AND target LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.target.trim())));
        }
        if !filter.status.trim().is_empty() {
            let st = filter.status.trim();
            if st.ends_with("xx") && st.len() == 3 {
                if let Ok(prefix) = st[0..1].parse::<i64>() {
                    sql.push_str(" AND status >= ? AND status < ?");
                    params_vec.push(Box::new(prefix * 100));
                    params_vec.push(Box::new((prefix + 1) * 100));
                }
            } else if let Ok(code) = st.parse::<i64>() {
                sql.push_str(" AND status = ?");
                params_vec.push(Box::new(code));
            } else {
                sql.push_str(" AND status LIKE ?");
                params_vec.push(Box::new(format!("%{}%", st)));
            }
        }
        if !filter.actor.trim().is_empty() {
            sql.push_str(" AND (actor LIKE ? OR ip LIKE ?)");
            params_vec.push(Box::new(format!("%{}%", filter.actor.trim())));
            params_vec.push(Box::new(format!("%{}%", filter.actor.trim())));
        }
        if !filter.ip.trim().is_empty() {
            sql.push_str(" AND ip LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ip.trim())));
        }
        if !filter.detail.trim().is_empty() {
            sql.push_str(" AND detail LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.detail.trim())));
        }

        sql.push_str(" ORDER BY id DESC LIMIT ? OFFSET ?");
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let map_row = |r: &rusqlite::Row<'_>| {
            Ok(SecurityHttpRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                direction: r.get(2)?,
                method: r.get(3)?,
                target: r.get(4)?,
                status: r.get(5)?,
                duration_ms: r.get(6)?,
                ip: r.get(7)?,
                actor: r.get(8)?,
                detail: r.get(9)?,
            })
        };
        let rows = stmt
            .query_map(&params_refs[..], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn count_security_http_logs_filtered(
        &self,
        direction: Option<&str>,
        filter: &SecurityHttpFilter,
    ) -> Result<i64> {
        let conn = self.lock_conn();
        let mut sql = "SELECT COUNT(*) FROM security_http_logs WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        let eff_dir = direction.or_else(|| {
            if !filter.direction.trim().is_empty() {
                Some(filter.direction.trim())
            } else {
                None
            }
        });

        if let Some(dir) = eff_dir {
            sql.push_str(" AND direction = ?");
            params_vec.push(Box::new(dir.to_string()));
        }

        if !filter.ts.trim().is_empty() {
            let (clause, value) = ts_filter(filter.ts.trim());
            sql.push_str(clause);
            params_vec.push(Box::new(value));
        }
        if !filter.method.trim().is_empty() {
            sql.push_str(" AND method LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.method.trim())));
        }
        if !filter.target.trim().is_empty() {
            sql.push_str(" AND target LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.target.trim())));
        }
        if !filter.status.trim().is_empty() {
            let st = filter.status.trim();
            if st.ends_with("xx") && st.len() == 3 {
                if let Ok(prefix) = st[0..1].parse::<i64>() {
                    sql.push_str(" AND status >= ? AND status < ?");
                    params_vec.push(Box::new(prefix * 100));
                    params_vec.push(Box::new((prefix + 1) * 100));
                }
            } else if let Ok(code) = st.parse::<i64>() {
                sql.push_str(" AND status = ?");
                params_vec.push(Box::new(code));
            } else {
                sql.push_str(" AND status LIKE ?");
                params_vec.push(Box::new(format!("%{}%", st)));
            }
        }
        if !filter.actor.trim().is_empty() {
            sql.push_str(" AND (actor LIKE ? OR ip LIKE ?)");
            params_vec.push(Box::new(format!("%{}%", filter.actor.trim())));
            params_vec.push(Box::new(format!("%{}%", filter.actor.trim())));
        }
        if !filter.ip.trim().is_empty() {
            sql.push_str(" AND ip LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ip.trim())));
        }
        if !filter.detail.trim().is_empty() {
            sql.push_str(" AND detail LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.detail.trim())));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let n: i64 = stmt.query_row(&params_refs[..], |r| r.get(0))?;
        Ok(n)
    }

    /// Deletes security HTTP log rows older than `days` days; returns the number removed.
    pub fn purge_security_http_logs(&self, days: i64) -> Result<usize> {
        let cutoff = (Utc::now() - chrono::Duration::days(days.max(0))).to_rfc3339();
        let conn = self.lock_conn();
        let n = conn.execute(
            "DELETE FROM security_http_logs WHERE ts < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SecurityHttpRow {
    pub id: i64,
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

pub fn normalize_policy(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "required" | "enforce" | "true" | "1" => "required",
        "disabled" | "exempt" | "false" | "0" => "disabled",
        _ => "default",
    }
}

fn split_grants(raw: Option<String>) -> Vec<String> {
    raw.map(|s| {
        s.split('\u{1f}')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn map_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        id: r.get(0)?,
        username: r.get(1)?,
        display_name: r.get(2)?,
        role: r.get(3)?,
        active: r.get::<_, i64>(4)? == 1,
        created_at: r.get(5)?,
        last_login: r.get(6)?,
        totp_enabled: r.get::<_, i64>(7)? == 1,
        two_fa_policy: r
            .get::<_, String>(8)
            .unwrap_or_else(|_| "default".to_string()),
        passkey_count: r.get::<_, i64>(9).unwrap_or(0),
    })
}

fn map_passkey(r: &rusqlite::Row<'_>) -> rusqlite::Result<PasskeyRow> {
    Ok(PasskeyRow {
        id: r.get(0)?,
        user_id: r.get(1)?,
        credential_id: r.get(2)?,
        device_name: r.get(3)?,
        passkey_json: r.get(4)?,
        counter: r.get(5)?,
        created_at: r.get(6)?,
        last_used_at: r.get(7)?,
    })
}

/// `ts` is stored as an RFC 3339 string, so a date-like filter is a prefix and
/// stays index-friendly; only input with no digits at all falls back to the
/// substring match that can never use an index.
fn ts_filter(input: &str) -> (&'static str, String) {
    if input.chars().any(|c| c.is_ascii_digit()) {
        (" AND ts LIKE ? || '%'", input.to_string())
    } else {
        (" AND ts LIKE ?", format!("%{input}%"))
    }
}

/// Drop the oldest chat exchanges until at most `keep` remain.
///
/// The subquery resolves to NULL while the table is smaller than the cap, and
/// `id <= NULL` matches nothing, so this is a no-op until the limit is reached.
fn prune_chat_history(conn: &Connection, keep: i64) {
    let _ = conn.execute(
        "DELETE FROM web_chat_history
          WHERE id <= (SELECT id FROM web_chat_history ORDER BY id DESC LIMIT 1 OFFSET ?1)",
        params![keep],
    );
}

/// Drop each user's oldest console turns until at most `keep` remain.
///
/// The limit is per user, so this walks the distinct accounts and trims each
/// one. A single global DELETE would let a busy account evict everybody else's
/// history, which is the opposite of what the per-user limit means.
fn prune_console_history(conn: &Connection, keep: i64) {
    let users: Vec<String> = match conn.prepare("SELECT DISTINCT username FROM web_console_history")
    {
        Ok(mut stmt) => stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default(),
        Err(_) => return,
    };
    for user in users {
        let _ = conn.execute(
            "DELETE FROM web_console_history
              WHERE username = ?1
                AND id <= (SELECT id FROM web_console_history
                            WHERE username = ?1
                            ORDER BY id DESC LIMIT 1 OFFSET ?2)",
            params![user, keep],
        );
    }
}

/// WHERE clauses shared by the chat-history list and count queries, so a filter
/// can never apply to one and not the other.
fn chat_filter_clauses(
    sql: &mut String,
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
    filter: &ChatLogFilter,
) {
    if !filter.platform.trim().is_empty() {
        // Exact match: the stored value is a fixed label, not free text.
        sql.push_str(" AND platform = ?");
        params_vec.push(Box::new(filter.platform.trim().to_ascii_lowercase()));
    }
    if !filter.username.trim().is_empty() {
        sql.push_str(" AND username LIKE ?");
        params_vec.push(Box::new(format!("%{}%", filter.username.trim())));
    }
    if !filter.ts.trim().is_empty() {
        let (clause, value) = ts_filter(filter.ts.trim());
        sql.push_str(clause);
        params_vec.push(Box::new(value));
    }
    if !filter.text.trim().is_empty() {
        // One box matches either side of the exchange: searching for a repo name
        // should find the question and the answer that mentions it.
        sql.push_str(" AND (command LIKE ? OR reply LIKE ?)");
        let needle = format!("%{}%", filter.text.trim());
        params_vec.push(Box::new(needle.clone()));
        params_vec.push(Box::new(needle));
    }
    if !filter.status.trim().is_empty() {
        sql.push_str(" AND status = ?");
        params_vec.push(Box::new(filter.status.trim().to_ascii_lowercase()));
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::exec::LogLine;

    fn line(level: &str, text: &str) -> LogLine {
        LogLine {
            level: level.to_string(),
            text: text.to_string(),
            ts: "10:00:00".to_string(),
        }
    }

    /// A turn is written twice — once at dispatch, once when it settles — and the
    /// console reads back exactly what the second write stored.
    #[test]
    fn console_history_round_trips_a_turn() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        db.console_history_start(
            "job-1",
            "alice",
            "o/r",
            "/issues 5",
            "2026-10-31T10:00:00+00:00",
        );

        let rows = db.console_history_page("alice", None, 10).expect("page");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_running(), "a dispatched turn starts as running");
        assert!(
            rows[0].lines.is_empty(),
            "the reply is unknown until it settles"
        );

        db.console_history_finish(
            "job-1",
            "done",
            &[line("cmd", "/issues 5"), line("ok", "#1 open")],
            "2026-10-31T10:00:02+00:00",
        );

        let rows = db.console_history_page("alice", None, 10).expect("page");
        assert_eq!(rows[0].status, "done");
        assert_eq!(rows[0].lines.len(), 2);
        assert_eq!(rows[0].lines[1].text, "#1 open");
        assert_eq!(
            rows[0].finished_at.as_deref(),
            Some("2026-10-31T10:00:02+00:00")
        );
    }

    /// Paging uses an id cursor, so a command dispatched between two page loads
    /// cannot shift the rows the reader is walking through.
    #[test]
    fn console_history_pages_backwards_without_overlap() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        for i in 0..3 {
            db.console_history_start(
                &format!("job-{i}"),
                "alice",
                "o/r",
                &format!("/issues {i}"),
                "2026-10-31T10:00:00+00:00",
            );
        }

        let first = db.console_history_page("alice", None, 2).expect("page 1");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].command, "/issues 2", "newest turn comes first");

        let cursor = first.last().map(|r| r.id);
        let second = db.console_history_page("alice", cursor, 2).expect("page 2");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].command, "/issues 0");
        assert!(
            second[0].id < first[1].id,
            "the second page continues behind the first"
        );
    }

    /// History is per account: a turn must not leak into another user's console.
    #[test]
    fn console_history_is_scoped_to_the_owner() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        db.console_history_start(
            "job-a",
            "alice",
            "o/r",
            "/issues",
            "2026-10-31T10:00:00+00:00",
        );
        db.console_history_start("job-b", "bob", "o/r", "/prs", "2026-10-31T10:00:00+00:00");

        let alice = db.console_history_page("alice", None, 10).expect("page");
        assert_eq!(alice.len(), 1);
        assert_eq!(alice[0].command, "/issues");
    }

    // ---------------------------------------------------------- chat history

    fn inbound(
        platform: crate::chat::ChatPlatform,
        user: &str,
        text: &str,
    ) -> crate::chat::InboundChatMessage {
        crate::chat::InboundChatMessage {
            platform,
            sender: crate::chat::InboundSender {
                id: format!("{user}-id"),
                username: user.to_string(),
                display_name: Some(format!("{user} display")),
            },
            raw_content: text.to_string(),
            stream_or_channel: Some("stream-1".to_string()),
            message_id: Some("m-1".to_string()),
        }
    }

    /// The recorded outcome comes from the reply text, because the executor
    /// signals problems with a leading marker instead of an error type.
    #[test]
    fn chat_history_derives_status_from_the_reply() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        let cases = [
            ("✅ Commented on o/r#1", "ok"),
            ("⚠️ Please specify repository", "warn"),
            ("❌ Could not access repository", "error"),
            ("❓ Unknown command `/nope`", "warn"),
        ];
        for (reply, expected) in cases {
            let msg = inbound(crate::chat::ChatPlatform::Zulip, "alice", "/issues");
            db.record_chat_exchange(&msg, reply, 7);
            let rows = db
                .list_chat_logs_filtered(&ChatLogFilter::default(), 1, 0)
                .expect("list");
            assert_eq!(rows[0].status, expected, "reply {reply:?}");
        }
        let total = db
            .count_chat_logs_filtered(&ChatLogFilter::default())
            .expect("count");
        assert_eq!(total, 4);
    }

    /// Everything the console page shows has to survive the round trip: the
    /// platform label, the sender, the channel and the reply text.
    #[test]
    fn chat_history_round_trips_the_exchange() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        let msg = inbound(crate::chat::ChatPlatform::Discord, "carol", "issues o/r");
        db.record_chat_exchange(&msg, "3 open issues in o/r", 42);

        let rows = db
            .list_chat_logs_filtered(&ChatLogFilter::default(), 10, 0)
            .expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].platform, "discord");
        assert_eq!(rows[0].username, "carol");
        assert_eq!(rows[0].display_name, "carol display");
        assert_eq!(rows[0].channel, "stream-1");
        assert_eq!(rows[0].command, "issues o/r");
        assert_eq!(rows[0].reply, "3 open issues in o/r");
        assert_eq!(rows[0].duration_ms, 42);
    }

    /// Filters are shared between the list and the count, so a filtered page
    /// never claims a total that disagrees with what it can show.
    #[test]
    fn chat_history_filters_apply_to_list_and_count_alike() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        db.record_chat_exchange(
            &inbound(crate::chat::ChatPlatform::Zulip, "alice", "issues o/r"),
            "3 open issues in o/r",
            5,
        );
        db.record_chat_exchange(
            &inbound(crate::chat::ChatPlatform::Lark, "bob", "status"),
            "✅ all systems normal",
            6,
        );

        let by_platform = ChatLogFilter {
            platform: "lark".to_string(),
            ..Default::default()
        };
        let rows = db
            .list_chat_logs_filtered(&by_platform, 10, 0)
            .expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].username, "bob");
        assert_eq!(db.count_chat_logs_filtered(&by_platform).expect("count"), 1);

        // Free text searches both sides of the exchange.
        let in_reply = ChatLogFilter {
            text: "open issues".to_string(),
            ..Default::default()
        };
        let rows = db.list_chat_logs_filtered(&in_reply, 10, 0).expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].username, "alice");

        let by_status = ChatLogFilter {
            status: "error".to_string(),
            ..Default::default()
        };
        assert_eq!(db.count_chat_logs_filtered(&by_status).expect("count"), 0);
    }

    /// Retention is off by default: an install that never touches the setting
    /// keeps every exchange.
    #[test]
    fn chat_history_keeps_everything_by_default() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        assert_eq!(db.chat_history_keep(), 0, "0 means unlimited");
        for i in 0..5 {
            let msg = inbound(
                crate::chat::ChatPlatform::Api,
                "alice",
                &format!("/issues {i}"),
            );
            db.record_chat_exchange(&msg, "ok", 1);
        }
        assert_eq!(
            db.count_chat_logs_filtered(&ChatLogFilter::default())
                .expect("count"),
            5
        );
    }

    /// A configured limit applies the moment it is saved, and keeps applying as
    /// new exchanges arrive, so the table cannot creep back over the cap.
    #[test]
    fn chat_history_retention_prunes_to_the_limit() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        for i in 0..5 {
            let msg = inbound(
                crate::chat::ChatPlatform::Api,
                "alice",
                &format!("/issues {i}"),
            );
            db.record_chat_exchange(&msg, "ok", 1);
        }

        db.set_chat_history_keep(2).expect("set keep");
        assert_eq!(db.chat_history_keep(), 2);
        let rows = db
            .list_chat_logs_filtered(&ChatLogFilter::default(), 10, 0)
            .expect("list");
        assert_eq!(rows.len(), 2, "saving the limit trims immediately");
        assert_eq!(rows[0].command, "/issues 4", "the newest exchanges survive");

        db.record_chat_exchange(
            &inbound(crate::chat::ChatPlatform::Api, "alice", "/prs"),
            "ok",
            1,
        );
        let rows = db
            .list_chat_logs_filtered(&ChatLogFilter::default(), 10, 0)
            .expect("list");
        assert_eq!(rows.len(), 2, "a new exchange evicts the oldest");
        assert_eq!(rows[0].command, "/prs");
    }

    /// Console retention is per user: capping the table must not let one busy
    /// account delete somebody else's conversation.
    #[test]
    fn console_history_retention_is_per_user() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        assert_eq!(db.console_history_keep(), 0, "0 means unlimited");
        for i in 0..4 {
            db.console_history_start(
                &format!("a-{i}"),
                "alice",
                "o/r",
                &format!("/issues {i}"),
                "2026-10-31T10:00:00+00:00",
            );
        }
        for i in 0..2 {
            db.console_history_start(
                &format!("b-{i}"),
                "bob",
                "o/r",
                &format!("/prs {i}"),
                "2026-10-31T10:00:00+00:00",
            );
        }
        assert_eq!(
            db.console_history_page("alice", None, 10)
                .expect("page")
                .len(),
            4,
            "nothing is dropped until a limit is set"
        );

        db.set_console_history_keep(2).expect("set keep");
        assert_eq!(db.console_history_keep(), 2);

        let alice = db.console_history_page("alice", None, 10).expect("page");
        assert_eq!(alice.len(), 2, "alice is trimmed to the cap");
        assert_eq!(alice[0].command, "/issues 3", "the newest turns survive");
        assert_eq!(
            db.console_history_page("bob", None, 10)
                .expect("page")
                .len(),
            2,
            "bob was already under the cap"
        );

        // A new turn for alice evicts her oldest, and only hers.
        db.console_history_start(
            "a-new",
            "alice",
            "o/r",
            "/help",
            "2026-10-31T10:01:00+00:00",
        );
        assert_eq!(
            db.console_history_page("alice", None, 10)
                .expect("page")
                .len(),
            2
        );
        assert_eq!(
            db.console_history_page("bob", None, 10)
                .expect("page")
                .len(),
            2
        );
    }
}

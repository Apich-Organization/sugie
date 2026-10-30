//! Persistent storage for the admin console: users, sessions, per-repo ACLs,
//! per-command grants, 2FA (TOTP + WebAuthn passkeys), and the audit trail.
//!
//! Tables are namespaced with `web_` and live in the same SQLite file as the
//! bot's own storage, so operations only ever have to back up one file.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const KEY_GLOBAL_2FA_ENABLED: &str = "global_2fa_enabled";

#[derive(Clone)]
pub struct WebDb {
    conn: Arc<Mutex<Connection>>,
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
        };
        db.init()?;
        Ok(db)
    }

    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;

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
                two_fa_policy TEXT NOT NULL DEFAULT 'default'
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
                user_agent TEXT NOT NULL DEFAULT ''
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

        // Seed default global 2FA policy (enabled by default so first-time logins prompt 2FA setup)
        let _ = conn.execute(
            "INSERT OR IGNORE INTO web_settings (key, value, updated_at) VALUES (?1, 'true', ?2)",
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
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM web_settings WHERE key = ?1",
            params![KEY_GLOBAL_2FA_ENABLED],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(true)
    }

    pub fn set_global_2fa_enabled(&self, enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO web_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![
                KEY_GLOBAL_2FA_ENABLED,
                if enabled { "true" } else { "false" },
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_security_log_settings(&self) -> crate::security_log::SecurityLogSettings {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO web_users (username, password_hash, role, display_name, active, created_at, two_fa_policy)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
            params![username, password_hash, role, display_name, Utc::now().to_rfc3339(), policy],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn find_user(&self, username: &str) -> Result<Option<UserRow>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("{USER_SELECT_SQL} WHERE u.username = ?1 COLLATE NOCASE");
        let row = conn
            .query_row(&sql, params![username], map_user)
            .optional()?;
        Ok(row)
    }

    pub fn get_user(&self, id: i64) -> Result<Option<UserRow>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("{USER_SELECT_SQL} WHERE u.id = ?1");
        let row = conn.query_row(&sql, params![id], map_user).optional()?;
        Ok(row)
    }

    pub fn password_hash(&self, user_id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET role = ?2, display_name = ?3, active = ?4 WHERE id = ?1",
            params![id, role, display_name, active as i64],
        )?;
        Ok(())
    }

    pub fn set_user_two_fa_policy(&self, id: i64, policy: &str) -> Result<()> {
        let p = normalize_policy(policy);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET two_fa_policy = ?2 WHERE id = ?1",
            params![id, p],
        )?;
        Ok(())
    }

    pub fn set_password(&self, id: i64, hash: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET password_hash = ?2 WHERE id = ?1",
            params![id, hash],
        )?;
        Ok(())
    }

    pub fn delete_user(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM web_repo_perms WHERE user_id = ?1", params![id])?;
        conn.execute(
            "DELETE FROM web_command_perms WHERE user_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM web_sessions WHERE user_id = ?1", params![id])?;
        conn.execute(
            "DELETE FROM web_pending_2fa WHERE user_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM web_passkeys WHERE user_id = ?1", params![id])?;
        conn.execute("DELETE FROM web_users WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn touch_login(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET last_login = ?2 WHERE id = ?1",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // --------------------------------------------------------- 2FA: TOTP

    pub fn get_totp_secret(&self, user_id: i64) -> Result<Option<(String, bool)>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET totp_secret = ?2, totp_enabled = 1 WHERE id = ?1",
            params![user_id, secret],
        )?;
        Ok(())
    }

    pub fn disable_totp(&self, user_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET totp_secret = '', totp_enabled = 0 WHERE id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    pub fn reset_user_2fa(&self, user_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET totp_secret = '', totp_enabled = 0 WHERE id = ?1",
            params![user_id],
        )?;
        conn.execute(
            "DELETE FROM web_passkeys WHERE user_id = ?1",
            params![user_id],
        )?;
        Ok(())
    }

    // --------------------------------------------------------- 2FA: Passkeys

    pub fn list_user_passkeys(&self, user_id: i64) -> Result<Vec<PasskeyRow>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM web_sessions WHERE token_hash = ?1",
            params![token_hash],
        )?;
        Ok(())
    }

    pub fn purge_sessions(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let res = self.get_pending_2fa(token_hash)?;
        if res.is_some() {
            let conn = self.conn.lock().unwrap();
            let _ = conn.execute(
                "DELETE FROM web_pending_2fa WHERE token_hash = ?1",
                params![token_hash],
            );
        }
        Ok(res)
    }

    // --------------------------------------------------------------- perms

    pub fn set_repo_perms(&self, user_id: i64, repos: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT repo FROM web_repo_perms WHERE user_id = ?1 ORDER BY repo")?;
        let rows = stmt
            .query_map(params![user_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn set_command_perms(&self, user_id: i64, commands: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT command FROM web_command_perms WHERE user_id = ?1 ORDER BY command")?;
        let rows = stmt
            .query_map(params![user_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
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
        if action != "security.log_settings" && !crate::security_log::current_settings().admin_audit
        {
            return;
        }
        // Audit must never take down a request path.
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut sql =
            "SELECT id, ts, username, action, target, detail, ip, ok FROM web_audit WHERE 1=1"
                .to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if !filter.ts.trim().is_empty() {
            sql.push_str(" AND ts LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ts.trim())));
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
        let conn = self.conn.lock().unwrap();
        let mut sql = "SELECT COUNT(*) FROM web_audit WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if !filter.ts.trim().is_empty() {
            sql.push_str(" AND ts LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ts.trim())));
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
        let conn = self.conn.lock().unwrap();
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
            sql.push_str(" AND ts LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ts.trim())));
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
        let conn = self.conn.lock().unwrap();
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
            sql.push_str(" AND ts LIKE ?");
            params_vec.push(Box::new(format!("%{}%", filter.ts.trim())));
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
        let conn = self.conn.lock().unwrap();
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

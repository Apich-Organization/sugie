//! Persistent storage for the admin console: users, sessions, per-repo ACLs,
//! per-command grants and the audit trail.
//!
//! Tables are namespaced with `web_` and live in the same SQLite file as the
//! bot's own storage, so operations only ever have to back up one file.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

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
                last_login TEXT
            );

            CREATE TABLE IF NOT EXISTS web_sessions (
                token_hash TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                ip TEXT NOT NULL DEFAULT '',
                user_agent TEXT NOT NULL DEFAULT ''
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
            "#,
        )?;

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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO web_users (username, password_hash, role, display_name, active, created_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            params![username, password_hash, role, display_name, Utc::now().to_rfc3339()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn find_user(&self, username: &str) -> Result<Option<UserRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, username, display_name, role, active, created_at, last_login
                 FROM web_users WHERE username = ?1 COLLATE NOCASE",
                params![username],
                map_user,
            )
            .optional()?;
        Ok(row)
    }

    pub fn get_user(&self, id: i64) -> Result<Option<UserRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, username, display_name, role, active, created_at, last_login
                 FROM web_users WHERE id = ?1",
                params![id],
                map_user,
            )
            .optional()?;
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
        let mut stmt = conn.prepare(
            "SELECT id, username, display_name, role, active, created_at, last_login
             FROM web_users ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], map_user)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn update_user_profile(&self, id: i64, role: &str, display_name: &str, active: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_users SET role = ?2, display_name = ?3, active = ?4 WHERE id = ?1",
            params![id, role, display_name, active as i64],
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
        conn.execute("DELETE FROM web_command_perms WHERE user_id = ?1", params![id])?;
        conn.execute("DELETE FROM web_sessions WHERE user_id = ?1", params![id])?;
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

        let row = conn
            .query_row(
                "SELECT id, username, display_name, role, active, created_at, last_login
                 FROM web_users WHERE id = ?1",
                params![user_id],
                map_user,
            )
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
        conn.execute(
            "DELETE FROM web_sessions WHERE expires_at < ?1",
            params![Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // --------------------------------------------------------------- perms

    pub fn set_repo_perms(&self, user_id: i64, repos: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM web_repo_perms WHERE user_id = ?1", params![user_id])?;
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, ts, username, action, target, detail, ip, ok
             FROM web_audit ORDER BY id DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt
            .query_map(params![limit, offset], |r| {
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

    pub fn count_audit(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM web_audit", [], |r| r.get(0))?;
        Ok(n)
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
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct ContributorStats {
    pub username: String,
    pub repo: String,
    pub merged_prs: u32,
    pub opened_issues: u32,
    pub reviews_done: u32,
    pub last_milestone_notified: u32,
}

#[derive(Debug, Clone)]
pub struct ReviewerWorkload {
    pub username: String,
    pub repo: String,
    pub active_reviews: u32,
    pub last_assigned_at: Option<DateTime<Utc>>,
}

impl Storage {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.init_tables()?;
        Ok(storage)
    }

    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.init_tables()?;
        Ok(storage)
    }

    fn init_tables(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS repositories (
                id INTEGER PRIMARY KEY,
                full_name TEXT UNIQUE NOT NULL,
                installation_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS contributor_stats (
                username TEXT NOT NULL,
                repo TEXT NOT NULL,
                merged_prs INTEGER DEFAULT 0,
                opened_issues INTEGER DEFAULT 0,
                reviews_done INTEGER DEFAULT 0,
                last_milestone_notified INTEGER DEFAULT 0,
                PRIMARY KEY (username, repo)
            );

            CREATE TABLE IF NOT EXISTS reviewer_workloads (
                username TEXT NOT NULL,
                repo TEXT NOT NULL,
                active_reviews INTEGER DEFAULT 0,
                last_assigned_at TEXT,
                PRIMARY KEY (username, repo)
            );

            CREATE TABLE IF NOT EXISTS stale_tracking (
                repo TEXT NOT NULL,
                issue_number INTEGER NOT NULL,
                warned_at TEXT,
                PRIMARY KEY (repo, issue_number)
            );
            "#,
        )?;
        Ok(())
    }

    // --- Repositories ---

    pub fn upsert_repository(&self, id: u64, full_name: &str, installation_id: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            r#"
            INSERT INTO repositories (id, full_name, installation_id, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?4)
            ON CONFLICT(full_name) DO UPDATE SET
                id = ?1,
                installation_id = ?3,
                updated_at = ?4
            "#,
            params![id as i64, full_name, installation_id as i64, now],
        )?;
        Ok(())
    }

    pub fn remove_repository(&self, full_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM repositories WHERE full_name = ?1",
            params![full_name],
        )?;
        Ok(())
    }

    pub fn list_repositories(&self) -> Result<Vec<(u64, String, u64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, full_name, installation_id FROM repositories")?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let full_name: String = row.get(1)?;
            let inst_id: i64 = row.get(2)?;
            Ok((id as u64, full_name, inst_id as u64))
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn get_installation_id_for_repo(&self, full_name: &str) -> Result<Option<u64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT installation_id FROM repositories WHERE full_name = ?1")?;
        let mut rows = stmt.query(params![full_name])?;
        if let Some(row) = rows.next()? {
            let inst_id: i64 = row.get(0)?;
            Ok(Some(inst_id as u64))
        } else {
            Ok(None)
        }
    }

    // --- Contributor Stats ---

    pub fn record_merged_pr(&self, username: &str, repo: &str) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO contributor_stats (username, repo, merged_prs, opened_issues, reviews_done, last_milestone_notified)
            VALUES (?1, ?2, 1, 0, 0, 0)
            ON CONFLICT(username, repo) DO UPDATE SET
                merged_prs = merged_prs + 1
            "#,
            params![username, repo],
        )?;

        let mut stmt = conn.prepare(
            "SELECT merged_prs FROM contributor_stats WHERE username = ?1 AND repo = ?2",
        )?;
        let count: u32 = stmt.query_row(params![username, repo], |r| r.get(0))?;
        Ok(count)
    }

    pub fn get_contributor_stats(
        &self,
        username: &str,
        repo: &str,
    ) -> Result<Option<ContributorStats>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT username, repo, merged_prs, opened_issues, reviews_done, last_milestone_notified FROM contributor_stats WHERE username = ?1 AND repo = ?2"
        )?;
        let mut rows = stmt.query(params![username, repo])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ContributorStats {
                username: row.get(0)?,
                repo: row.get(1)?,
                merged_prs: row.get(2)?,
                opened_issues: row.get(3)?,
                reviews_done: row.get(4)?,
                last_milestone_notified: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn update_last_milestone_notified(
        &self,
        username: &str,
        repo: &str,
        milestone: u32,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE contributor_stats SET last_milestone_notified = ?3 WHERE username = ?1 AND repo = ?2",
            params![username, repo, milestone],
        )?;
        Ok(())
    }

    // --- Reviewer Workloads ---

    pub fn get_reviewer_workload(&self, username: &str, repo: &str) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT active_reviews FROM reviewer_workloads WHERE username = ?1 AND repo = ?2",
        )?;
        let mut rows = stmt.query(params![username, repo])?;
        if let Some(row) = rows.next()? {
            let active: u32 = row.get(0)?;
            Ok(active)
        } else {
            Ok(0)
        }
    }

    pub fn increment_reviewer_workload(&self, username: &str, repo: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            r#"
            INSERT INTO reviewer_workloads (username, repo, active_reviews, last_assigned_at)
            VALUES (?1, ?2, 1, ?3)
            ON CONFLICT(username, repo) DO UPDATE SET
                active_reviews = active_reviews + 1,
                last_assigned_at = ?3
            "#,
            params![username, repo, now],
        )?;
        Ok(())
    }

    pub fn decrement_reviewer_workload(&self, username: &str, repo: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            UPDATE reviewer_workloads
            SET active_reviews = CASE WHEN active_reviews > 0 THEN active_reviews - 1 ELSE 0 END
            WHERE username = ?1 AND repo = ?2
            "#,
            params![username, repo],
        )?;
        Ok(())
    }

    // --- Stale Tracking ---

    pub fn record_stale_warned(&self, repo: &str, issue_number: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            r#"
            INSERT INTO stale_tracking (repo, issue_number, warned_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(repo, issue_number) DO UPDATE SET warned_at = ?3
            "#,
            params![repo, issue_number as i64, now],
        )?;
        Ok(())
    }

    pub fn get_stale_warned(&self, repo: &str, issue_number: u64) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT warned_at FROM stale_tracking WHERE repo = ?1 AND issue_number = ?2",
        )?;
        let mut rows = stmt.query(params![repo, issue_number as i64])?;
        if let Some(row) = rows.next()? {
            let warned_str: Option<String> = row.get(0)?;
            if let Some(ws) = warned_str {
                let dt = DateTime::parse_from_rfc3339(&ws)
                    .map(|d| d.with_timezone(&Utc))
                    .ok();
                return Ok(dt);
            }
        }
        Ok(None)
    }

    pub fn clear_stale_warned(&self, repo: &str, issue_number: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM stale_tracking WHERE repo = ?1 AND issue_number = ?2",
            params![repo, issue_number as i64],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_basic() {
        let storage = Storage::new_in_memory().unwrap();

        // Repositories
        storage
            .upsert_repository(101, "rust-lang/rust", 999)
            .unwrap();
        let repos = storage.list_repositories().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].1, "rust-lang/rust");
        assert_eq!(
            storage
                .get_installation_id_for_repo("rust-lang/rust")
                .unwrap(),
            Some(999)
        );

        // Contributor stats
        let count1 = storage
            .record_merged_pr("developer1", "rust-lang/rust")
            .unwrap();
        assert_eq!(count1, 1);
        let count2 = storage
            .record_merged_pr("developer1", "rust-lang/rust")
            .unwrap();
        assert_eq!(count2, 2);

        let stats = storage
            .get_contributor_stats("developer1", "rust-lang/rust")
            .unwrap()
            .unwrap();
        assert_eq!(stats.merged_prs, 2);

        // Reviewer workload
        storage
            .increment_reviewer_workload("reviewer1", "rust-lang/rust")
            .unwrap();
        assert_eq!(
            storage
                .get_reviewer_workload("reviewer1", "rust-lang/rust")
                .unwrap(),
            1
        );
        storage
            .decrement_reviewer_workload("reviewer1", "rust-lang/rust")
            .unwrap();
        assert_eq!(
            storage
                .get_reviewer_workload("reviewer1", "rust-lang/rust")
                .unwrap(),
            0
        );
    }
}

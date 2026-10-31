//! Read / validate / back up / atomically write the two config artefacts the
//! bot consumes at startup: `sugie.toml` and `.env`.

use crate::config::SugieConfig;
use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ConfigFiles {
    pub toml_path: PathBuf,
    pub env_path: PathBuf,
    pub backup_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
    pub secret: bool,
    pub commented: bool,
}

#[derive(Clone, Debug)]
pub struct BackupInfo {
    pub name: String,
    pub size: u64,
    pub modified: String,
}

pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    [
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "PRIVATE_KEY",
        "PEM",
        "SIGNATURE",
        // A webhook URL embeds its own bearer credential; DATABASE_URL often
        // carries `user:password@`. Neither matched the original list, so both
        // were rendered in clear text.
        "WEBHOOK",
        "DATABASE_URL",
        "CONNECTION_STRING",
        "DSN",
        "CREDENTIAL",
        "AUTHORIZATION",
    ]
    .iter()
    .any(|needle| k.contains(needle))
}

pub fn mask(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    // Never reveal any real character: a 4-character prefix is enough to
    // confirm a guessed secret. The value length is not leaked either.
    "•".repeat(10)
}

impl ConfigFiles {
    pub fn new(toml_path: impl Into<PathBuf>, env_path: impl Into<PathBuf>) -> Self {
        let toml_path = toml_path.into();
        let backup_dir = toml_path
            .parent()
            .map(|p| p.join("data/config-backups"))
            .unwrap_or_else(|| PathBuf::from("data/config-backups"));
        Self {
            toml_path,
            env_path: env_path.into(),
            backup_dir,
        }
    }

    // ------------------------------------------------------------ sugie.toml

    pub fn read_toml(&self) -> Result<String> {
        std::fs::read_to_string(&self.toml_path)
            .with_context(|| format!("read {}", self.toml_path.display()))
    }

    /// Parse into the real config struct — this is the gate that stops a bad
    /// edit from ever reaching disk.
    pub fn validate_toml(&self, content: &str) -> std::result::Result<SugieConfig, String> {
        let mut cfg: SugieConfig =
            toml::from_str(content).map_err(|e| format!("TOML parse error: {e}"))?;

        if cfg.port == 0 {
            return Err("port cannot be 0".to_string());
        }
        if cfg.host.trim().is_empty() {
            return Err("host cannot be empty".to_string());
        }
        if cfg.db_path.trim().is_empty() {
            return Err("db_path cannot be empty".to_string());
        }
        for repo in cfg
            .notifications
            .allowed_repositories
            .clone()
            .unwrap_or_default()
        {
            if !repo.contains('/') {
                return Err(format!(
                    "`{repo}` in allowed_repositories is not in owner/repo format"
                ));
            }
        }
        if let Some(ref z) = cfg.notifications.zulip {
            if !z.site_url.starts_with("http") {
                return Err("zulip.site_url must start with http:// or https://".to_string());
            }
        }
        // Keep secrets sourced from .env rather than the TOML file.
        cfg.github.webhook_secret.clear();
        Ok(cfg)
    }

    pub fn save_toml(&self, content: &str) -> Result<PathBuf> {
        self.validate_toml(content).map_err(|e| anyhow!(e))?;
        let backup = self.backup_file(&self.toml_path)?;
        atomic_write(&self.toml_path, content)?;
        Ok(backup)
    }

    // ------------------------------------------------------------------ .env

    pub fn read_env_entries(&self) -> Vec<EnvEntry> {
        let Ok(raw) = std::fs::read_to_string(&self.env_path) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            // Skip blank lines and the `#!` shebang only: a commented-out variable
            // is still shown in the UI (flagged as commented).
            if trimmed.is_empty() || trimmed.starts_with("#!") {
                continue;
            }
            let commented = trimmed.starts_with('#');
            let body = if commented {
                trimmed.trim_start_matches('#').trim()
            } else {
                trimmed
            };
            let Some((key, value)) = body.split_once('=') else {
                continue;
            };
            let key = key.trim().to_string();
            if key.is_empty() {
                continue;
            }
            out.push(EnvEntry {
                secret: is_secret_key(&key),
                key,
                value: unquote(value.trim()),
                commented,
            });
        }
        out
    }

    /// Merge submitted values back into the file, preserving comments, ordering
    /// and any keys the form did not touch. An empty value means "keep current"
    /// for secret keys (the form never renders them); for any other key it means
    /// "set to empty".
    pub fn save_env(&self, updates: &[(String, String)]) -> Result<Option<PathBuf>> {
        let raw = std::fs::read_to_string(&self.env_path).unwrap_or_default();
        let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
        let mut pending: Vec<(String, String)> = Vec::new();
        for (k, v) in updates {
            let key = k.trim().to_string();
            if key.is_empty() {
                continue;
            }
            // A form field name becomes a raw line prefix; without this check a
            // crafted key could append arbitrary `SECRET=...` definitions.
            if !is_valid_env_key(&key) {
                return Err(anyhow!("Invalid environment variable name `{key}`"));
            }
            if v.is_empty() && is_secret_key(&key) {
                continue;
            }
            pending.push((key, v.clone()));
        }

        let mut rewritten: HashSet<String> = HashSet::new();
        let mut commented: HashSet<String> = HashSet::new();
        for line in lines.iter_mut() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                let Some((key, value)) = trimmed.trim_start_matches('#').trim().split_once('=')
                else {
                    continue;
                };
                // Owned copies so the line can be rewritten below.
                let key = key.trim().to_string();
                let value = unquote(value.trim());
                // An untouched comment stays documentation. A comment whose value
                // the operator changed is promoted to a live definition in place —
                // otherwise the UI reports "Saved .env" while dropping a rotated
                // secret on the floor.
                if let Some((_, new_value)) =
                    pending.iter().find(|(k, _)| k.as_str() == key.as_str())
                {
                    if *new_value != value {
                        *line = render_env(&key, new_value);
                        rewritten.insert(key.clone());
                    }
                }
                commented.insert(key);
                continue;
            }
            let Some((key, _)) = trimmed.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if let Some((_, value)) = pending.iter().find(|(k, _)| k == key) {
                // dotenv honours the *last* definition, so every occurrence has to
                // be rewritten or an edit to a duplicated key silently does nothing.
                let rendered = render_env(key, value);
                rewritten.insert(key.to_string());
                *line = rendered;
            }
        }

        for (key, value) in pending {
            if !rewritten.contains(&key) && !commented.contains(&key) {
                lines.push(render_env(&key, &value));
            }
        }

        let mut content = lines.join("\n");
        if !content.ends_with('\n') {
            content.push('\n');
        }

        if raw == content {
            return Ok(None);
        }

        let backup = self.backup_file(&self.env_path)?;
        atomic_write(&self.env_path, &content)?;
        Ok(Some(backup))
    }

    // -------------------------------------------------------------- backups

    pub fn backup_file(&self, file: &Path) -> Result<PathBuf> {
        if !file.exists() {
            return Ok(PathBuf::new());
        }
        std::fs::create_dir_all(&self.backup_dir)?;
        restrict_dir(&self.backup_dir);
        // Millisecond precision plus a de-duplication suffix: two saves inside
        // the same second must not silently overwrite each other's backup.
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f");
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "config".to_string());
        let mut dest = self.backup_dir.join(format!("{name}.{stamp}.bak"));
        let mut seq = 1;
        while dest.exists() {
            dest = self.backup_dir.join(format!("{name}.{stamp}-{seq}.bak"));
            seq += 1;
            if seq > 99 {
                break;
            }
        }
        std::fs::copy(file, &dest)?;
        // Backups contain plaintext secrets; keep them owner-only.
        restrict_file(&dest);
        Ok(dest)
    }

    pub fn list_backups(&self) -> Vec<BackupInfo> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.backup_dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_default();
            out.push(BackupInfo {
                name: entry.file_name().to_string_lossy().to_string(),
                size: meta.len(),
                modified,
            });
        }
        out.sort_by(|a, b| b.name.cmp(&a.name));
        out.truncate(20);
        out
    }

    pub fn restore_backup(&self, name: &str) -> Result<()> {
        // Reject traversal: backups are flat file names only.
        if name.contains('/') || name.contains("..") {
            return Err(anyhow!("Invalid backup filename"));
        }
        let src = self.backup_dir.join(name);
        if !src.exists() {
            return Err(anyhow!("Backup does not exist: {name}"));
        }
        let target = if name.starts_with(".env") {
            &self.env_path
        } else {
            &self.toml_path
        };
        let content = std::fs::read_to_string(&src)?;
        if target == &self.toml_path {
            self.validate_toml(&content).map_err(|e| anyhow!(e))?;
        }
        self.backup_file(target)?;
        atomic_write(target, &content)?;
        Ok(())
    }
}

/// A single value change applied to `sugie.toml` while preserving comments and
/// layout (via `toml_edit`).
pub enum Patch {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
    Remove,
}

pub fn patch_toml(text: &str, patches: &[(String, Patch)]) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| format!("TOML parse error: {e}"))?;

    for (path, patch) in patches {
        let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
        if segs.is_empty() {
            continue;
        }
        let item = ensure_path(&mut doc, &segs);
        *item = match patch {
            Patch::Remove => toml_edit::Item::None,
            Patch::Str(s) => toml_edit::Item::Value(toml_edit::Value::from(s.clone())),
            Patch::Int(i) => toml_edit::Item::Value(toml_edit::Value::from(*i)),
            Patch::Bool(b) => toml_edit::Item::Value(toml_edit::Value::from(*b)),
            Patch::List(items) => {
                let mut arr = toml_edit::Array::new();
                for it in items {
                    arr.push(it.clone());
                }
                toml_edit::Item::Value(toml_edit::Value::Array(arr))
            }
        };
    }

    Ok(doc.to_string())
}

fn ensure_path<'a>(doc: &'a mut toml_edit::DocumentMut, segs: &[&str]) -> &'a mut toml_edit::Item {
    let mut cur: &mut toml_edit::Item = doc.as_item_mut();
    for seg in segs {
        if !cur.is_table() {
            *cur = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let table = cur.as_table_mut().expect("table just ensured");
        cur = table
            .entry(seg)
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    }
    cur
}

/// Write via a unique temp file in the same directory, fsync, then rename. A
/// unique name means two admins saving at once cannot interleave their bytes into
/// one temp file, and the fsync makes the rename durable.
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    let tmp = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        file_name,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));

    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }

    // fsync the directory so the rename itself survives a crash.
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    restrict_file(path);
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) {}

#[cfg(unix)]
fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) {}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// `^[A-Za-z_][A-Za-z0-9_]*$` — the only shape a dotenv key may take, and the
/// only way a submitted field name is allowed to reach the file verbatim.
fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An empty value is written as a bare `KEY=`, not `KEY=""`.
fn render_env(key: &str, value: &str) -> String {
    if value.is_empty() {
        format!("{key}=")
    } else {
        format!("{key}={}", quote_if_needed(value))
    }
}

fn quote_if_needed(v: &str) -> String {
    // A newline would let a single form field append arbitrary lines to `.env`
    // (for example a second definition of a secret that shadows the real one).
    let v = v.replace(['\n', '\r'], " ");
    if v.contains(' ')
        || v.contains('#')
        || v.contains('"')
        || v.contains('\'')
        || v.contains('\\')
        || v.is_empty()
    {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        v
    }
}

/// Compact, human-readable summary used by `/config` and the dashboard.
pub struct ConfigSummary {
    pub host: String,
    pub port: u16,
    pub db_path: String,
    pub channels: Vec<String>,
    pub allowed_repositories: Vec<String>,
    pub super_users: Vec<String>,
    pub restricted_commands: Vec<String>,
    pub stale_enabled: bool,
    pub reviewers_enabled: bool,
    pub labels_enabled: bool,
}

impl ConfigSummary {
    pub fn from_config(cfg: &SugieConfig) -> Self {
        let mut channels = Vec::new();
        if cfg.notifications.discord.is_some() {
            channels.push("Discord".to_string());
        }
        if cfg.notifications.zulip.is_some() {
            channels.push("Zulip".to_string());
        }
        if cfg.notifications.lark.is_some() {
            channels.push("Lark".to_string());
        }
        Self {
            host: cfg.host.clone(),
            port: cfg.port,
            db_path: cfg.db_path.clone(),
            channels,
            allowed_repositories: cfg
                .notifications
                .allowed_repositories
                .clone()
                .unwrap_or_default(),
            super_users: cfg.default_rules.commands.super_users.clone(),
            restricted_commands: cfg.default_rules.commands.restricted_commands.clone(),
            stale_enabled: cfg.default_rules.stale.enabled,
            reviewers_enabled: cfg.default_rules.reviewers.enabled,
            labels_enabled: cfg.default_rules.labels.enabled,
        }
    }
}

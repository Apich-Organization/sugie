//! Read / validate / back up / atomically write the two config artefacts the
//! bot consumes at startup: `sugie.toml` and `.env`.

use crate::config::SugieConfig;
use anyhow::{anyhow, Context, Result};
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
    ["SECRET", "TOKEN", "PASSWORD", "API_KEY", "PRIVATE_KEY", "PEM", "SIGNATURE"]
        .iter()
        .any(|needle| k.contains(needle))
}

pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if value.is_empty() {
        return String::new();
    }
    if chars.len() <= 8 {
        return "•".repeat(chars.len().max(4));
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}••••{tail}")
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
                return Err(format!("`{repo}` in allowed_repositories is not in owner/repo format"));
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
            if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.starts_with("#!")) {
                continue;
            }
            let commented = trimmed.starts_with('#');
            let body = trimmed.trim_start_matches('#').trim();
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
    /// and any keys the form did not touch. Empty values mean "keep current".
    pub fn save_env(&self, updates: &[(String, String)]) -> Result<Option<PathBuf>> {
        let raw = std::fs::read_to_string(&self.env_path).unwrap_or_default();
        let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
        let mut pending: Vec<(String, String)> = updates
            .iter()
            .filter(|(k, v)| !k.trim().is_empty() && !v.is_empty())
            .map(|(k, v)| (k.trim().to_string(), v.clone()))
            .collect();

        for line in lines.iter_mut() {
            let trimmed = line.trim();
            let is_comment = trimmed.starts_with('#');
            let body = if is_comment {
                trimmed.trim_start_matches('#').trim()
            } else {
                trimmed
            };
            let Some((key, _)) = body.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if let Some(idx) = pending.iter().position(|(k, _)| k == key) {
                let value = pending.remove(idx).1;
                let rendered = format!("{}={}", key, quote_if_needed(&value));
                *line = if is_comment {
                    format!("#{rendered}")
                } else {
                    rendered
                };
            }
        }

        for (key, value) in pending {
            lines.push(format!("{}={}", key, quote_if_needed(&value)));
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
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "config".to_string());
        let dest = self.backup_dir.join(format!("{name}.{stamp}.bak"));
        std::fs::copy(file, &dest)?;
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
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("TOML parse error: {e}"))?;

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

fn ensure_path<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    segs: &[&str],
) -> &'a mut toml_edit::Item {
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

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

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

fn quote_if_needed(v: &str) -> String {
    if v.contains(' ') || v.contains('#') || v.contains('"') {
        format!("\"{}\"", v.replace('"', "\\\""))
    } else {
        v.to_string()
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

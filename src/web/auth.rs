//! Password hashing, opaque session cookies and principal resolution.

use crate::web::db::WebDb;
use crate::web::perms::{CurrentUser, Role};
use anyhow::{anyhow, Result};
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

pub const COOKIE_NAME: &str = "sugie_admin";
const SESSION_TTL_HOURS: i64 = 12;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow!("hash password: {e}"))
}

pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// 32 random bytes, hex encoded — used as the opaque session token.
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

pub fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

pub fn token_hash(token: &str) -> String {
    sha256_hex(&format!("sugie-admin-session:{token}"))
}

pub fn cookie_value(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(k, _)| k.trim() == COOKIE_NAME)
        .map(|(_, v)| v.trim().to_string())
}

pub fn session_cookie(token: &str) -> String {
    format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_TTL_HOURS * 3600
    )
}

pub fn clear_cookie() -> String {
    format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

pub fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string())
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Resolve the caller from the session cookie. Returns `None` when the caller is
/// anonymous, expired, or disabled.
pub fn current_user(db: &WebDb, headers: &HeaderMap) -> Option<CurrentUser> {
    let token = cookie_value(headers)?;
    let row = db.session_user(&token_hash(&token)).ok().flatten()?;
    let repos = db.repo_perms(row.id).unwrap_or_default();
    let commands = db.command_perms(row.id).unwrap_or_default();
    Some(CurrentUser::from_row(row, repos, commands))
}

/// Verify credentials and mint a session. Returns the raw token on success.
pub fn login(
    db: &WebDb,
    username: &str,
    password: &str,
    ip: &str,
    user_agent: &str,
) -> Result<Option<String>> {
    let Some(row) = db.find_user(username)? else {
        return Ok(None);
    };
    if !row.active {
        return Ok(None);
    }
    let Some(hash) = db.password_hash(row.id)? else {
        return Ok(None);
    };
    if !verify_password(password, &hash) {
        return Ok(None);
    }

    let token = random_token();
    db.create_session(
        &token_hash(&token),
        row.id,
        SESSION_TTL_HOURS,
        ip,
        user_agent,
    )?;
    db.touch_login(row.id)?;
    let _ = db.purge_sessions();
    Ok(Some(token))
}

/// Create the bootstrap `owner` account the first time the console starts.
/// Returns the generated password when one had to be invented.
pub fn bootstrap_admin(db: &WebDb) -> Result<Option<String>> {
    if db.count_users()? > 0 {
        return Ok(None);
    }

    let from_env = std::env::var("SUGIE_ADMIN_PASSWORD").ok().filter(|p| p.len() >= 8);
    let generated = from_env.is_none();
    let password = from_env.unwrap_or_else(random_token);

    db.create_user("admin", &hash_password(&password)?, Role::Owner.as_str(), "System Administrator")?;

    if generated {
        // Keep the secret out of shell history / process listings.
        let path = std::path::Path::new("data/admin-initial-password.txt");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, format!("admin / {password}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }

    Ok(generated.then_some(password))
}

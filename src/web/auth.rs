//! Password hashing, opaque session cookies and principal resolution.

use crate::web::db::{UserRow, WebDb};
use crate::web::perms::{CurrentUser, Role};
use anyhow::{anyhow, Result};
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

pub const COOKIE_NAME: &str = "sugie_admin";
/// Preferred name when the request arrives over TLS: the `__Host-` prefix makes
/// the browser refuse any cookie that was set with a `Domain` attribute, which
/// blocks session fixation from a compromised sibling subdomain.
pub const SECURE_COOKIE_NAME: &str = "__Host-sugie_admin";
const SESSION_TTL_HOURS: i64 = 12;

/// Consecutive failures before an account is temporarily locked.
const MAX_LOGIN_ATTEMPTS: i64 = 5;
const LOCKOUT_SECS: i64 = 15 * 60;
/// Wrong OTP codes tolerated for a single pending 2FA challenge.
pub const MAX_OTP_ATTEMPTS: i64 = 5;

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
        .find(|(k, _)| {
            let k = k.trim();
            k == COOKIE_NAME || k == SECURE_COOKIE_NAME
        })
        .map(|(_, v)| v.trim().to_string())
}

/// Whether the console is being served over TLS. Honours an explicit override
/// (`SUGIE_ADMIN_SECURE_COOKIE=1|0`) so deployments behind a proxy that does not
/// forward `X-Forwarded-Proto` can still opt in.
pub fn is_secure_request(headers: &HeaderMap) -> bool {
    if let Ok(raw) = std::env::var("SUGIE_ADMIN_SECURE_COOKIE") {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => return true,
            "0" | "false" | "no" | "off" => return false,
            _ => {}
        }
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    let max_age = SESSION_TTL_HOURS * 3600;
    if secure {
        format!(
            "{SECURE_COOKIE_NAME}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}"
        )
    } else {
        format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}")
    }
}

/// Clear both the secure and the plain cookie so a scheme change cannot leave a
/// stale session behind.
pub fn clear_cookie(secure: bool) -> String {
    let first = if secure {
        format!("{SECURE_COOKIE_NAME}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0")
    } else {
        format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
    };
    format!("{first}, {COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
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
///
/// This runs on every admin request, so session + user + grants are fetched in a
/// single statement (`WebDb::session_principal`).
pub fn current_user(db: &WebDb, headers: &HeaderMap) -> Option<CurrentUser> {
    let token = cookie_value(headers)?;
    let (row, repos, commands) = db.session_principal(&token_hash(&token)).ok().flatten()?;
    let global_2fa = db.is_global_2fa_enabled();
    Some(CurrentUser::from_row(row, repos, commands, global_2fa))
}

pub enum LoginOutcome {
    Session(String),
    Require2fa {
        pending_token: String,
        user: UserRow,
    },
    Invalid,
    /// Too many consecutive failures — caller should surface the remaining time.
    Locked(i64),
}

/// A throwaway Argon2 hash used to equalise the cost of "user does not exist"
/// and "wrong password" responses. Computing it once keeps the timing signal flat
/// without paying the hashing cost on every request.
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY
        .get_or_init(|| {
            hash_password("sugie-timing-equaliser-not-a-real-password").unwrap_or_else(|_| {
                String::from(
                    "$argon2id$v=19$m=19456,t=2,p=1$c3VnaWU$00000000000000000000000000000000",
                )
            })
        })
        .as_str()
}

/// Mint a full session for a user after authentication (password, passkey, or 2FA).
pub fn mint_session_for_user(
    db: &WebDb,
    user_id: i64,
    ip: &str,
    user_agent: &str,
) -> Result<String> {
    let token = random_token();
    db.create_session(
        &token_hash(&token),
        user_id,
        SESSION_TTL_HOURS,
        ip,
        user_agent,
    )?;
    db.touch_login(user_id)?;
    let _ = db.purge_sessions();
    // The bootstrap password has served its purpose once the first real session
    // exists — remove it so it cannot be read off disk later.
    remove_bootstrap_password_file();
    Ok(token)
}

fn bootstrap_password_path() -> std::path::PathBuf {
    std::path::PathBuf::from("data/admin-initial-password.txt")
}

pub fn remove_bootstrap_password_file() {
    let path = bootstrap_password_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

/// Verify credentials and either mint a session or return a pending 2FA challenge.
///
/// - If the user has `two_fa_policy != "disabled"` and already has 2FA enrolled
///   (TOTP or Passkey), password login requires the second factor.
/// - If 2FA is required for the user (either globally or per-user `"required"`),
///   and they have NOT enrolled 2FA yet (first login), we issue a session and
///   `gate()` redirects them to `/admin/security?setup=1` until they enroll OTP or Passkey.
pub fn login(
    db: &WebDb,
    username: &str,
    password: &str,
    ip: &str,
    user_agent: &str,
) -> Result<LoginOutcome> {
    if let Some(remaining) = db.login_lockout_remaining(username)? {
        return Ok(LoginOutcome::Locked(remaining));
    }

    let Some(row) = db.find_user(username)? else {
        // Burn the same amount of CPU as a real verification so response time
        // does not reveal whether the account exists.
        let _ = verify_password(password, dummy_hash());
        return Ok(LoginOutcome::Invalid);
    };
    if !row.active {
        let _ = verify_password(password, dummy_hash());
        return Ok(LoginOutcome::Invalid);
    }
    let Some(hash) = db.password_hash(row.id)? else {
        let _ = verify_password(password, dummy_hash());
        return Ok(LoginOutcome::Invalid);
    };
    if !verify_password(password, &hash) {
        let locked_for = db.record_login_failure(username, MAX_LOGIN_ATTEMPTS, LOCKOUT_SECS)?;
        if let Some(secs) = locked_for {
            return Ok(LoginOutcome::Locked(secs));
        }
        return Ok(LoginOutcome::Invalid);
    }

    db.clear_login_failures(row.id)?;

    if !row.is_2fa_disabled() && row.has_2fa_enrolled() {
        let pending_token = random_token();
        db.create_pending_2fa(&token_hash(&pending_token), row.id, "", ip, user_agent)?;
        return Ok(LoginOutcome::Require2fa {
            pending_token,
            user: row,
        });
    }

    let token = mint_session_for_user(db, row.id, ip, user_agent)?;
    Ok(LoginOutcome::Session(token))
}

/// Create the bootstrap `owner` account the first time the console starts.
/// Returns the generated password when one had to be invented.
pub fn bootstrap_admin(db: &WebDb) -> Result<Option<String>> {
    if db.count_users()? > 0 {
        return Ok(None);
    }

    let from_env = std::env::var("SUGIE_ADMIN_PASSWORD")
        .ok()
        .filter(|p| p.len() >= 8);
    let generated = from_env.is_none();
    let password = from_env.unwrap_or_else(random_token);

    db.create_user(
        "admin",
        &hash_password(&password)?,
        Role::Owner.as_str(),
        "System Administrator",
    )?;

    if generated {
        // Keep the secret out of shell history / process listings. Create the
        // file with 0600 from the very first byte so there is no window where it
        // is world-readable.
        let path = bootstrap_password_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
            {
                let _ = writeln!(f, "admin / {password}");
            }
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::write(&path, format!("admin / {password}\n"));
        }
    }

    Ok(generated.then_some(password))
}

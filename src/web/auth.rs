//! Password hashing, opaque session cookies and principal resolution.

use crate::web::db::{UserRow, WebDb};
use crate::web::perms::{CurrentUser, Role};
use anyhow::{anyhow, Result};
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
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

/// Hash a password with Argon2id and a fresh random salt.
///
/// `password-hash` 0.6 draws the salt inside `hash_password` itself (via the
/// `getrandom` feature, which `argon2` enables by default), so there is no
/// `SaltString` to generate and pass along any more.
pub fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
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

/// `hash_password` off the async runtime: Argon2id needs ~19 MiB and tens of
/// milliseconds, which would otherwise stall every other task on the worker.
pub async fn hash_password_async(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| anyhow!("hash password task: {e}"))?
}

/// `verify_password` off the async runtime. A failed join is treated as a
/// non-match rather than an error.
pub async fn verify_password_async(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false)
}

/// 32 random bytes, hex encoded — used as the opaque session token.
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    // Straight to the OS CSPRNG. A failure means the platform cannot provide
    // randomness at all, so there is no safe value to fall back to.
    getrandom::fill(&mut buf).expect("operating system randomness unavailable");
    hex::encode(buf)
}

pub fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

pub fn token_hash(token: &str) -> String {
    sha256_hex(&format!("sugie-admin-session:{token}"))
}

/// Normalise a cookie value: RFC 6265 allows a quoted value, and an empty one is
/// treated as absent so it cannot shadow a valid cookie under the other name.
fn normalize_cookie_value(raw: &str) -> Option<String> {
    let mut v = raw.trim();
    if let Some(rest) = v.strip_prefix('"') {
        v = rest;
    }
    if let Some(rest) = v.strip_suffix('"') {
        v = rest;
    }
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Read the session cookie, honouring the `__Host-` prefix rules: the plain name
/// is only trusted on a non-TLS request, otherwise a sibling subdomain could
/// plant it and pin the victim to an attacker-controlled session.
pub fn cookie_value_for(headers: &HeaderMap, secure: bool) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    let mut plain = None;
    let mut prefixed = None;
    for part in raw.split(';') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        let Some(v) = normalize_cookie_value(v) else {
            continue;
        };
        if k == SECURE_COOKIE_NAME {
            prefixed.get_or_insert(v);
        } else if k == COOKIE_NAME && !secure {
            plain.get_or_insert(v);
        }
    }
    // Prefer the `__Host-` value when both are present.
    prefixed.or(plain)
}

pub fn cookie_value(headers: &HeaderMap) -> Option<String> {
    cookie_value_for(headers, is_secure_request(headers))
}

/// Every distinct session token presented in the request, under either cookie
/// name. Used by logout so a request carrying both cookies kills both sessions.
pub fn cookie_tokens(headers: &HeaderMap) -> Vec<String> {
    let Some(raw) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(';') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        let Some(v) = normalize_cookie_value(v) else {
            continue;
        };
        if (k == COOKIE_NAME || k == SECURE_COOKIE_NAME) && !out.iter().any(|seen| seen == &v) {
            out.push(v);
        }
    }
    out
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
/// stale session behind. Each deletion is a separate header value: browsers do
/// not split a single `Set-Cookie` on commas, so a combined value clears neither.
pub fn clear_cookie(secure: bool) -> Vec<String> {
    let plain = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    let prefixed =
        format!("{SECURE_COOKIE_NAME}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0");
    if secure {
        vec![prefixed, plain]
    } else {
        vec![plain, prefixed]
    }
}

fn trust_proxy_headers() -> bool {
    if let Ok(raw) = std::env::var("SUGIE_TRUST_PROXY") {
        if matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ) {
            return true;
        }
    }
    std::env::var("SUGIE_TRUSTED_PROXIES")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

fn trusted_proxy_ips() -> Vec<std::net::IpAddr> {
    std::env::var("SUGIE_TRUSTED_PROXIES")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse::<std::net::IpAddr>().ok())
        .collect()
}

/// Extract the client IP from proxy headers only when `SUGIE_TRUST_PROXY=true`
/// or `SUGIE_TRUSTED_PROXIES` is configured, and strictly validate `IpAddr` syntax
/// to prevent X-Forwarded-For spoofing and log injection.
pub fn client_ip(headers: &HeaderMap) -> String {
    if !trust_proxy_headers() {
        return "-".to_string();
    }
    let trusted = trusted_proxy_ips();
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        let hops: Vec<std::net::IpAddr> = xff
            .split(',')
            .filter_map(|part| part.trim().parse::<std::net::IpAddr>().ok())
            .collect();
        if !hops.is_empty() {
            if !trusted.is_empty() {
                // Walk right-to-left and pick the first hop not in the trusted proxy set
                if let Some(ip) = hops.iter().rev().find(|ip| !trusted.contains(ip)) {
                    return ip.to_string();
                }
            }
            // Rightmost hop appended by the reverse proxy
            if let Some(ip) = hops.last() {
                return ip.to_string();
            }
        }
    }
    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if let Ok(ip) = real_ip.trim().parse::<std::net::IpAddr>() {
            return ip.to_string();
        }
    }
    "-".to_string()
}

/// Per-IP sliding-window rate limiter for `POST /admin/login` and `POST /admin/login/2fa`
/// (max 20 attempts per 60s per IP) to prevent login / Argon2 / 2FA flood DoS.
pub fn login_rate_limit_allowed(ip: &str) -> bool {
    const MAX_ATTEMPTS_PER_MINUTE: u32 = 20;
    static HITS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, u32)>>,
    > = std::sync::OnceLock::new();
    let hits = HITS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = hits
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = std::time::Instant::now();
    if guard.len() > 4096 {
        guard.retain(|_, (since, _)| {
            now.duration_since(*since) < std::time::Duration::from_secs(60)
        });
    }
    let entry = guard.entry(ip.to_string()).or_insert((now, 0));
    if now.duration_since(entry.0) >= std::time::Duration::from_secs(60) {
        *entry = (now, 0);
    }
    entry.1 += 1;
    entry.1 <= MAX_ATTEMPTS_PER_MINUTE
}

/// A principal resolved once per request by the audit middleware.
///
/// The raw cookie token is kept alongside the user so a cached principal can
/// only ever be returned for the *same* cookie: a stale task-local scope can
/// never leak one request's identity into another.
#[derive(Clone)]
pub struct RequestPrincipal {
    token: Option<String>,
    pub user: Option<CurrentUser>,
}

impl RequestPrincipal {
    pub fn username(&self) -> Option<&str> {
        self.user.as_ref().map(|u| u.username.as_str())
    }
}

tokio::task_local! {
    /// Set by the HTTP audit middleware around the rest of the request. Reading
    /// it turns two `session_principal` queries per admin page into one.
    static REQUEST_PRINCIPAL: RequestPrincipal;
}

fn lookup(db: &WebDb, token: &str) -> Option<CurrentUser> {
    let (row, repos, commands) = db.session_principal(&token_hash(token)).ok().flatten()?;
    let global_2fa = db.is_global_2fa_enabled();
    Some(CurrentUser::from_row(row, repos, commands, global_2fa))
}

/// Resolve the caller for a request up front (once), so the audit log and the
/// page handlers can share the result.
pub fn resolve_principal(db: &WebDb, headers: &HeaderMap) -> RequestPrincipal {
    let token = cookie_value(headers);
    let user = token.as_deref().and_then(|t| lookup(db, t));
    RequestPrincipal { token, user }
}

/// Run `fut` with `principal` visible to [`current_user`]. The handler runs
/// inside this future, so no request state has to be threaded through 39
/// handler signatures.
pub async fn scope_principal<F>(principal: RequestPrincipal, fut: F) -> F::Output
where
    F: std::future::Future,
{
    REQUEST_PRINCIPAL.scope(principal, fut).await
}

/// Resolve the caller from the session cookie. Returns `None` when the caller is
/// anonymous, expired, or disabled.
///
/// The audit middleware already resolved this request's principal; reuse it when
/// the presented cookie matches. Otherwise (handler reached without the
/// middleware, or a different cookie) fall back to a real lookup, so behaviour
/// never depends on the middleware being installed.
pub fn current_user(db: &WebDb, headers: &HeaderMap) -> Option<CurrentUser> {
    let token = cookie_value(headers);
    if let Ok(cached) = REQUEST_PRINCIPAL.try_with(|p| p.clone()) {
        if cached.token == token {
            return cached.user;
        }
    }
    token.as_deref().and_then(|t| lookup(db, t))
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
    if let Some(remaining) = db.login_lockout_remaining_for_ip(username, ip)? {
        return Ok(LoginOutcome::Locked(remaining));
    }

    let Some(row) = db.find_user(username)? else {
        // Burn the same amount of CPU as a real verification so response time
        // does not reveal whether the account exists.
        let _ = verify_password(password, dummy_hash());
        let _ = db.record_login_failure_for_ip(username, ip, MAX_LOGIN_ATTEMPTS, LOCKOUT_SECS);
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
        let locked_for =
            db.record_login_failure_for_ip(username, ip, MAX_LOGIN_ATTEMPTS, LOCKOUT_SECS)?;
        if let Some(secs) = locked_for {
            return Ok(LoginOutcome::Locked(secs));
        }
        return Ok(LoginOutcome::Invalid);
    }

    db.clear_login_failures_for_ip(row.id, username, ip)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header, HeaderValue};

    fn fake_user(name: &str) -> CurrentUser {
        CurrentUser {
            id: 1,
            username: name.to_string(),
            display_name: name.to_string(),
            role: Role::parse("admin"),
            repos: Vec::new(),
            commands: Vec::new(),
            totp_enabled: false,
            two_fa_policy: "disabled".to_string(),
            passkey_count: 0,
            needs_2fa_setup: false,
        }
    }

    fn cookie_header(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{COOKIE_NAME}={token}")).expect("cookie header"),
        );
        headers
    }

    #[tokio::test]
    async fn cached_principal_avoids_a_second_lookup() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        let headers = cookie_header("cached-token");
        // The database holds no session for this token: only the request-scoped
        // principal can answer, which is exactly the single-lookup path.
        let principal = RequestPrincipal {
            token: Some("cached-token".to_string()),
            user: Some(fake_user("alice")),
        };
        let seen = scope_principal(principal, async { current_user(&db, &headers) }).await;
        assert_eq!(seen.map(|u| u.username), Some("alice".to_string()));
    }

    #[tokio::test]
    async fn another_cookie_is_never_served_from_the_cache() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        let headers = cookie_header("attacker-token");
        let principal = RequestPrincipal {
            token: Some("cached-token".to_string()),
            user: Some(fake_user("alice")),
        };
        let seen = scope_principal(principal, async { current_user(&db, &headers) }).await;
        assert!(
            seen.is_none(),
            "a cached principal must not answer for a different cookie"
        );
    }

    #[tokio::test]
    async fn anonymous_request_resolves_to_no_principal() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        let principal = resolve_principal(&db, &HeaderMap::new());
        assert!(principal.user.is_none());
        assert!(principal.username().is_none());
    }

    #[test]
    fn per_ip_lockout_isolates_attacker_from_legitimate_ip() {
        let db = WebDb::new_in_memory().expect("in-memory db");
        let pw_hash = hash_password("correct-horse-battery-staple").expect("hash");
        db.create_user("admin", &pw_hash, Role::Owner.as_str(), "Admin")
            .expect("create user");

        // Attacker on 203.0.113.99 fails 5 times and gets locked out
        for i in 1..=MAX_LOGIN_ATTEMPTS {
            let outcome = login(&db, "admin", "wrong-password", "203.0.113.99", "curl").expect("login");
            if i < MAX_LOGIN_ATTEMPTS {
                assert!(matches!(outcome, LoginOutcome::Invalid));
            } else {
                assert!(matches!(outcome, LoginOutcome::Locked(_)));
            }
        }

        // Subsequent attempt from attacker IP with even the RIGHT password is still locked
        let attacker_retry = login(
            &db,
            "admin",
            "correct-horse-battery-staple",
            "203.0.113.99",
            "curl",
        )
        .expect("login");
        assert!(matches!(attacker_retry, LoginOutcome::Locked(_)));

        // Meanwhile, legitimate admin on 198.51.100.10 can still sign in!
        let legit = login(
            &db,
            "admin",
            "correct-horse-battery-staple",
            "198.51.100.10",
            "Mozilla/5.0",
        )
        .expect("login");
        assert!(
            matches!(legit, LoginOutcome::Session(_)),
            "attacker lockout on one IP must not lock out legitimate users on other IPs"
        );
    }

    #[test]
    fn client_ip_ignores_untrusted_proxy_headers_by_default() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.2.3.4, 5.6.7.8"),
        );
        headers.insert("x-real-ip", HeaderValue::from_static("9.9.9.9"));
        // Without SUGIE_TRUST_PROXY / SUGIE_TRUSTED_PROXIES, spoofed headers are ignored
        assert_eq!(client_ip(&headers), "-");
    }
}

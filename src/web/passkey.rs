//! WebAuthn / FIDO2 passkey and YubiKey hardware authentication manager.
//!
//! Powered by `webauthn-rs` (0.5), supporting passwordless discoverable login,
//! second-factor passkey verification, and hardware authenticator registration.

use base64::Engine;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;
use webauthn_rs::prelude::*;

fn b64url_encode(data: impl AsRef<[u8]>) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

pub fn user_uuid(user_id: i64) -> Uuid {
    Uuid::from_u128(user_id.max(0) as u128)
}

pub fn user_id_from_uuid(uuid: Uuid) -> i64 {
    uuid.as_u128() as i64
}

fn read_env_first(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(v) = env::var(k) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[derive(Clone, Debug)]
struct RegEntry {
    user_id: i64,
    state: PasskeyRegistration,
    created_at: Instant,
}

#[derive(Clone, Debug)]
struct AuthEntry {
    state: DiscoverableAuthentication,
    created_at: Instant,
}

/// Upper bound on pending authentication ceremonies. The auth-start endpoint is
/// unauthenticated, so the map is otherwise only bounded by the 300 s TTL.
const MAX_AUTH_CHALLENGES: usize = 4096;
/// Passkey auth-start requests allowed per source address per minute.
const AUTH_STARTS_PER_MINUTE: u32 = 20;

/// Coarse in-memory per-IP throttle for the unauthenticated auth-start endpoint.
pub fn auth_start_allowed(ip: &str) -> bool {
    static HITS: std::sync::OnceLock<Mutex<HashMap<String, (Instant, u32)>>> =
        std::sync::OnceLock::new();
    let hits = HITS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = hits
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    // Opportunistic sweep so a spray across many source addresses cannot grow
    // the map without bound.
    if guard.len() > 4096 {
        guard.retain(|_, (since, _)| now.duration_since(*since) < Duration::from_secs(60));
    }
    let entry = guard.entry(ip.to_string()).or_insert((now, 0));
    if now.duration_since(entry.0) >= Duration::from_secs(60) {
        *entry = (now, 0);
    }
    entry.1 += 1;
    entry.1 <= AUTH_STARTS_PER_MINUTE
}

/// Manager for WebAuthn/FIDO2 passkey registration and assertion ceremonies.
#[derive(Clone)]
pub struct PasskeyManager {
    webauthn: Arc<Webauthn>,
    rp_id: String,
    rp_origin: String,
    rp_name: String,
    reg_challenges: Arc<Mutex<HashMap<String, RegEntry>>>,
    auth_challenges: Arc<Mutex<HashMap<String, AuthEntry>>>,
}

impl PasskeyManager {
    /// Load Relying Party configuration (`rp_id`, `rp_origin`, `rp_name`) from environment
    /// variables (`SUGIE_WEBAUTHN_RP_ID`, `SUGIE_WEBAUTHN_RP_ORIGIN`, `SUGIE_WEBAUTHN_RP_NAME`,
    /// or unprefixed `WEBAUTHN_RP_ID`, `WEBAUTHN_RP_ORIGIN`, `WEBAUTHN_RP_NAME`, `RP_ID`, `RP_ORIGIN`, `RP_NAME`).
    pub fn from_env(default_port: u16) -> Self {
        let default_origin = format!("http://localhost:{default_port}");

        let rp_origin = read_env_first(&[
            "SUGIE_WEBAUTHN_RP_ORIGIN",
            "WEBAUTHN_RP_ORIGIN",
            "RP_ORIGIN",
        ])
        .unwrap_or_else(|| default_origin.clone());

        let rp_id = read_env_first(&["SUGIE_WEBAUTHN_RP_ID", "WEBAUTHN_RP_ID", "RP_ID"])
            .unwrap_or_else(|| {
                Url::parse(&rp_origin)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_string()))
                    .unwrap_or_else(|| "localhost".to_string())
            });

        let rp_name = read_env_first(&["SUGIE_WEBAUTHN_RP_NAME", "WEBAUTHN_RP_NAME", "RP_NAME"])
            .unwrap_or_else(|| "Sugie Admin".to_string());

        Self::new(&rp_id, &rp_origin, &rp_name, default_port)
    }

    pub fn new(rp_id: &str, rp_origin: &str, rp_name: &str, fallback_port: u16) -> Self {
        let build_try = |id: &str, origin_str: &str, name: &str| -> Result<Webauthn, String> {
            let parsed_origin = Url::parse(origin_str)
                .map_err(|e| format!("Invalid rp_origin {origin_str:?}: {e}"))?;
            let builder = WebauthnBuilder::new(id, &parsed_origin)
                .map_err(|e| format!("Invalid WebAuthn RP ({id:?}, {origin_str:?}): {e}"))?
                .rp_name(name);
            builder
                .build()
                .map_err(|e| format!("Failed to build Webauthn: {e}"))
        };

        let (webauthn, final_id, final_origin) = match build_try(rp_id, rp_origin, rp_name) {
            Ok(w) => (w, rp_id.to_string(), rp_origin.to_string()),
            Err(e) => {
                let fb_origin = format!("http://localhost:{fallback_port}");
                tracing::warn!(
                    "WebAuthn RP config ({rp_id}, {rp_origin}) rejected ({e}); falling back to localhost ({fb_origin})"
                );
                let w = build_try("localhost", &fb_origin, rp_name)
                    .expect("Fallback localhost WebAuthn builder must succeed");
                (w, "localhost".to_string(), fb_origin)
            }
        };

        tracing::info!(
            rp_id = %final_id,
            rp_origin = %final_origin,
            rp_name = %rp_name,
            "WebAuthn / Passkey manager initialized"
        );

        Self {
            webauthn: Arc::new(webauthn),
            rp_id: final_id,
            rp_origin: final_origin,
            rp_name: rp_name.to_string(),
            reg_challenges: Arc::new(Mutex::new(HashMap::new())),
            auth_challenges: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn rp_id(&self) -> &str {
        &self.rp_id
    }

    pub fn rp_origin(&self) -> &str {
        &self.rp_origin
    }

    pub fn rp_name(&self) -> &str {
        &self.rp_name
    }

    /// Starts a passkey registration ceremony for a user.
    pub fn start_registration(
        &self,
        user_id: i64,
        username: &str,
        display_name: &str,
        exclude_credentials: Option<Vec<CredentialID>>,
    ) -> Result<CreationChallengeResponse, String> {
        let u_uuid = user_uuid(user_id);
        let disp = if display_name.trim().is_empty() {
            username
        } else {
            display_name
        };
        let (mut ccr, reg_state) = self
            .webauthn
            .start_passkey_registration(u_uuid, username, disp, exclude_credentials)
            .map_err(|e| format!("Failed to start passkey registration: {e}"))?;

        if let Some(selection) = ccr.public_key.authenticator_selection.as_mut() {
            selection.resident_key = serde_json::from_str("\"required\"").ok();
            selection.require_resident_key = true;
        }

        let challenge_id = b64url_encode(&ccr.public_key.challenge);

        let mut lock = self
            .reg_challenges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lock.retain(|_, v| v.created_at.elapsed() < Duration::from_secs(300));
        lock.insert(
            challenge_id,
            RegEntry {
                user_id,
                state: reg_state,
                created_at: Instant::now(),
            },
        );

        Ok(ccr)
    }

    /// Finishes a passkey registration ceremony and verifies the authenticator attestation.
    pub fn finish_registration(
        &self,
        user_id: i64,
        reg: &RegisterPublicKeyCredential,
    ) -> Result<Passkey, String> {
        #[derive(serde::Deserialize)]
        struct ClientData {
            challenge: String,
        }
        let challenge_str = serde_json::from_slice::<ClientData>(&reg.response.client_data_json)
            .map(|c| c.challenge)
            .unwrap_or_default();

        let mut lock = self
            .reg_challenges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lock.retain(|_, v| v.created_at.elapsed() < Duration::from_secs(300));

        // Match strictly on the challenge the client signed. A fallback such as
        // "take any pending challenge for this user" would let one browser tab
        // consume another tab's (or another session's) ceremony.
        let entry = lock.remove(&challenge_str).ok_or_else(|| {
            "No registration ceremony in progress or challenge expired".to_string()
        })?;

        if entry.user_id != user_id {
            return Err("User mismatch during passkey registration".to_string());
        }

        self.webauthn
            .finish_passkey_registration(reg, &entry.state)
            .map_err(|e| format!("WebAuthn registration verification failed: {e}"))
    }

    /// Starts a discoverable authentication ceremony (usernameless 1-touch login or 2FA verification).
    pub fn start_discoverable_authentication(&self) -> Result<RequestChallengeResponse, String> {
        let (mut rcr, auth_state) = self
            .webauthn
            .start_discoverable_authentication()
            .map_err(|e| format!("Failed to start passkey authentication: {e}"))?;

        rcr.mediation = None;

        let challenge_id = b64url_encode(&rcr.public_key.challenge);
        let mut lock = self
            .auth_challenges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lock.retain(|_, v| v.created_at.elapsed() < Duration::from_secs(300));
        // Evict the oldest ceremonies before inserting so a caller hammering this
        // endpoint cannot grow the map past the cap.
        if lock.len() >= MAX_AUTH_CHALLENGES {
            let mut oldest: Vec<(Instant, String)> = lock
                .iter()
                .map(|(k, v)| (v.created_at, k.clone()))
                .collect();
            oldest.sort_by(|a, b| a.0.cmp(&b.0));
            let excess = lock.len() + 1 - MAX_AUTH_CHALLENGES;
            for (_, key) in oldest.into_iter().take(excess) {
                lock.remove(&key);
            }
        }
        lock.insert(
            challenge_id,
            AuthEntry {
                state: auth_state,
                created_at: Instant::now(),
            },
        );

        Ok(rcr)
    }

    /// Identifies the claimed user ID and base64url credential ID from an incoming credential response.
    pub fn identify_credential(&self, cred: &PublicKeyCredential) -> Result<(i64, String), String> {
        let (u_uuid, cred_id_bytes) = self
            .webauthn
            .identify_discoverable_authentication(cred)
            .map_err(|e| format!("Could not identify passkey credential: {e}"))?;
        Ok((user_id_from_uuid(u_uuid), b64url_encode(cred_id_bytes)))
    }

    /// Finishes a discoverable authentication ceremony against the user's registered keys.
    pub fn finish_discoverable_authentication(
        &self,
        cred: &PublicKeyCredential,
        discoverable_keys: &[DiscoverableKey],
    ) -> Result<AuthenticationResult, String> {
        #[derive(serde::Deserialize)]
        struct ClientData {
            challenge: String,
        }
        let challenge_str = serde_json::from_slice::<ClientData>(&cred.response.client_data_json)
            .map(|c| c.challenge)
            .unwrap_or_default();

        let mut lock = self
            .auth_challenges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lock.retain(|_, v| v.created_at.elapsed() < Duration::from_secs(300));

        let entry = lock.remove(&challenge_str).ok_or_else(|| {
            "No authentication ceremony in progress or challenge expired".to_string()
        })?;

        self.webauthn
            .finish_discoverable_authentication(cred, entry.state, discoverable_keys)
            .map_err(|e| format!("Passkey authentication verification failed: {e}"))
    }
}

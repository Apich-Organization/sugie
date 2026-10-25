use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct GitHubAuthManager {
    app_id: u64,
    private_key_pem: String,
    http_client: reqwest::Client,
    base_url: String,
    token_cache: Arc<RwLock<HashMap<u64, CachedToken>>>,
}

#[derive(Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: DateTime<Utc>,
}

impl GitHubAuthManager {
    pub fn new(app_id: u64, private_key_pem: String) -> Self {
        Self {
            app_id,
            private_key_pem,
            http_client: reqwest::Client::builder()
                .user_agent("sugie-bot/0.1.0")
                .build()
                .unwrap_or_default(),
            base_url: "https://api.github.com".to_string(),
            token_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    pub fn generate_jwt(&self) -> Result<String> {
        let now = Utc::now();
        let iat = now - Duration::seconds(60);
        let exp = now + Duration::minutes(9);

        let claims = Claims {
            iat: iat.timestamp(),
            exp: exp.timestamp(),
            iss: self.app_id.to_string(),
        };

        let encoding_key = EncodingKey::from_rsa_pem(self.private_key_pem.as_bytes())
            .context("Failed to parse GitHub App RSA private key")?;

        let header = Header::new(Algorithm::RS256);
        let token = encode(&header, &claims, &encoding_key)
            .context("Failed to encode and sign GitHub App JWT")?;

        Ok(token)
    }

    pub async fn get_installation_token(&self, installation_id: u64) -> Result<String> {
        let now = Utc::now();

        // Check cache first
        {
            let cache = self.token_cache.read().await;
            if let Some(cached) = cache.get(&installation_id) {
                // If token still valid for more than 3 minutes, use it
                if cached.expires_at > now + Duration::minutes(3) {
                    return Ok(cached.token.clone());
                }
            }
        }

        // Generate JWT to request installation token
        let jwt = self.generate_jwt()?;
        let url = format!(
            "{}/app/installations/{}/access_tokens",
            self.base_url, installation_id
        );

        let res = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", jwt))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .context("Failed to send installation token request to GitHub API")?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to get installation token for {}: HTTP {} - {}",
                installation_id,
                status,
                body
            );
        }

        let token_resp: InstallationTokenResponse = res
            .json()
            .await
            .context("Failed to parse installation token response")?;

        // Update cache
        {
            let mut cache = self.token_cache.write().await;
            cache.insert(
                installation_id,
                CachedToken {
                    token: token_resp.token.clone(),
                    expires_at: token_resp.expires_at,
                },
            );
        }

        Ok(token_resp.token)
    }
}

use crate::config::SugieConfig;
use crate::github::models::*;
use crate::github::webhook::verify_signature;
use crate::triage::TriageService;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde_json::json;
use std::sync::{Arc, RwLock};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

pub mod chat;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<SugieConfig>>,
    pub triage: TriageService,
    /// Shared secret for `POST /api/chat/command`. `None` keeps the endpoint
    /// open (the historical behaviour) and is warned about at startup.
    pub chat_api_token: Option<Arc<str>>,
    /// Console database, used to record the chat-bot exchanges shown on
    /// `/admin/chats`. `None` when the admin console is not mounted: the bot
    /// still answers, it just has nowhere to keep the conversation.
    pub web_db: Option<crate::web::db::WebDb>,
}

impl AppState {
    pub fn config(&self) -> SugieConfig {
        self.config.read().map(|c| c.clone()).unwrap_or_default()
    }
}

pub fn create_router(state: AppState) -> Router {
    create_router_with_web(state, None)
}

/// Build the bot router and optionally mount the admin console at `/admin`.
pub fn create_router_with_web(state: AppState, web: Option<crate::web::WebState>) -> Router {
    let web_db = web.as_ref().map(|w| w.db.clone());
    let github_webhook_routes = Router::new()
        .route("/webhook", post(github_webhook_handler))
        .route("/api/github/webhook", post(github_webhook_handler))
        .layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024));

    let chat_webhook_routes = Router::new()
        .route("/api/chat/zulip", post(chat::zulip_chat_handler))
        .route("/api/chat/lark", post(chat::lark_chat_handler))
        .route("/api/chat/discord", post(chat::discord_chat_handler))
        .route(
            "/api/chat/command",
            post(chat::generic_chat_command_handler),
        )
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024));

    let router = Router::new()
        .route("/health", get(health_check))
        .route("/status", get(status_check))
        .merge(github_webhook_routes)
        .merge(chat_webhook_routes)
        .with_state(state);

    let merged = match web {
        Some(web_state) => router.merge(crate::web::router(web_state)),
        None => router,
    };

    merged
        .layer(axum::middleware::from_fn_with_state(
            web_db,
            security_http_log_middleware,
        ))
        .layer(TraceLayer::new_for_http())
}

fn sanitize_log_val(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || c == '\r' || c == '\n' {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

async fn security_http_log_middleware(
    State(web_db): State<Option<crate::web::db::WebDb>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path().to_string();
    // Skip static assets to keep security review logs focused on API/webhook/admin requests
    let skip_log = path.starts_with("/admin/assets/") || path == "/favicon.ico";

    let method = req.method().as_str().to_string();
    let uri = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| path.clone());

    let headers = req.headers();
    let ip = sanitize_log_val(&crate::web::auth::client_ip(headers));
    let ua = sanitize_log_val(
        headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-"),
    );
    let gh_event = headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .map(sanitize_log_val);
    let gh_delivery = headers
        .get("X-GitHub-Delivery")
        .and_then(|v| v.to_str().ok())
        .map(sanitize_log_val);

    // One session lookup for the whole request: the audit log needs the actor
    // and the page handlers need the principal.
    let principal = web_db
        .as_ref()
        .map(|db| crate::web::auth::resolve_principal(db, headers));

    let actor = if gh_event.is_some() {
        "github-webhook".to_string()
    } else if path.starts_with("/api/chat/discord") {
        "discord-webhook".to_string()
    } else if path.starts_with("/api/chat/lark") {
        "lark-webhook".to_string()
    } else if path.starts_with("/api/chat/zulip") {
        "zulip-webhook".to_string()
    } else if web_db.is_some() {
        sanitize_log_val(
            principal
                .as_ref()
                .and_then(|p| p.username())
                .unwrap_or("anonymous"),
        )
    } else {
        "anonymous".to_string()
    };

    let mut detail_parts = Vec::new();
    if let Some(ev) = gh_event {
        detail_parts.push(format!("github_event={ev}"));
    }
    if let Some(del) = gh_delivery {
        detail_parts.push(format!("delivery={del}"));
    }
    if ua != "-" && !ua.is_empty() {
        detail_parts.push(format!("ua={ua}"));
    }
    let detail = detail_parts.join(" ");

    let start = std::time::Instant::now();
    // Resolve the session once and hand it to the inner router through a
    // task-local, so page handlers do not repeat the lookup. Anonymous and
    // non-admin routes skip the scope entirely.
    let secure = crate::web::auth::is_secure_request(headers);
    let mut resp = match principal {
        Some(p) => crate::web::auth::scope_principal(p, next.run(req)).await,
        None => next.run(req).await,
    };
    {
        let resp_headers = resp.headers_mut();
        resp_headers
            .entry("x-content-type-options")
            .or_insert(axum::http::HeaderValue::from_static("nosniff"));
        resp_headers
            .entry("x-frame-options")
            .or_insert(axum::http::HeaderValue::from_static("DENY"));
        resp_headers
            .entry("referrer-policy")
            .or_insert(axum::http::HeaderValue::from_static("same-origin"));
        if secure {
            resp_headers.entry("strict-transport-security").or_insert(
                axum::http::HeaderValue::from_static("max-age=31536000; includeSubDomains"),
            );
        }
    }
    if !skip_log {
        let status = resp.status().as_u16();
        let duration_ms = start.elapsed().as_millis() as u64;
        crate::security_log::log_inbound_http(
            &method,
            &uri,
            status,
            duration_ms,
            &ip,
            &actor,
            &detail,
        );
    }
    resp
}

async fn health_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "name": "sugie",
            "version": env!("CARGO_PKG_VERSION")
        })),
    )
}

async fn status_check(State(state): State<AppState>) -> impl IntoResponse {
    let repos = state
        .triage
        .storage()
        .list_repositories()
        .unwrap_or_default();
    // This route is unauthenticated, so it reports only liveness and a count.
    // Listing the repository names handed anyone who could reach the port an
    // inventory of a private org; the console shows the full list to signed-in
    // users who are allowed to see it.
    (
        StatusCode::OK,
        Json(json!({
            "status": "running",
            "bot": "sugie",
            "version": env!("CARGO_PKG_VERSION"),
            "onboarded_repositories_count": repos.len()
        })),
    )
}

static GITHUB_DEDUP_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn is_duplicate_github_delivery(delivery_id: &str) -> bool {
    let clean = delivery_id.trim();
    if clean.is_empty() {
        return false;
    }
    let mut cache = GITHUB_DEDUP_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = std::time::Instant::now();
    cache.retain(|_, seen_at| now.duration_since(*seen_at).as_secs() < 600);
    if cache.contains_key(clean) {
        true
    } else {
        cache.insert(clean.to_string(), now);
        false
    }
}

async fn github_webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let event_name = match headers.get("X-GitHub-Event").and_then(|v| v.to_str().ok()) {
        Some(e) => e,
        None => {
            warn!("Missing X-GitHub-Event header");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Missing X-GitHub-Event header" })),
            );
        }
    };

    let signature = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok());

    // A missing secret used to leave this endpoint forgeable: HMAC accepts an
    // empty key, so `sha256=HMAC(key="", body)` would verify. Refuse outright
    // and say why, instead of logging it as an ordinary signature failure.
    let webhook_secret = state
        .config
        .read()
        .map(|c| c.github.webhook_secret.clone())
        .unwrap_or_default();
    if webhook_secret.is_empty() {
        warn!(
            "Rejected GitHub webhook: SUGIE_GITHUB_WEBHOOK_SECRET is not configured, \
             so deliveries cannot be authenticated"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "Webhook secret is not configured" })),
        );
    }

    // Verify HMAC-SHA256 signature
    if !verify_signature(&webhook_secret, signature, &body) {
        warn!("Webhook signature verification failed");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid webhook signature" })),
        );
    }

    // Deduplicate deliveries using X-GitHub-Delivery (replay attack prevention,
    // backed by both in-memory cache and persistent SQLite storage)
    if let Some(delivery) = headers
        .get("X-GitHub-Delivery")
        .and_then(|v| v.to_str().ok())
    {
        let clean = delivery.trim();
        if !clean.is_empty() {
            let db_dup = state
                .triage
                .storage()
                .check_and_record_webhook_delivery(&format!("github:{clean}"), 3600);
            let mem_dup = is_duplicate_github_delivery(clean);
            if db_dup || mem_dup {
                info!("Ignoring replayed GitHub webhook delivery: {}", clean);
                return (
                    StatusCode::OK,
                    Json(json!({ "message": "Duplicate delivery ignored" })),
                );
            }
        }
    }

    if event_name == "ping" {
        info!("Received GitHub ping event");
        return (
            StatusCode::OK,
            Json(json!({ "message": "Pong from Sugie Bot!" })),
        );
    }

    let triage = state.triage.clone();
    let event_type = event_name.to_string();
    let body_bytes = body.to_vec();

    // Spawn async background processing so GitHub webhook request responds immediately
    tokio::spawn(async move {
        match event_type.as_str() {
            "installation_repositories" => {
                match serde_json::from_slice::<InstallationRepositoriesWebhookEvent>(&body_bytes) {
                    Ok(payload) => {
                        if let Err(e) = triage.handle_installation_repositories(payload).await {
                            error!("Error in handle_installation_repositories: {:?}", e);
                        }
                    }
                    Err(e) => warn!("Failed to parse installation_repositories event: {:?}", e),
                }
            }
            "installation" => {
                match serde_json::from_slice::<InstallationWebhookEvent>(&body_bytes) {
                    Ok(payload) => {
                        if let Err(e) = triage.handle_installation(payload).await {
                            error!("Error in handle_installation: {:?}", e);
                        }
                    }
                    Err(e) => warn!("Failed to parse installation event: {:?}", e),
                }
            }
            "issues" => match serde_json::from_slice::<IssueWebhookEvent>(&body_bytes) {
                Ok(payload) => {
                    if let Err(e) = triage.handle_issue(payload).await {
                        error!("Error in handle_issue: {:?}", e);
                    }
                }
                Err(e) => warn!("Failed to parse issues event: {:?}", e),
            },
            "pull_request" => {
                match serde_json::from_slice::<PullRequestWebhookEvent>(&body_bytes) {
                    Ok(payload) => {
                        if let Err(e) = triage.handle_pull_request(payload).await {
                            error!("Error in handle_pull_request: {:?}", e);
                        }
                    }
                    Err(e) => warn!("Failed to parse pull_request event: {:?}", e),
                }
            }
            "issue_comment" => {
                match serde_json::from_slice::<IssueCommentWebhookEvent>(&body_bytes) {
                    Ok(payload) => {
                        if let Err(e) = triage.handle_issue_comment(payload).await {
                            error!("Error in handle_issue_comment: {:?}", e);
                        }
                    }
                    Err(e) => warn!("Failed to parse issue_comment event: {:?}", e),
                }
            }
            "check_suite" => match serde_json::from_slice::<CheckSuiteWebhookEvent>(&body_bytes) {
                Ok(payload) => {
                    if let Err(e) = triage.handle_check_suite(payload).await {
                        error!("Error in handle_check_suite: {:?}", e);
                    }
                }
                Err(e) => warn!("Failed to parse check_suite event: {:?}", e),
            },
            other => {
                info!("Ignored unhandled GitHub event: {}", other);
            }
        }
    });

    (
        StatusCode::OK,
        Json(json!({ "status": "accepted", "event": event_name })),
    )
}

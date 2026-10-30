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
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

pub mod chat;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<SugieConfig>,
    pub triage: TriageService,
}

pub fn create_router(state: AppState) -> Router {
    create_router_with_web(state, None)
}

/// Build the bot router and optionally mount the admin console at `/admin`.
pub fn create_router_with_web(state: AppState, web: Option<crate::web::WebState>) -> Router {
    let web_db = web.as_ref().map(|w| w.db.clone());
    let router = Router::new()
        .route("/health", get(health_check))
        .route("/status", get(status_check))
        .route("/webhook", post(github_webhook_handler))
        .route("/api/github/webhook", post(github_webhook_handler))
        .route("/api/chat/zulip", post(chat::zulip_chat_handler))
        .route("/api/chat/lark", post(chat::lark_chat_handler))
        .route("/api/chat/discord", post(chat::discord_chat_handler))
        .route(
            "/api/chat/command",
            post(chat::generic_chat_command_handler),
        )
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
    let ip = crate::web::auth::client_ip(headers);
    let ua = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string();
    let gh_event = headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let gh_delivery = headers
        .get("X-GitHub-Delivery")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let actor = if gh_event.is_some() {
        "github-webhook".to_string()
    } else if path.starts_with("/api/chat/discord") {
        "discord-webhook".to_string()
    } else if path.starts_with("/api/chat/lark") {
        "lark-webhook".to_string()
    } else if path.starts_with("/api/chat/zulip") {
        "zulip-webhook".to_string()
    } else if let Some(ref db) = web_db {
        crate::web::auth::current_user(db, headers)
            .map(|u| u.username)
            .unwrap_or_else(|| "anonymous".to_string())
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
    let resp = next.run(req).await;
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
    (
        StatusCode::OK,
        Json(json!({
            "status": "running",
            "bot": "sugie",
            "version": env!("CARGO_PKG_VERSION"),
            "onboarded_repositories_count": repos.len(),
            "repositories": repos.into_iter().map(|(_, name, _)| name).collect::<Vec<_>>()
        })),
    )
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

    // Verify HMAC-SHA256 signature
    if !verify_signature(&state.config.github.webhook_secret, signature, &body) {
        warn!("Webhook signature verification failed");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid webhook signature" })),
        );
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

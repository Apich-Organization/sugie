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

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<SugieConfig>,
    pub triage: TriageService,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/status", get(status_check))
        .route("/webhook", post(github_webhook_handler))
        .route("/api/github/webhook", post(github_webhook_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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
    let repos = state.triage.storage().list_repositories().unwrap_or_default();
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
            "issues" => {
                match serde_json::from_slice::<IssueWebhookEvent>(&body_bytes) {
                    Ok(payload) => {
                        if let Err(e) = triage.handle_issue(payload).await {
                            error!("Error in handle_issue: {:?}", e);
                        }
                    }
                    Err(e) => warn!("Failed to parse issues event: {:?}", e),
                }
            }
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
            "check_suite" => {
                match serde_json::from_slice::<CheckSuiteWebhookEvent>(&body_bytes) {
                    Ok(payload) => {
                        if let Err(e) = triage.handle_check_suite(payload).await {
                            error!("Error in handle_check_suite: {:?}", e);
                        }
                    }
                    Err(e) => warn!("Failed to parse check_suite event: {:?}", e),
                }
            }
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

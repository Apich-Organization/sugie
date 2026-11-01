// The SSR pages are single deeply-nested `view!` trees. Leptos encodes every
// element's children in the type, so the admin pages sit close to rustc's
// default trait-solving depth — the release profile, which computes layouts
// differently from debug, tips over it. Raising the limit is the standard fix
// for deeply nested Leptos views.
#![recursion_limit = "256"]

pub mod chat;
pub mod config;
pub mod github;
pub mod metrics;
pub mod notifications;
pub mod security_log;
pub mod server;
pub mod stale;
pub mod storage;
pub mod triage;
pub mod web;

pub use config::SugieConfig;
pub use github::auth::GitHubAuthManager;
pub use github::client::GitHubClient;
pub use notifications::NotificationDispatcher;
pub use server::{create_router, create_router_with_web, AppState};
pub use stale::StaleManager;
pub use storage::Storage;
pub use triage::TriageService;
pub use web::WebState;

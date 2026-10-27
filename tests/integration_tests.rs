use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::sync::Arc;
use sugie::config::{
    CommandsConfig, DcoConfig, DuplicateConfig, GrowthConfig, GrowthMilestone, RepoConfig,
    ReviewersConfig, SugieConfig,
};
use sugie::github::auth::GitHubAuthManager;
use sugie::github::models::{CommitDetails, CommitItem, Issue, User};
use sugie::metrics::{MetricsReporter, TriageMetrics};
use sugie::notifications::{NotificationDispatcher, NotificationLevel, SugieEventType};
use sugie::server::{create_router, AppState};
use sugie::storage::Storage;
use sugie::triage::commands::{CommandParser, SlashCommand};
use sugie::triage::dco::DcoValidator;
use sugie::triage::duplicate::DuplicateDetector;
use sugie::triage::growth::GrowthTracker;
use sugie::triage::reviewers::ReviewerLoadBalancer;
use sugie::triage::TriageService;
use tower::ServiceExt;

type HmacSha256 = Hmac<Sha256>;

fn generate_test_signature(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let result = mac.finalize();
    format!("sha256={}", hex::encode(result.into_bytes()))
}

fn create_test_state() -> (AppState, Storage) {
    let config = SugieConfig {
        host: "127.0.0.1".to_string(),
        port: 8080,
        db_path: ":memory:".to_string(),
        github: sugie::config::GitHubAuthConfig {
            app_id: 12345,
            private_key_pem: Some("dummy-key".to_string()),
            private_key_path: None,
            webhook_secret: "webhook-secret-123".to_string(),
        },
        notifications: sugie::config::NotificationsConfig::default(),
        chat: sugie::config::ChatConfig {
            zulip_token: Some("test-zulip-token".to_string()),
            lark_verification_token: Some("test-lark-token".to_string()),
            lark_encrypt_key: Some("test-lark-encrypt-key-123456789".to_string()),
            discord_public_key: None,
        },
        default_rules: RepoConfig::default(),
    };

    let storage = Storage::new_in_memory().unwrap();
    let auth = Arc::new(GitHubAuthManager::new(12345, "dummy-key".to_string()));
    let notifier = NotificationDispatcher::new(&config.notifications);
    let triage = TriageService::new(Arc::new(config.clone()), storage.clone(), notifier, auth);

    let state = AppState {
        config: Arc::new(config),
        triage,
    };

    (state, storage)
}

#[tokio::test]
async fn test_health_endpoint() {
    let (state, _) = create_test_state();
    let app = create_router(state);

    let req = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_webhook_signature_verification_and_routing() {
    let (state, _) = create_test_state();
    let secret = "webhook-secret-123";
    let payload = br#"{"action":"ping"}"#;
    let signature = generate_test_signature(secret, payload);

    let app = create_router(state.clone());

    // 1. Valid signature
    let req = Request::builder()
        .uri("/webhook")
        .method("POST")
        .header("X-GitHub-Event", "ping")
        .header("X-Hub-Signature-256", signature)
        .body(Body::from(payload.to_vec()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 2. Invalid signature
    let app2 = create_router(state);
    let bad_req = Request::builder()
        .uri("/webhook")
        .method("POST")
        .header("X-GitHub-Event", "ping")
        .header("X-Hub-Signature-256", "sha256=invalidhex")
        .body(Body::from(payload.to_vec()))
        .unwrap();

    let resp2 = app2.oneshot(bad_req).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_growth_tracker_milestones() {
    let storage = Storage::new_in_memory().unwrap();
    let notifier = NotificationDispatcher::new(&sugie::config::NotificationsConfig::default());
    let config = GrowthConfig {
        enabled: true,
        thresholds: vec![
            GrowthMilestone {
                merged_prs: 3,
                message: "3 PRs reached for @{author}!".to_string(),
                notify_channels: false,
                recommend_role: Some("triage".to_string()),
            },
            GrowthMilestone {
                merged_prs: 10,
                message: "10 PRs reached for @{author}!".to_string(),
                notify_channels: false,
                recommend_role: Some("maintainer".to_string()),
            },
        ],
    };

    let tracker = GrowthTracker::new(config, storage.clone(), notifier);
    let user = "contributor-bob";
    let repo = "my-org/core-repo";

    // PR #1 merged -> no milestone
    let res1 = tracker.on_pr_merged(user, repo, 101, None).await.unwrap();
    assert!(res1.is_none());

    // PR #2 merged -> no milestone
    let res2 = tracker.on_pr_merged(user, repo, 102, None).await.unwrap();
    assert!(res2.is_none());

    // PR #3 merged -> milestone 3!
    let res3 = tracker.on_pr_merged(user, repo, 103, None).await.unwrap();
    assert!(res3.is_some());
    let alert = res3.unwrap();
    assert_eq!(alert.merged_prs, 3);
    assert_eq!(alert.recommend_role.as_deref(), Some("triage"));
    assert!(alert.message.contains("@contributor-bob"));

    // PR #4 merged -> no duplicate milestone notification
    let res4 = tracker.on_pr_merged(user, repo, 104, None).await.unwrap();
    assert!(res4.is_none());
}

#[tokio::test]
async fn test_reviewer_load_balancing_workload() {
    let storage = Storage::new_in_memory().unwrap();
    let config = ReviewersConfig {
        enabled: true,
        pool: vec!["reviewer-a".to_string(), "reviewer-b".to_string()],
        reviewers_per_pr: 1,
        max_active_reviews: 3,
        load_balancing: "least_busy".to_string(),
        out_of_office: vec![],
        component_pools: Default::default(),
    };

    let balancer = ReviewerLoadBalancer::new(config, storage.clone());
    let repo = "my-org/my-repo";

    let chosen = balancer.select_best_reviewers(repo, &[], &[], 1).unwrap();
    assert_eq!(chosen, vec!["reviewer-a".to_string()]);

    balancer.on_review_requested(repo, "reviewer-a").unwrap();
    let next_chosen = balancer.select_best_reviewers(repo, &[], &[], 1).unwrap();
    assert_eq!(next_chosen, vec!["reviewer-b".to_string()]);

    balancer.on_review_requested(repo, "reviewer-b").unwrap();
    balancer
        .on_review_completed_or_closed(repo, "reviewer-a")
        .unwrap();
    let third_chosen = balancer.select_best_reviewers(repo, &[], &[], 1).unwrap();
    assert_eq!(third_chosen, vec!["reviewer-a".to_string()]);
}

#[test]
fn test_slash_commands_parser() {
    let body = r#"
    /assign @ferris
    /label +needs-review +area-compiler
    /unlabel wip
    /close not planned
    /move my-org/other-repo
    /retest
    /project 2 In Progress
    /duplicate #99
    "#;

    let cmds = CommandParser::parse_comment(body, &CommandsConfig::default());
    assert_eq!(cmds.len(), 8);
    assert!(cmds.contains(&SlashCommand::Assign(vec!["ferris".to_string()])));
    assert!(cmds.contains(&SlashCommand::Unlabel(vec!["wip".to_string()])));
    assert!(cmds.contains(&SlashCommand::Move {
        target_repo: "my-org/other-repo".to_string()
    }));
    assert!(cmds.contains(&SlashCommand::Duplicate(99)));
}

#[test]
fn test_bot_mention_commands_parser() {
    let body = r#"
    @starlabssisugie label +bug
    @starlabssisugie /assign @alice
    @sugie-bot help
    @starlabssisugie
    "#;
    let cmds = CommandParser::parse_comment(body, &CommandsConfig::default());
    assert_eq!(cmds.len(), 4);
    assert!(cmds.contains(&SlashCommand::Label {
        add: vec!["bug".to_string()],
        remove: vec![]
    }));
    assert!(cmds.contains(&SlashCommand::Assign(vec!["alice".to_string()])));
    assert!(cmds.contains(&SlashCommand::Help));
}

#[test]
fn test_restricted_commands_authorization() {
    let mut config = CommandsConfig::default();
    config.super_users = vec!["alice".to_string(), "bob".to_string()];
    config.restricted_commands = vec!["close".to_string(), "move".to_string()];

    let super_user = User {
        login: "alice".to_string(),
        id: 1,
        r#type: None,
    };

    let regular_user = User {
        login: "charlie".to_string(),
        id: 2,
        r#type: None,
    };

    let close_cmd = SlashCommand::Close(None);
    let label_cmd = SlashCommand::Label {
        add: vec!["bug".to_string()],
        remove: vec![],
    };

    let is_auth = |cmd: &SlashCommand, user: &User| -> bool {
        let is_restricted = config
            .restricted_commands
            .iter()
            .any(|r| r.eq_ignore_ascii_case(cmd.name()));
        if !is_restricted {
            return true;
        }
        config
            .super_users
            .iter()
            .any(|su| su.eq_ignore_ascii_case(&user.login))
    };

    assert!(is_auth(&close_cmd, &super_user));
    assert!(is_auth(&label_cmd, &super_user));
    assert!(is_auth(&label_cmd, &regular_user));
    assert!(!is_auth(&close_cmd, &regular_user));
}

#[test]
fn test_duplicate_detector() {
    let config = DuplicateConfig {
        enabled: true,
        threshold: 0.60,
        auto_close_threshold: Some(0.95),
        add_label: Some("possible-duplicate".to_string()),
        search_open_only: true,
        max_search_candidates: 100,
        algorithm: "combined".to_string(),
        comment_template: None,
    };
    let detector = DuplicateDetector::new(config);

    let existing_issue = Issue {
        id: 1,
        number: 10,
        title: "Compiler panic when parsing generic trait bounds".to_string(),
        body: Some("Running cargo check panics on syntax error in generic bounds".to_string()),
        state: "open".to_string(),
        user: User {
            login: "alice".to_string(),
            id: 1,
            r#type: None,
        },
        labels: vec![],
        assignees: vec![],
        created_at: Utc::now(),
        updated_at: Utc::now(),
        closed_at: None,
        pull_request: None,
        author_association: None,
        comments: None,
    };

    let matches = detector.find_duplicates(
        11,
        "Compiler panic when parsing generic trait bounds",
        Some("cargo check panics on syntax error in bounds"),
        &[existing_issue],
    );

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].issue_number, 10);
    assert!(matches[0].similarity > 0.70);

    let comment = detector.build_comment_body("bob", 11, &matches, false);
    assert!(comment.contains("Potential Duplicate Detected"));
    assert!(comment.contains("#10"));
}

#[test]
fn test_dco_validator() {
    let config = DcoConfig::default();
    let validator = DcoValidator::new(config);

    let commits = vec![
        CommitItem {
            sha: "1111111111".to_string(),
            commit: CommitDetails {
                author: None,
                committer: None,
                message: "feat: add feature\n\nSigned-off-by: Dev <dev@example.com>".to_string(),
            },
            author: None,
        },
        CommitItem {
            sha: "2222222222".to_string(),
            commit: CommitDetails {
                author: None,
                committer: None,
                message: "fix: fix bug\n\nSigned-off-by: Dev <dev@example.com>".to_string(),
            },
            author: None,
        },
    ];

    let result = validator.validate_commits(&commits, "Dev");
    assert!(result.is_valid);
    assert!(result.unsigned_commits.is_empty());
}

#[test]
fn test_metrics_report_formatting() {
    let notifier = NotificationDispatcher::new(&sugie::config::NotificationsConfig::default());
    let reporter = MetricsReporter::new(sugie::config::ReportsConfig::default(), notifier);

    let metrics = TriageMetrics {
        period_name: "Weekly".to_string(),
        start_time: Utc::now(),
        end_time: Utc::now(),
        repo: "rust-lang/cargo".to_string(),
        new_issues_count: 12,
        closed_issues_count: 10,
        issue_close_rate: 83.3,
        new_prs_count: 8,
        merged_prs_count: 7,
        closed_prs_count: 1,
        avg_first_response_hours: 2.5,
        active_contributors: vec![("ferris".to_string(), 5), ("rustacean".to_string(), 3)],
        lines_added: 450,
        lines_deleted: 120,
        total_commits: 22,
    };

    let md = reporter.format_markdown_report(&metrics);
    assert!(md.contains("# 📊 Weekly Report: rust-lang/cargo"));
    assert!(md.contains("83.3%"));
    assert!(md.contains("@ferris"));
    assert!(md.contains("+330"));
}

#[tokio::test]
async fn test_notification_event_routing() {
    let cfg = sugie::config::NotificationsConfig::default();
    let notifier = NotificationDispatcher::new(&cfg);

    // Tests dispatch without panicking even without real endpoints
    notifier
        .dispatch_event(
            SugieEventType::Assigned,
            "Assign test",
            "Assign description",
            None,
            NotificationLevel::Info,
            Vec::new(),
        )
        .await;

    notifier
        .dispatch_event(
            SugieEventType::WeeklyReport,
            "Weekly Report Test",
            "Report content",
            None,
            NotificationLevel::Info,
            Vec::new(),
        )
        .await;
}

#[tokio::test]
async fn test_inbound_zulip_chat_webhook() {
    let (state, storage) = create_test_state();
    storage.upsert_repository(100, "StarLabsSI/ensia", 12345).unwrap();

    let app = create_router(state);

    let zulip_payload = serde_json::json!({
        "token": "test-zulip-token",
        "data": "@**sugie-bot** status",
        "message": {
            "sender_email": "developer@starlabs.si",
            "sender_full_name": "Developer",
            "content": "@**sugie-bot** status"
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/api/chat/zulip")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&zulip_payload).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let content = body_json.get("content").and_then(|v| v.as_str()).unwrap();
    assert!(content.contains("Sugie Bot Status"));
    assert!(content.contains("Running"));
}

#[tokio::test]
async fn test_inbound_lark_url_verification_and_event() {
    let (state, _) = create_test_state();
    let app = create_router(state);

    // 1. URL Verification Challenge
    let challenge_payload = serde_json::json!({
        "type": "url_verification",
        "challenge": "my_unique_challenge_token_123",
        "token": "test-lark-token"
    });

    let req1 = Request::builder()
        .method("POST")
        .uri("/api/chat/lark")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&challenge_payload).unwrap()))
        .unwrap();

    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    let bytes1 = axum::body::to_bytes(resp1.into_body(), usize::MAX).await.unwrap();
    let json1: serde_json::Value = serde_json::from_slice(&bytes1).unwrap();
    assert_eq!(json1.get("challenge").and_then(|v| v.as_str()), Some("my_unique_challenge_token_123"));

    // 2. Lark Message Event
    let event_payload = serde_json::json!({
        "schema": "2.0",
        "header": {
            "event_type": "im.message.receive_v1",
            "token": "test-lark-token"
        },
        "event": {
            "sender": {
                "sender_id": {
                    "open_id": "ou_test_user"
                }
            },
            "message": {
                "chat_id": "oc_test_chat",
                "content": "{\"text\":\"@_user_1 help\"}",
                "message_type": "text"
            }
        }
    });

    let req2 = Request::builder()
        .method("POST")
        .uri("/api/chat/lark")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&event_payload).unwrap()))
        .unwrap();

    let resp2 = app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    let bytes2 = axum::body::to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
    let json2: serde_json::Value = serde_json::from_slice(&bytes2).unwrap();
    assert_eq!(json2.get("code").and_then(|v| v.as_i64()), Some(0));
}

#[tokio::test]
async fn test_inbound_lark_encrypted_callback() {
    let (state, _) = create_test_state();
    let app = create_router(state);

    let encrypt_key = "test-lark-encrypt-key-123456789";

    // 1. Encrypted URL Verification
    let plaintext = serde_json::json!({
        "type": "url_verification",
        "challenge": "challenge_abc_123",
        "token": "test-lark-token"
    })
    .to_string();
    let encrypted_challenge = sugie::server::chat::encrypt_lark_payload(encrypt_key, &plaintext).unwrap();
    let encrypted_wrapper = serde_json::json!({
        "encrypt": encrypted_challenge
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/chat/lark")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&encrypted_wrapper).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json.get("challenge").and_then(|v| v.as_str()), Some("challenge_abc_123"));
}

#[tokio::test]
async fn test_inbound_discord_ping_interaction() {
    let (state, _) = create_test_state();
    let app = create_router(state);

    let ping_payload = serde_json::json!({
        "type": 1
    });

    let request = Request::builder()
        .method("POST")
        .uri("/api/chat/discord")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&ping_payload).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body_json.get("type").and_then(|v| v.as_i64()), Some(1));
}

#[tokio::test]
async fn test_generic_chat_command_endpoint() {
    let (state, storage) = create_test_state();
    storage.upsert_repository(100, "StarLabsSI/ensia", 12345).unwrap();
    let app = create_router(state);

    let payload = serde_json::json!({
        "command": "repos",
        "sender": "cn-starlabs"
    });

    let request = Request::builder()
        .method("POST")
        .uri("/api/chat/command")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let resp_str = body_json.get("response").and_then(|v| v.as_str()).unwrap();
    assert!(resp_str.contains("StarLabsSI/ensia"));
}

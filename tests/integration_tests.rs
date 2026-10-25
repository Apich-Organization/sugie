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

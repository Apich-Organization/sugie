use clap::{Parser, Subcommand};
use sugie::config::{self, SugieConfig};
use sugie::github::auth::GitHubAuthManager;
use sugie::metrics::MetricsReporter;
use sugie::notifications::NotificationDispatcher;
use sugie::server::AppState;
use sugie::stale::StaleManager;
use sugie::storage::Storage;
use sugie::triage::TriageService;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(name = "sugie")]
#[command(about = "A powerful, full-featured GitHub triage bot for the Rust ecosystem", long_about = None)]
#[command(version)]
struct Cli {
    #[arg(short, long, default_value = "sugie.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the triage bot webhook server and background schedulers
    Serve {
        #[arg(long)]
        host: Option<String>,
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Run stale issue & PR maintenance sweep
    Stale {
        #[arg(long)]
        repo: Option<String>,
    },
    /// Generate repository triage report (weekly or monthly)
    Report {
        #[arg(long)]
        repo: String,
        #[arg(long, default_value_t = true)]
        weekly: bool,
        #[arg(long)]
        monthly: bool,
        #[arg(long, default_value_t = false)]
        publish: bool,
    },
    /// Initialize template configuration files
    Init {
        #[arg(short, long, default_value = ".")]
        output_dir: PathBuf,
    },
    /// Validate configuration and credentials
    Validate,
    /// Send test notifications to configured channels (Discord, Zulip, Lark)
    TestNotifications {
        /// Target channel: "discord", "zulip", "lark", or all
        #[arg(long)]
        channel: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sugie::config::load_dotenv();

    let subscriber = FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { host, port } => {
            info!("Starting Sugie Triage Bot...");
            let config = SugieConfig::load_from_env_or_file(&cli.config)?;
            sugie::security_log::init(&config.db_path);
            let private_key = config.github.get_private_key()?;

            let auth = Arc::new(GitHubAuthManager::new(config.github.app_id, private_key));
            let storage = Storage::new(&config.db_path)?;
            let notifier = NotificationDispatcher::new(&config.notifications);

            let triage = TriageService::new(
                Arc::new(config.clone()),
                storage.clone(),
                notifier.clone(),
                auth.clone(),
            );

            // Auto-sync repositories from GitHub App installations in background
            let sync_triage = triage.clone();
            tokio::spawn(async move {
                info!("Syncing repositories from active GitHub App installations...");
                match sync_triage.sync_all_repositories().await {
                    Ok(repos) => info!(
                        "Successfully synced {} repositories into database.",
                        repos.len()
                    ),
                    Err(e) => warn!(
                        "Auto-syncing repositories from installations skipped/failed: {:?}",
                        e
                    ),
                }
            });

            // 1. Configurable background task for stale issues maintenance
            let stale_triage = triage.clone();
            let stale_storage = storage.clone();
            let stale_rules = config.default_rules.stale.clone();
            let stale_notifier = notifier.clone();
            let stale_interval_secs = (stale_rules.check_interval_hours.max(1) as u64) * 3600;

            tokio::spawn(async move {
                let stale_mgr =
                    StaleManager::new(stale_rules, stale_storage.clone(), stale_notifier);
                let mut interval = tokio::time::interval(Duration::from_secs(stale_interval_secs));
                // Skip the immediate first tick on server startup
                interval.tick().await;

                loop {
                    interval.tick().await;
                    info!("Running periodic stale issue & PR sweep...");
                    if let Ok(repos) = stale_storage.list_repositories() {
                        for (_, full_name, inst_id) in repos {
                            if let Ok(client) =
                                stale_triage.get_client_for_installation(inst_id).await
                            {
                                let parts: Vec<&str> = full_name.split('/').collect();
                                if parts.len() == 2 {
                                    if let Err(e) = stale_mgr
                                        .process_repository(&client, parts[0], parts[1])
                                        .await
                                    {
                                        error!(
                                            "Error in stale processor for {}: {:?}",
                                            full_name, e
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            });

            // Start Axum HTTP server
            let listen_host = host.unwrap_or_else(|| config.host.clone());
            let listen_port = port.unwrap_or(config.port);
            let addr: SocketAddr = format!("{}:{}", listen_host, listen_port).parse()?;

            let config = Arc::new(config);
            let shutdown = Arc::new(tokio::sync::Notify::new());

            let web_state = sugie::web::WebState::new(
                config.clone(),
                triage.clone(),
                sugie::web::config_store::ConfigFiles::new(cli.config.clone(), ".env"),
                shutdown.clone(),
            )?;
            info!(
                "Admin console available at http://{}:{}/admin",
                listen_host, listen_port
            );

            // Shared secret for the generic chat-command endpoint. Empty or
            // unset keeps the historical open behaviour, with a warning.
            let chat_api_token: Option<Arc<str>> = std::env::var("SUGIE_CHAT_API_TOKEN")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .map(|v| Arc::from(v.as_str()));
            match chat_api_token.as_deref() {
                None => warn!(
                    "POST /api/chat/command is unauthenticated — anyone who can reach this port can run chat commands. Set SUGIE_CHAT_API_TOKEN to require a shared secret."
                ),
                Some(t) if t.chars().count() < 16 => warn!(
                    "SUGIE_CHAT_API_TOKEN is shorter than 16 characters; use a long random value (e.g. `openssl rand -hex 32`)."
                ),
                Some(_) => {}
            }

            // The platform webhooks authenticate with their own credentials, not
            // with SUGIE_CHAT_API_TOKEN. Each one silently stops verifying when
            // its secret is unset, so say so at boot rather than at incident time.
            if config.github.webhook_secret.trim().is_empty() {
                warn!(
                    "SUGIE_GITHUB_WEBHOOK_SECRET is not set — /webhook rejects every delivery \
                     (it cannot authenticate them). Set it to the same value as the GitHub App's webhook secret."
                );
            }
            if config
                .chat
                .zulip_token
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                warn!(
                    "SUGIE_ZULIP_OUTGOING_TOKEN is not set — POST /api/chat/zulip accepts unverified requests."
                );
            }
            if config
                .chat
                .lark_verification_token
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                warn!(
                    "SUGIE_LARK_VERIFICATION_TOKEN is not set — POST /api/chat/lark accepts unverified requests."
                );
            }
            if config
                .chat
                .lark_encrypt_key
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                info!(
                    "SUGIE_LARK_ENCRYPT_KEY is not set — encrypted Lark callbacks will be rejected; \
                     only configure this if the app uses an Encrypt Key."
                );
            }
            if config
                .chat
                .discord_public_key
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                warn!(
                    "SUGIE_DISCORD_PUBLIC_KEY is not set — POST /api/chat/discord accepts unverified requests."
                );
            }

            let state = AppState {
                config: config.clone(),
                triage,
                chat_api_token,
            };

            let app = sugie::server::create_router_with_web(state, Some(web_state));
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            info!("Sugie Webhook Server listening on http://{}", addr);

            // Connect info exposes the real peer address, so the unauthenticated
            // passkey throttle cannot be keyed on a caller-controlled header.
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal(shutdown))
            .await?;

            info!("Sugie Bot shut down gracefully.");
        }
        Commands::Stale { repo } => {
            let config = SugieConfig::load_from_env_or_file(&cli.config)?;
            let private_key = config.github.get_private_key()?;
            let auth = Arc::new(GitHubAuthManager::new(config.github.app_id, private_key));
            let storage = Storage::new(&config.db_path)?;
            let notifier = NotificationDispatcher::new(&config.notifications);
            let stale_mgr = StaleManager::new(
                config.default_rules.stale.clone(),
                storage.clone(),
                notifier,
            );

            let triage = TriageService::new(
                Arc::new(config),
                storage.clone(),
                NotificationDispatcher::new(&notifications_config_default()),
                auth.clone(),
            );

            let repos = match repo {
                Some(r) => {
                    let inst_id = storage.get_installation_id_for_repo(&r)?.unwrap_or(1);
                    vec![(0, r, inst_id)]
                }
                None => storage.list_repositories()?,
            };

            for (_, full_name, inst_id) in repos {
                let client = triage.get_client_for_installation(inst_id).await?;
                let parts: Vec<&str> = full_name.split('/').collect();
                if parts.len() == 2 {
                    info!("Running stale check on {}", full_name);
                    let report = stale_mgr
                        .process_repository(&client, parts[0], parts[1])
                        .await?;
                    println!(
                        "Repository {}: Warned {}, Closed {}, Locked {}",
                        full_name, report.warned_count, report.closed_count, report.locked_count
                    );
                }
            }
        }
        Commands::Report {
            repo,
            weekly: _,
            monthly,
            publish,
        } => {
            let config = SugieConfig::load_from_env_or_file(&cli.config)?;
            let private_key = config.github.get_private_key()?;
            let auth = Arc::new(GitHubAuthManager::new(config.github.app_id, private_key));
            let storage = Storage::new(&config.db_path)?;
            let notifier = NotificationDispatcher::new(&config.notifications);

            let triage = TriageService::new(
                Arc::new(config.clone()),
                storage.clone(),
                notifier.clone(),
                auth,
            );

            let client = triage.get_client_for_repo(&repo).await?;
            let parts: Vec<&str> = repo.split('/').collect();
            anyhow::ensure!(parts.len() == 2, "Repo must be in owner/repo format");

            let days = if monthly { 30 } else { 7 };
            let label = if monthly { "Monthly" } else { "Weekly" };

            let mut report_cfg = config.default_rules.reports.clone();
            if publish {
                report_cfg.publish_as_issue = true;
            }

            let reporter = MetricsReporter::new(report_cfg, notifier);
            let metrics_data = reporter
                .generate_report(&client, parts[0], parts[1], days, label)
                .await?;
            let md = reporter.format_markdown_report(&metrics_data);

            println!("{}", md);

            if publish {
                reporter
                    .publish_report(&client, parts[0], parts[1], &metrics_data)
                    .await?;
                println!("\n✅ Successfully published report to GitHub and notified channels.");
            }
        }
        Commands::Init { output_dir } => {
            init_template_files(&output_dir)?;
            println!("✅ Template files generated in {}", output_dir.display());
        }
        Commands::Validate => {
            let config = SugieConfig::load_from_env_or_file(&cli.config)?;
            let pk = config.github.get_private_key()?;
            let auth = GitHubAuthManager::new(config.github.app_id, pk);
            let _ = auth.generate_jwt()?;
            println!("✅ Local configuration file and RSA private key are valid!");

            println!(
                "🔄 Testing outbound connection to GitHub API (App ID: {})...",
                config.github.app_id
            );
            match auth.get_app_info().await {
                Ok(app) => {
                    let name = app
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let slug = app
                        .get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let owner = app
                        .get("owner")
                        .and_then(|o| o.get("login"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    println!("✅ Successfully authenticated with GitHub API as App '{}' (slug: {}, owner: {})", name, slug, owner);

                    match auth.get_installations().await {
                        Ok(installs) => {
                            if installs.is_empty() {
                                println!("ℹ️  App has 0 active installations. Install the GitHub App on your account/repository to begin receiving events.");
                            } else {
                                println!("📦 Found {} active installation(s):", installs.len());
                                let storage = Storage::new(&config.db_path).ok();
                                for inst in installs {
                                    let id = inst.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let acc = inst
                                        .get("account")
                                        .and_then(|a| a.get("login"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown");
                                    let target_type = inst
                                        .get("target_type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    println!(
                                        "   • Installation ID: {} (Target: {} [{}])",
                                        id, acc, target_type
                                    );

                                    match auth.get_installation_repositories(id).await {
                                        Ok(repos) => {
                                            if repos.is_empty() {
                                                println!("     └─ (No repositories selected or granted yet)");
                                            } else {
                                                println!("     └─ Repositories ({}) :", repos.len());
                                                for repo in repos {
                                                    let repo_id = repo.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                                                    let full_name = repo.get("full_name").and_then(|v| v.as_str()).unwrap_or("unknown");
                                                    let is_private = repo.get("private").and_then(|v| v.as_bool()).unwrap_or(false);
                                                    let visibility = if is_private { "private" } else { "public" };
                                                    let default_branch = repo.get("default_branch").and_then(|v| v.as_str()).unwrap_or("main");
                                                    println!("        - {} ({}, default branch: {})", full_name, visibility, default_branch);

                                                    if let Some(ref s) = storage {
                                                        let _ = s.upsert_repository(repo_id, full_name, id);
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => println!("     └─ ⚠️ Failed to fetch repositories for installation {}: {}", id, e),
                                    }
                                }
                                if storage.is_some() {
                                    println!(
                                        "💾 Synced active repositories to local database ({}).",
                                        config.db_path
                                    );
                                }
                            }
                        }
                        Err(e) => println!("⚠️  Could not retrieve installations list: {}", e),
                    }
                }
                Err(e) => {
                    println!("❌ Failed to authenticate with GitHub API: {}", e);
                    println!("💡 Verify that SUGIE_GITHUB_APP_ID matches your GitHub App and that the private key is active.");
                }
            }

            let format_repos = |channel_repos: Option<&Vec<String>>| -> String {
                if let Some(r) = channel_repos {
                    format!("(Repos: {})", r.join(", "))
                } else if let Some(ref g) = config.notifications.allowed_repositories {
                    format!("(Global filter: {})", g.join(", "))
                } else {
                    "(All repositories allowed)".to_string()
                }
            };

            println!("🔔 Notification Channels configured:");
            if let Some(ref d) = config.notifications.discord {
                println!(
                    "   • Discord: ✅ Configured {}",
                    format_repos(d.allowed_repositories.as_ref())
                );
            } else {
                println!("   • Discord: ⚪ Not configured");
            }

            if let Some(ref z) = config.notifications.zulip {
                println!(
                    "   • Zulip:   ✅ Configured {}",
                    format_repos(z.allowed_repositories.as_ref())
                );
            } else {
                println!("   • Zulip:   ⚪ Not configured");
            }

            if let Some(ref l) = config.notifications.lark {
                println!(
                    "   • Lark:    ✅ Configured {}",
                    format_repos(l.allowed_repositories.as_ref())
                );
            } else {
                println!("   • Lark:    ⚪ Not configured");
            }

            println!("👑 Command Permissions:");
            if !config.default_rules.commands.super_users.is_empty() {
                println!(
                    "   • Super Users:         @{}",
                    config.default_rules.commands.super_users.join(", @")
                );
            } else {
                println!("   • Super Users:         (None configured - anyone can run permitted commands)");
            }

            if !config.default_rules.commands.restricted_commands.is_empty() {
                println!(
                    "   • Restricted Commands: /{}",
                    config
                        .default_rules
                        .commands
                        .restricted_commands
                        .join(", /")
                );
            } else {
                println!("   • Restricted Commands: (None - all commands are open)");
            }

            println!("💡 Run `cargo run -- test-notifications` to verify delivery to all configured channels.");
        }
        Commands::TestNotifications { channel } => {
            let config = SugieConfig::load_from_env_or_file(&cli.config)?;
            println!("🔔 Testing notification channels...");

            let target = channel.as_deref().unwrap_or("all").to_lowercase();
            let mut any_tested = false;

            // 1. Discord
            if target == "all" || target == "discord" {
                if let Some(ref discord_cfg) = config.notifications.discord {
                    any_tested = true;
                    println!("📡 Testing Discord webhook...");
                    let notifier = sugie::notifications::DiscordNotifier::new(discord_cfg.clone());
                    let fields = vec![
                        sugie::notifications::DiscordEmbedField {
                            name: "Channel".to_string(),
                            value: "Discord Webhook".to_string(),
                            inline: true,
                        },
                        sugie::notifications::DiscordEmbedField {
                            name: "Bot Version".to_string(),
                            value: env!("CARGO_PKG_VERSION").to_string(),
                            inline: true,
                        },
                    ];
                    match notifier
                        .send_embed(
                            "🦀 Sugie Bot: Notification Test",
                            "This is a test notification from Sugie Triage Bot verifying connectivity.",
                            Some("https://github.com/cn-starlabs"),
                            0x2ecc71,
                            fields,
                            false,
                        )
                        .await
                    {
                        Ok(_) => println!("   ✅ Discord notification sent successfully!"),
                        Err(e) => println!("   ❌ Discord notification failed: {:?}", e),
                    }
                } else if target == "discord" {
                    println!(
                        "   ⚠️ Discord is not configured (missing SUGIE_DISCORD_WEBHOOK_URL)."
                    );
                }
            }

            // 2. Zulip
            if target == "all" || target == "zulip" {
                if let Some(ref zulip_cfg) = config.notifications.zulip {
                    any_tested = true;
                    println!(
                        "📡 Testing Zulip stream '{}' on {}...",
                        zulip_cfg.stream, zulip_cfg.site_url
                    );
                    let notifier = sugie::notifications::ZulipNotifier::new(zulip_cfg.clone());
                    let mut test_msg = "### 🦀 Sugie Bot: Notification Test\n\nThis is a test notification from Sugie Triage Bot verifying connectivity.".to_string();
                    test_msg.push_str("\n\n* **Channel**: Zulip Stream\n* **Bot Version**: ");
                    test_msg.push_str(env!("CARGO_PKG_VERSION"));
                    match notifier
                        .send_message(sugie::notifications::ZulipTopicCategory::Alerts, &test_msg)
                        .await
                    {
                        Ok(_) => println!("   ✅ Zulip notification sent successfully!"),
                        Err(e) => println!("   ❌ Zulip notification failed: {:?}", e),
                    }
                } else if target == "zulip" {
                    println!("   ⚠️ Zulip is not configured (missing SUGIE_ZULIP_SITE_URL, SUGIE_ZULIP_BOT_EMAIL, or SUGIE_ZULIP_BOT_API_KEY).");
                }
            }

            // 3. Lark
            if target == "all" || target == "lark" {
                if let Some(ref lark_cfg) = config.notifications.lark {
                    any_tested = true;
                    println!(
                        "📡 Testing Lark bot webhook (signing: {})...",
                        if lark_cfg.secret.is_some() {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    );
                    let notifier = sugie::notifications::LarkNotifier::new(lark_cfg.clone());
                    let fields = [
                        ("Channel", "Lark Custom Bot"),
                        ("Bot Version", env!("CARGO_PKG_VERSION")),
                    ];
                    match notifier
                        .send_notification(
                            "🦀 Sugie Bot: Notification Test",
                            "This is a test notification from Sugie Triage Bot verifying Lark webhook connectivity and signature verification.",
                            Some("https://github.com/cn-starlabs"),
                            "green",
                            &fields,
                        )
                        .await
                    {
                        Ok(_) => println!("   ✅ Lark notification sent successfully!"),
                        Err(e) => println!("   ❌ Lark notification failed: {:?}", e),
                    }
                } else if target == "lark" {
                    println!("   ⚠️ Lark is not configured (missing SUGIE_LARK_WEBHOOK_URL).");
                }
            }

            if !any_tested {
                println!("   ⚠️ No notification channels were configured to test.");
            }
        }
    }

    Ok(())
}

fn notifications_config_default() -> config::NotificationsConfig {
    config::NotificationsConfig::default()
}

async fn shutdown_signal(notify: Arc<tokio::sync::Notify>) {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = notify.notified() => {
            info!("Restart requested from the admin console, shutting down...");
        }
    }
    info!("Termination signal received, shutting down server...");
}

fn init_template_files(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;

    let sugie_toml = include_str!("../templates/sugie.toml");
    std::fs::write(dir.join("sugie.toml"), sugie_toml)?;

    let env_example = include_str!("../templates/.env.example");
    std::fs::write(dir.join(".env.example"), env_example)?;

    let github_dir = dir.join(".github");
    std::fs::create_dir_all(&github_dir)?;
    let repo_sugie_toml = include_str!("../templates/repo-sugie.toml");
    std::fs::write(github_dir.join("sugie.toml"), repo_sugie_toml)?;

    let codeowners_example = include_str!("../templates/CODEOWNERS.example");
    std::fs::write(github_dir.join("CODEOWNERS"), codeowners_example)?;

    Ok(())
}

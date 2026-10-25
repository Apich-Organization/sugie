use clap::{Parser, Subcommand};
use sugie::config::{self, SugieConfig};
use sugie::github::auth::GitHubAuthManager;
use sugie::metrics::MetricsReporter;
use sugie::notifications::NotificationDispatcher;
use sugie::server::{create_router, AppState};
use sugie::stale::StaleManager;
use sugie::storage::Storage;
use sugie::triage::TriageService;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, Level};
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { host, port } => {
            info!("Starting Sugie Triage Bot...");
            let config = SugieConfig::load_from_env_or_file(&cli.config)?;
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

            // 2. Configurable background scheduler for Weekly & Monthly Reports
            let report_triage = triage.clone();
            let report_storage = storage.clone();
            let report_cfg = config.default_rules.reports.clone();
            let report_notifier = notifier.clone();

            tokio::spawn(async move {
                let weekly_interval_secs = (report_cfg.weekly_interval_days.max(1) as u64) * 86400;
                let mut weekly_timer =
                    tokio::time::interval(Duration::from_secs(weekly_interval_secs));
                let reporter = MetricsReporter::new(report_cfg.clone(), report_notifier.clone());

                loop {
                    weekly_timer.tick().await;
                    if !report_cfg.weekly_enabled {
                        continue;
                    }
                    info!("Running scheduled weekly triage report generator...");
                    if let Ok(repos) = report_storage.list_repositories() {
                        for (_, full_name, inst_id) in repos {
                            if let Ok(client) =
                                report_triage.get_client_for_installation(inst_id).await
                            {
                                let parts: Vec<&str> = full_name.split('/').collect();
                                if parts.len() == 2 {
                                    match reporter
                                        .generate_report(
                                            &client,
                                            parts[0],
                                            parts[1],
                                            report_cfg.weekly_interval_days as i64,
                                            "Weekly",
                                        )
                                        .await
                                    {
                                        Ok(metrics_data) => {
                                            if let Err(e) = reporter
                                                .publish_report(
                                                    &client,
                                                    parts[0],
                                                    parts[1],
                                                    &metrics_data,
                                                )
                                                .await
                                            {
                                                error!(
                                                    "Failed to publish weekly report for {}: {:?}",
                                                    full_name, e
                                                );
                                            }
                                        }
                                        Err(e) => error!(
                                            "Failed to generate weekly report for {}: {:?}",
                                            full_name, e
                                        ),
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

            let state = AppState {
                config: Arc::new(config),
                triage,
            };

            let app = create_router(state);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            info!("Sugie Webhook Server listening on http://{}", addr);

            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
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
            let _ = GitHubAuthManager::new(config.github.app_id, pk).generate_jwt()?;
            println!("✅ Configuration file and GitHub App RSA private key are valid!");
        }
    }

    Ok(())
}

fn notifications_config_default() -> config::NotificationsConfig {
    config::NotificationsConfig::default()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
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

mod api;
mod cli;
mod config;
mod db;
mod models;
mod runner;
mod systemd;

use std::sync::Arc;

use clap::Parser;
use cli::{Cli, Commands};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon => {
            // Load config
            let config = config::Config::load()?;

            // Create database and run migrations
            let database = db::Database::new(&config.db_path).await?;
            database.run_migrations().await?;

            // Mark orphaned runs as crashed on startup
            let conn = database.connect().await?;
            let orphaned = db::runs::mark_orphaned_runs_crashed(&conn).await?;
            if orphaned > 0 {
                info!("Marked {} orphaned run(s) as crashed on startup", orphaned);
            }

            // Create systemd manager
            let systemd = systemd::Systemctl::new(&config)?;

            // Create app state
            let state = api::AppState {
                db: Arc::new(database),
                systemd: Arc::new(systemd),
                config: Arc::new(config.clone()),
            };

            // Build router
            let app = api::router(state);

            // Start server
            let addr = format!("{}:{}", config.host, config.port);
            info!("Starting cron-rs daemon on {}", addr);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }
        Commands::Init => {
            cli::init::run_init().await?;
        }
        Commands::Task { command } => {
            cli::task::handle_task_command(command).await?;
        }
        Commands::Hook { command } => {
            cli::hook::handle_hook_command(command).await?;
        }
        Commands::Runs { command } => {
            cli::runs::handle_runs_command(command).await?;
        }
        Commands::Status => {
            let config = config::Config::load()?;
            cli::task::show_status(&config).await?;
        }
        Commands::Doctor => {
            cli::doctor::run_doctor().await?;
        }
        Commands::Regenerate => {
            cli::doctor::run_regenerate().await?;
        }
        Commands::Run {
            task_id,
            task_name,
            db_path,
        } => {
            let exit_code = runner::run_task(&task_id, &task_name, &db_path).await?;
            std::process::exit(exit_code);
        }
    }

    Ok(())
}

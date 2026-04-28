pub mod doctor;
pub mod hook;
pub mod import;
pub mod init;
pub mod runs;
pub mod service;
pub mod task;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "cron-rs", version, about = "Systemd timer management platform")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Start the daemon (API server)
    Daemon {
        /// Bind address for this daemon process
        #[arg(long)]
        host: Option<String>,
        /// Bind port for this daemon process
        #[arg(long)]
        port: Option<u16>,
    },

    /// Interactive first-time setup
    Init {
        /// Admin username to write to config
        #[arg(long)]
        username: Option<String>,
        /// Admin password to hash and write to config
        #[arg(long)]
        password: Option<String>,
        /// API bind address to write to config, e.g. 0.0.0.0 for SSH-hosted use
        #[arg(long)]
        host: Option<String>,
        /// API bind port to write to config
        #[arg(long)]
        port: Option<u16>,
        /// Config directory to write, defaults to ~/cron-rs
        #[arg(long)]
        config_dir: Option<String>,
        /// Database path to write, defaults to <config-dir>/cron-rs.db
        #[arg(long)]
        db_path: Option<String>,
    },

    /// Import existing crontab and systemd timer entries into cron-rs
    Import {
        /// Source to import from
        #[arg(long, value_enum, default_value_t = ImportSource::All)]
        source: ImportSource,
        /// Also inspect system-wide systemd timers (`systemctl list-unit-files --type=timer`)
        #[arg(long)]
        include_system: bool,
        /// Print what would be imported without changing the database
        #[arg(long)]
        dry_run: bool,
        /// Enable imported tasks and install cron-rs timers immediately
        #[arg(long)]
        enable: bool,
    },

    /// Manage the cron-rs daemon user systemd service
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },

    /// Manage tasks
    Task {
        #[command(subcommand)]
        command: TaskCommands,
    },

    /// Manage hooks
    Hook {
        #[command(subcommand)]
        command: HookCommands,
    },

    /// View job runs
    Runs {
        #[command(subcommand)]
        command: RunsCommands,
    },

    /// Show status of all tasks
    Status,

    /// Diagnose common issues
    Doctor,

    /// Regenerate systemd units from DB
    Regenerate,

    /// Internal: run a task (called by systemd service units)
    Run {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        task_name: String,
        #[arg(long)]
        db_path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImportSource {
    All,
    Crontab,
    Systemd,
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommands {
    /// Install or update the user systemd service
    Install {
        /// Bind address for the daemon service
        #[arg(long)]
        host: Option<String>,
        /// Bind port for the daemon service
        #[arg(long)]
        port: Option<u16>,
        /// Start or restart the service after installing it
        #[arg(long)]
        start: bool,
    },
    /// Remove the user systemd service
    Uninstall,
    /// Start the user systemd service
    Start,
    /// Stop the user systemd service
    Stop,
    /// Restart the user systemd service
    Restart,
    /// Show the user systemd service status
    Status,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommands {
    /// List all tasks
    List,
    /// Create a new task
    Create {
        /// Task name
        name: String,
        #[arg(long)]
        command: String,
        #[arg(long)]
        schedule: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        max_retries: Option<i32>,
        #[arg(long)]
        retry_delay_secs: Option<i32>,
        #[arg(long)]
        timeout_secs: Option<i32>,
        #[arg(long)]
        concurrency_policy: Option<String>,
    },
    /// Show a task by name or id
    Show { name_or_id: String },
    /// Edit a task
    Edit {
        name_or_id: String,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        schedule: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        max_retries: Option<i32>,
        #[arg(long)]
        retry_delay_secs: Option<i32>,
        #[arg(long)]
        timeout_secs: Option<i32>,
        #[arg(long)]
        concurrency_policy: Option<String>,
    },
    /// Delete a task
    Delete { name_or_id: String },
    /// Enable a task
    Enable { name_or_id: String },
    /// Disable a task
    Disable { name_or_id: String },
    /// Trigger a task to run immediately
    Trigger { name_or_id: String },
}

#[derive(Debug, Subcommand)]
pub enum HookCommands {
    /// Add a hook to a task
    Add {
        /// Task name or id
        task: String,
        /// Hook type: on_failure, on_success, on_retry_exhausted
        #[arg(long, value_name = "TYPE")]
        on: String,
        /// Command to execute
        #[arg(long)]
        command: String,
        #[arg(long)]
        timeout_secs: Option<i32>,
        #[arg(long)]
        run_order: Option<i32>,
    },
    /// List hooks for a task
    List {
        /// Task name or id
        task: String,
    },
    /// Remove a hook
    Remove {
        /// Hook id
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RunsCommands {
    /// List recent runs
    List {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value = "20")]
        limit: i64,
    },
    /// Show details of a specific run
    Show { id: String },
}

use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::unit_gen;
use super::SystemdManager;
use crate::config::Config;
use crate::models::Task;

/// Real implementation of SystemdManager that calls `systemctl --user`.
pub struct Systemctl {
    /// Directory where unit files are written (e.g. ~/.config/systemd/user/).
    pub unit_dir: PathBuf,
    /// Resolved path to the cron-rs binary.
    pub binary_path: PathBuf,
    /// Database path string, passed into generated service units.
    pub db_path: String,
}

impl Systemctl {
    /// Create a new Systemctl instance from the application config.
    ///
    /// - Creates the unit_dir (~/.config/systemd/user/) if it doesn't exist.
    /// - Resolves binary_path via std::env::current_exe().
    /// - Verifies that `systemctl` is available in PATH (warns if missing).
    pub fn new(config: &Config) -> Result<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/root"));
        let unit_dir = PathBuf::from(&home)
            .join(".config")
            .join("systemd")
            .join("user");

        // Ensure the unit directory exists.
        std::fs::create_dir_all(&unit_dir).with_context(|| {
            format!(
                "failed to create systemd user unit directory: {}",
                unit_dir.display()
            )
        })?;

        // Resolve the current binary path.
        let binary_path =
            std::env::current_exe().context("failed to resolve current executable path")?;

        // Extract db_path as a string from config.
        let db_path = config.db_path.to_string_lossy().to_string();

        // Check that systemctl is available in PATH.
        match std::process::Command::new("which")
            .arg("systemctl")
            .output()
        {
            Ok(output) if output.status.success() => {
                debug!("systemctl found in PATH");
            }
            _ => {
                warn!("systemctl not found in PATH; systemd operations will fail");
            }
        }

        Ok(Self {
            unit_dir,
            binary_path,
            db_path,
        })
    }

    /// Run a `systemctl --user` command with the given arguments.
    /// Returns the command output. Logs the command and its stdout/stderr.
    /// Returns an error if the command exits with a non-zero status.
    async fn run_systemctl(&self, args: &[&str]) -> Result<std::process::Output> {
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user");
        for arg in args {
            cmd.arg(arg);
        }

        info!("running: systemctl --user {}", args.join(" "));

        let output = cmd.output().await.context("failed to execute systemctl")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !stdout.is_empty() {
            debug!("systemctl stdout: {}", stdout.trim());
        }
        if !stderr.is_empty() {
            debug!("systemctl stderr: {}", stderr.trim());
        }

        if !output.status.success() {
            anyhow::bail!(
                "systemctl --user {} failed with status {}: {}",
                args.join(" "),
                output.status,
                stderr.trim()
            );
        }

        Ok(output)
    }

    /// Run a `systemctl --user` command, but do not fail if the exit code is non-zero.
    /// Returns the output regardless.
    async fn run_systemctl_no_fail(&self, args: &[&str]) -> Result<std::process::Output> {
        let mut cmd = Command::new("systemctl");
        cmd.arg("--user");
        for arg in args {
            cmd.arg(arg);
        }

        info!("running: systemctl --user {}", args.join(" "));

        let output = cmd.output().await.context("failed to execute systemctl")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !stdout.is_empty() {
            debug!("systemctl stdout: {}", stdout.trim());
        }
        if !stderr.is_empty() {
            debug!("systemctl stderr: {}", stderr.trim());
        }

        Ok(output)
    }
}

#[async_trait::async_trait]
impl SystemdManager for Systemctl {
    async fn install_task(&self, task: &Task) -> Result<()> {
        if task.lock_key.is_some() || task.sandbox_profile.is_some() {
            unit_gen::ensure_lock_dir().context("failed to prepare cron-rs lock directory")?;
        }

        // Generate unit file contents.
        let timer_content = unit_gen::generate_timer(&task.name, &task.schedule);
        let service_content = unit_gen::generate_service_for_task(
            task,
            &self.binary_path.to_string_lossy(),
            &self.db_path,
        );

        // Write unit files.
        let timer_path = self.unit_dir.join(unit_gen::timer_filename(&task.name));
        let service_path = self.unit_dir.join(unit_gen::service_filename(&task.name));

        tokio::fs::write(&timer_path, &timer_content)
            .await
            .with_context(|| {
                format!("failed to write timer unit file: {}", timer_path.display())
            })?;
        info!("wrote timer unit: {}", timer_path.display());

        tokio::fs::write(&service_path, &service_content)
            .await
            .with_context(|| {
                format!(
                    "failed to write service unit file: {}",
                    service_path.display()
                )
            })?;
        info!("wrote service unit: {}", service_path.display());

        // Reload daemon to pick up new files, then enable and start the timer.
        self.daemon_reload().await?;
        self.enable_timer(&task.name).await?;
        self.start_timer(&task.name).await?;

        info!("installed and started timer for task '{}'", task.name);
        Ok(())
    }

    async fn remove_task(&self, task_name: &str) -> Result<()> {
        // Stop and disable the timer first (ignore errors if already stopped/disabled).
        self.stop_timer(task_name).await.ok();
        self.disable_timer(task_name).await.ok();

        // Delete the unit files.
        let timer_path = self.unit_dir.join(unit_gen::timer_filename(task_name));
        let service_path = self.unit_dir.join(unit_gen::service_filename(task_name));

        if timer_path.exists() {
            tokio::fs::remove_file(&timer_path).await.with_context(|| {
                format!("failed to remove timer unit: {}", timer_path.display())
            })?;
            info!("removed timer unit: {}", timer_path.display());
        }

        if service_path.exists() {
            tokio::fs::remove_file(&service_path)
                .await
                .with_context(|| {
                    format!("failed to remove service unit: {}", service_path.display())
                })?;
            info!("removed service unit: {}", service_path.display());
        }

        // Reload daemon to reflect removed files.
        self.daemon_reload().await?;

        info!("removed all units for task '{}'", task_name);
        Ok(())
    }

    async fn enable_timer(&self, task_name: &str) -> Result<()> {
        let timer = unit_gen::timer_filename(task_name);
        self.run_systemctl(&["enable", &timer]).await?;
        Ok(())
    }

    async fn disable_timer(&self, task_name: &str) -> Result<()> {
        let timer = unit_gen::timer_filename(task_name);
        self.run_systemctl(&["disable", &timer]).await?;
        Ok(())
    }

    async fn start_timer(&self, task_name: &str) -> Result<()> {
        let timer = unit_gen::timer_filename(task_name);
        self.run_systemctl(&["start", &timer]).await?;
        Ok(())
    }

    async fn stop_timer(&self, task_name: &str) -> Result<()> {
        let timer = unit_gen::timer_filename(task_name);
        self.run_systemctl(&["stop", &timer]).await?;
        Ok(())
    }

    async fn daemon_reload(&self) -> Result<()> {
        self.run_systemctl(&["daemon-reload"]).await?;
        Ok(())
    }

    async fn is_timer_active(&self, task_name: &str) -> Result<bool> {
        let timer = unit_gen::timer_filename(task_name);
        let output = self.run_systemctl_no_fail(&["is-active", &timer]).await?;
        Ok(output.status.success())
    }
}

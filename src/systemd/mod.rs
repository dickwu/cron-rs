pub mod ctl;
pub mod unit_gen;

pub use ctl::Systemctl;

use crate::models::Task;

/// Trait for systemd operations. Allows mocking in tests.
#[async_trait::async_trait]
pub trait SystemdManager: Send + Sync {
    /// Install a task: generate timer + service files, reload daemon, enable & start timer.
    async fn install_task(&self, task: &Task) -> anyhow::Result<()>;
    /// Remove a task: stop & disable timer, delete unit files, reload daemon.
    async fn remove_task(&self, task_name: &str) -> anyhow::Result<()>;
    /// Enable the timer unit for the given task.
    async fn enable_timer(&self, task_name: &str) -> anyhow::Result<()>;
    /// Disable the timer unit for the given task.
    async fn disable_timer(&self, task_name: &str) -> anyhow::Result<()>;
    /// Start the timer unit for the given task.
    async fn start_timer(&self, task_name: &str) -> anyhow::Result<()>;
    /// Stop the timer unit for the given task.
    async fn stop_timer(&self, task_name: &str) -> anyhow::Result<()>;
    /// Reload the systemd daemon to pick up unit file changes.
    async fn daemon_reload(&self) -> anyhow::Result<()>;
    /// Check whether the timer for the given task is currently active.
    async fn is_timer_active(&self, task_name: &str) -> anyhow::Result<bool>;
}

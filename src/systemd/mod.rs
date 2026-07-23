pub mod ctl;
pub mod unit_gen;

pub use ctl::Systemctl;

use crate::models::Task;

/// Trait for systemd operations. Allows mocking in tests.
#[async_trait::async_trait]
pub trait SystemdManager: Send + Sync {
    /// Install a task: generate timer + service files, reload daemon, enable &
    /// (re)start timer. `stagger_second` (see [`unit_gen::stagger_assignments`])
    /// shifts an every-minute schedule onto its own second so such tasks never
    /// fire simultaneously.
    async fn install_task(&self, task: &Task, stagger_second: Option<u8>) -> anyhow::Result<()>;
    /// Remove a task: stop & disable timer, delete unit files, reload daemon.
    async fn remove_task(&self, task_name: &str) -> anyhow::Result<()>;
    /// Enable the timer unit for the given task.
    async fn enable_timer(&self, task_name: &str) -> anyhow::Result<()>;
    /// Disable the timer unit for the given task.
    async fn disable_timer(&self, task_name: &str) -> anyhow::Result<()>;
    /// Stop the timer unit for the given task.
    async fn stop_timer(&self, task_name: &str) -> anyhow::Result<()>;
    /// Reload the systemd daemon to pick up unit file changes.
    async fn daemon_reload(&self) -> anyhow::Result<()>;
    /// Check whether the oneshot service for the given task is currently
    /// active, i.e. a run is executing right now.
    async fn is_service_active(&self, task_name: &str) -> anyhow::Result<bool>;
    /// Unit names (`cron-rs-<task>.timer`) of all currently active cron-rs
    /// timers, fetched with a single systemctl invocation.
    async fn active_timer_names(&self) -> anyhow::Result<std::collections::HashSet<String>>;
    /// Validate an OnCalendar expression as it will be rendered into a timer
    /// unit (including any `CRON_RS_TIMEZONE` suffix). Err carries a
    /// human-readable reason when systemd would reject the expression.
    async fn validate_calendar(&self, schedule: &str) -> anyhow::Result<()>;
}

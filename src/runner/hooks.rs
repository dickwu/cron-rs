use crate::db;
use crate::db::helpers::now_timestamp;
use crate::models::{Hook, HookRun, HookRunStatus};
use tracing::{info, warn};

use super::executor;

/// Execute all hooks of the given type for a job run, in order.
/// Each hook runs regardless of whether previous hooks failed.
/// Returns a list of HookRun records for what was executed.
pub async fn execute_hooks(
    hooks: &[Hook],
    job_run_id: &str,
    db_path: &str,
) -> anyhow::Result<Vec<HookRun>> {
    if hooks.is_empty() {
        return Ok(Vec::new());
    }

    let db = db::Database::new(std::path::Path::new(db_path)).await?;
    let conn = db.connect().await?;

    let mut results = Vec::new();

    for hook in hooks {
        info!(
            hook_id = %hook.id,
            hook_type = %hook.hook_type,
            "Executing hook: {}",
            hook.command
        );

        // Create a hook_run record with initial status
        let hook_run = HookRun {
            id: String::new(),
            job_run_id: job_run_id.to_string(),
            hook_id: hook.id.clone(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            started_at: String::new(),
            finished_at: None,
            status: HookRunStatus::Success, // Placeholder; will be updated
        };

        let created_run = match db::runs::create_hook_run(&conn, &hook_run).await {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to create hook_run record: {}", e);
                continue;
            }
        };

        // Execute the hook command
        let result = executor::execute_command(&hook.command, hook.timeout_secs).await;

        let (status, exit_code, stdout, stderr) = match result {
            Ok(cmd_result) => {
                if cmd_result.timed_out {
                    (
                        HookRunStatus::Timeout,
                        Some(cmd_result.exit_code),
                        cmd_result.stdout,
                        cmd_result.stderr,
                    )
                } else if cmd_result.exit_code == 0 {
                    (
                        HookRunStatus::Success,
                        Some(cmd_result.exit_code),
                        cmd_result.stdout,
                        cmd_result.stderr,
                    )
                } else {
                    (
                        HookRunStatus::Failed,
                        Some(cmd_result.exit_code),
                        cmd_result.stdout,
                        cmd_result.stderr,
                    )
                }
            }
            Err(e) => {
                warn!("Hook execution error: {}", e);
                (
                    HookRunStatus::Failed,
                    None,
                    String::new(),
                    format!("Hook execution error: {}", e),
                )
            }
        };

        info!(
            hook_id = %hook.id,
            status = %status,
            "Hook completed"
        );

        // Update the hook_run record
        let updated_run = HookRun {
            id: created_run.id.clone(),
            job_run_id: created_run.job_run_id.clone(),
            hook_id: created_run.hook_id.clone(),
            exit_code,
            stdout,
            stderr,
            started_at: created_run.started_at.clone(),
            finished_at: Some(now_timestamp()),
            status: status.clone(),
        };

        if let Err(e) = db::runs::update_hook_run(&conn, &updated_run).await {
            warn!("Failed to update hook_run record: {}", e);
        }

        results.push(updated_run);

        // Continue to next hook regardless of failure
        if status != HookRunStatus::Success {
            warn!(
                hook_id = %hook.id,
                "Hook failed or timed out, continuing to next hook"
            );
        }
    }

    Ok(results)
}

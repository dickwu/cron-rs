pub mod executor;
pub mod hooks;
pub mod lock;
pub mod retry;

use std::path::Path;

use tracing::{error, info, warn};

use crate::db;
use crate::db::helpers::now_timestamp;
use crate::models::{ConcurrencyPolicy, HookType, JobRun, JobRunStatus};

/// Run a task by ID. This is the entry point called by the systemd service unit.
/// Returns the final exit code (0 for success, 1 for failure/timeout).
pub async fn run_task(task_id: &str, task_name: &str, db_path: &str) -> anyhow::Result<i32> {
    info!(task_id = %task_id, task_name = %task_name, "Starting task runner");

    // 1. Open DB connection
    let database = db::Database::new(Path::new(db_path)).await?;
    let conn = database.connect().await?;

    // 2. Read task config from DB
    let task = match db::tasks::get_by_id(&conn, task_id).await {
        Ok(t) => t,
        Err(db::helpers::DbError::NotFound) => {
            error!(
                task_id = %task_id,
                task_name = %task_name,
                "Task '{}' not found in database",
                task_name
            );
            return Err(anyhow::anyhow!(
                "Task '{}' (id: {}) not found in database",
                task_name,
                task_id
            ));
        }
        Err(e) => {
            error!(
                task_id = %task_id,
                task_name = %task_name,
                "Failed to read task config: {}",
                e
            );
            return Err(e.into());
        }
    };

    // Derive lock directory from db_path's parent directory
    let lock_dir = Path::new(db_path)
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .join("locks");

    // 3. Check concurrency policy
    let _lock_guard;
    match task.concurrency_policy {
        ConcurrencyPolicy::Skip => {
            match lock::try_acquire_lock(&lock_dir, task_id)? {
                Some(guard) => {
                    _lock_guard = Some(guard);
                    info!(
                        task_name = %task_name,
                        "Lock acquired for task (skip policy)"
                    );
                }
                None => {
                    // Another instance is running; skip this execution
                    info!(
                        task_name = %task_name,
                        "Task already running, skipping (skip policy)"
                    );

                    // Insert a skipped job_run record
                    let skipped_run = JobRun {
                        id: String::new(),
                        task_id: task_id.to_string(),
                        started_at: String::new(),
                        finished_at: Some(now_timestamp()),
                        exit_code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        status: JobRunStatus::Skipped,
                        attempt: 1,
                        duration_ms: Some(0),
                    };
                    if let Err(e) = db::runs::create_job_run(&conn, &skipped_run).await {
                        warn!(
                            task_name = %task_name,
                            "Failed to record skipped run: {}",
                            e
                        );
                    }
                    return Ok(0);
                }
            }
        }
        ConcurrencyPolicy::Allow => {
            // Try to get the lock, but proceed even without it
            match lock::try_acquire_lock(&lock_dir, task_id) {
                Ok(Some(guard)) => {
                    _lock_guard = Some(guard);
                }
                _ => {
                    _lock_guard = None;
                    info!(
                        task_name = %task_name,
                        "Proceeding without exclusive lock (allow policy)"
                    );
                }
            }
        }
        ConcurrencyPolicy::Queue => {
            // Blocking acquire — wait until the lock is available
            let guard = lock::acquire_lock(&lock_dir, task_id)?;
            _lock_guard = Some(guard);
            info!(
                task_name = %task_name,
                "Lock acquired for task (queue policy)"
            );
        }
    }

    // 4. Insert job_run record with status='running'
    let mut job_run = JobRun {
        id: String::new(),
        task_id: task_id.to_string(),
        started_at: String::new(),
        finished_at: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        status: JobRunStatus::Running,
        attempt: 1,
        duration_ms: None,
    };

    job_run = match db::runs::create_job_run(&conn, &job_run).await {
        Ok(r) => r,
        Err(e) => {
            error!(
                task_name = %task_name,
                "Failed to create job_run record: {}",
                e
            );
            return Err(e.into());
        }
    };

    info!(
        task_name = %task_name,
        job_run_id = %job_run.id,
        "Job run created, executing command: {}",
        task.command
    );

    // 5-8. Execute with retry loop
    let mut attempt = 1;
    loop {
        info!(
            task_name = %task_name,
            attempt = attempt,
            "Executing command (attempt {})",
            attempt
        );

        let result = executor::execute_command(&task.command, task.timeout_secs).await;

        match result {
            Ok(cmd_result) => {
                // Update the run with output regardless of outcome
                job_run.stdout = cmd_result.stdout;
                job_run.stderr = cmd_result.stderr;
                job_run.exit_code = Some(cmd_result.exit_code);
                job_run.duration_ms = Some(cmd_result.duration_ms);
                job_run.attempt = attempt;

                if cmd_result.timed_out {
                    // 6. Timeout
                    warn!(
                        task_name = %task_name,
                        "Command timed out after attempt {}",
                        attempt
                    );

                    job_run.status = JobRunStatus::Timeout;
                    job_run.finished_at = Some(now_timestamp());

                    if let Err(e) = db::runs::update_job_run(&conn, &job_run).await {
                        error!(task_name = %task_name, "Failed to update job_run: {}", e);
                    }

                    // Run on_failure hooks
                    run_hooks_by_type(&conn, &task.id, &job_run.id, HookType::Failure, db_path, task_name).await;

                    return Ok(1);
                } else if cmd_result.exit_code == 0 {
                    // 7. Success
                    info!(
                        task_name = %task_name,
                        "Command succeeded on attempt {}",
                        attempt
                    );

                    job_run.status = JobRunStatus::Success;
                    job_run.finished_at = Some(now_timestamp());

                    if let Err(e) = db::runs::update_job_run(&conn, &job_run).await {
                        error!(task_name = %task_name, "Failed to update job_run: {}", e);
                    }

                    // Run on_success hooks
                    run_hooks_by_type(&conn, &task.id, &job_run.id, HookType::Success, db_path, task_name).await;

                    return Ok(0);
                } else {
                    // 8. Non-zero exit code
                    warn!(
                        task_name = %task_name,
                        exit_code = cmd_result.exit_code,
                        "Command failed with exit code {} on attempt {}",
                        cmd_result.exit_code,
                        attempt
                    );

                    let next_attempt = attempt + 1;
                    if retry::should_retry(task.max_retries, next_attempt) {
                        // Still have retries left
                        info!(
                            task_name = %task_name,
                            "Will retry (attempt {}/{})",
                            next_attempt,
                            task.max_retries + 1
                        );

                        job_run.status = JobRunStatus::Retrying;
                        if let Err(e) = db::runs::update_job_run(&conn, &job_run).await {
                            error!(task_name = %task_name, "Failed to update job_run: {}", e);
                        }

                        retry::sleep_with_backoff(task.retry_delay_secs, attempt).await;
                        attempt = next_attempt;
                        continue;
                    } else {
                        // Retries exhausted
                        error!(
                            task_name = %task_name,
                            "All retries exhausted after {} attempts",
                            attempt
                        );

                        job_run.status = JobRunStatus::Failed;
                        job_run.finished_at = Some(now_timestamp());

                        if let Err(e) = db::runs::update_job_run(&conn, &job_run).await {
                            error!(task_name = %task_name, "Failed to update job_run: {}", e);
                        }

                        // Run on_failure hooks
                        run_hooks_by_type(&conn, &task.id, &job_run.id, HookType::Failure, db_path, task_name).await;

                        // Run on_retry_exhausted hooks
                        run_hooks_by_type(&conn, &task.id, &job_run.id, HookType::RetryExhausted, db_path, task_name).await;

                        return Ok(1);
                    }
                }
            }
            Err(e) => {
                // Command failed to even start
                error!(
                    task_name = %task_name,
                    "Command execution error on attempt {}: {}",
                    attempt,
                    e
                );

                job_run.stderr = format!("Execution error: {}", e);
                job_run.status = JobRunStatus::Failed;
                job_run.finished_at = Some(now_timestamp());
                job_run.attempt = attempt;

                if let Err(update_err) = db::runs::update_job_run(&conn, &job_run).await {
                    error!(task_name = %task_name, "Failed to update job_run: {}", update_err);
                }

                // Run on_failure hooks
                run_hooks_by_type(&conn, &task.id, &job_run.id, HookType::Failure, db_path, task_name).await;

                return Ok(1);
            }
        }
    }
}

/// Helper to fetch hooks of a given type and execute them.
async fn run_hooks_by_type(
    conn: &libsql::Connection,
    task_id: &str,
    job_run_id: &str,
    hook_type: HookType,
    db_path: &str,
    task_name: &str,
) {
    let mut matched_hooks = match db::hooks::get_by_type(conn, task_id, &hook_type).await {
        Ok(task_hooks) => task_hooks,
        Err(e) => {
            warn!(
                task_name = %task_name,
                "Failed to fetch task-scoped {} hooks: {}",
                hook_type,
                e
            );
            Vec::new()
        }
    };

    match db::hooks::get_global_by_type(conn, &hook_type).await {
        Ok(global_hooks) => matched_hooks.extend(global_hooks),
        Err(e) => {
            warn!(
                task_name = %task_name,
                "Failed to fetch global {} hooks: {}",
                hook_type,
                e
            );
        }
    }

    if !matched_hooks.is_empty() {
        info!(
            task_name = %task_name,
            hook_type = %hook_type,
            count = matched_hooks.len(),
            "Running {} hooks",
            hook_type
        );
        if let Err(e) = hooks::execute_hooks(&matched_hooks, job_run_id, db_path).await {
            warn!(
                task_name = %task_name,
                "Error executing {} hooks: {}",
                hook_type,
                e
            );
        }
    }
}

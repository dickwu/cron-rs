use std::path::PathBuf;

use cron_rs::db;
use cron_rs::models::task::ConcurrencyPolicy;
use cron_rs::models::*;
use cron_rs::runner;
use cron_rs::runner::executor;
use cron_rs::runner::retry;

/// Create a temp DB path with a unique name.
fn temp_db_path() -> PathBuf {
    let id = uuid::Uuid::new_v4();
    let dir = PathBuf::from(format!("/tmp/cron-rs-test-{id}"));
    std::fs::create_dir_all(&dir).expect("Failed to create test directory");
    dir.join("cron-rs.db")
}

/// Create and initialize a fresh test database, returning (Database, path_string).
async fn setup_db() -> (db::Database, PathBuf) {
    let path = temp_db_path();
    let database = db::Database::new(&path)
        .await
        .expect("Failed to create database");
    database
        .run_migrations()
        .await
        .expect("Failed to run migrations");
    (database, path)
}

fn cleanup_db(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

fn make_test_task(name: &str, command: &str) -> Task {
    Task {
        id: String::new(),
        name: name.to_string(),
        command: command.to_string(),
        schedule: "*-*-* *:00:00".to_string(),
        tags: Vec::new(),
        description: "test task".to_string(),
        enabled: true,
        max_retries: 0,
        retry_delay_secs: 5,
        timeout_secs: None,
        concurrency_policy: ConcurrencyPolicy::Skip,
        lock_key: None,
        sandbox_profile: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

// T5: Command succeeds -> status='success'
#[tokio::test]
async fn t5_command_succeeds_status_success() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("success-task", "echo hello && exit 0");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let db_path_str = path.to_str().unwrap();
    let exit_code = runner::run_task(&created.id, &created.name, db_path_str)
        .await
        .unwrap();
    assert_eq!(exit_code, 0, "Successful command should return exit code 0");

    // Verify the run record in DB
    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, JobRunStatus::Success);
    assert_eq!(runs[0].exit_code, Some(0));

    cleanup_db(&path);
}

// T6: Command fails -> status='failed'
#[tokio::test]
async fn t6_command_fails_status_failed() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("fail-task", "exit 1");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let db_path_str = path.to_str().unwrap();
    let exit_code = runner::run_task(&created.id, &created.name, db_path_str)
        .await
        .unwrap();
    assert_eq!(exit_code, 1, "Failed command should return exit code 1");

    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, JobRunStatus::Failed);
    assert_eq!(runs[0].exit_code, Some(1));

    cleanup_db(&path);
}

// T10: stdout/stderr captured correctly
#[tokio::test]
async fn t10_stdout_stderr_captured() {
    let result =
        executor::execute_command("echo stdout_content; echo stderr_content >&2", None, None)
            .await
            .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(
        result.stdout.contains("stdout_content"),
        "stdout should contain 'stdout_content', got: '{}'",
        result.stdout
    );
    assert!(
        result.stderr.contains("stderr_content"),
        "stderr should contain 'stderr_content', got: '{}'",
        result.stderr
    );
    assert!(!result.timed_out);
    assert!(result.duration_ms >= 0);
}

// T8: Retry succeeds on 2nd attempt (counter file approach)
#[tokio::test]
async fn t8_retry_succeeds_on_second_attempt() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    // Use a counter file: fail on first run, succeed on second
    let counter_file = format!("/tmp/cron-rs-retry-test-{}", uuid::Uuid::new_v4());
    let command = format!(
        "if [ -f '{}' ]; then exit 0; else touch '{}'; exit 1; fi",
        counter_file, counter_file
    );

    let mut task = make_test_task("retry-task", &command);
    task.max_retries = 2;
    task.retry_delay_secs = 1; // Short delay for tests
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let db_path_str = path.to_str().unwrap();
    let exit_code = runner::run_task(&created.id, &created.name, db_path_str)
        .await
        .unwrap();
    assert_eq!(exit_code, 0, "Should succeed on retry");

    // Verify the run ended as success after retrying
    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, JobRunStatus::Success);
    assert_eq!(runs[0].attempt, 2, "Should have succeeded on attempt 2");

    // Cleanup
    let _ = std::fs::remove_file(&counter_file);
    cleanup_db(&path);
}

#[tokio::test]
async fn global_success_hooks_execute_for_task_runs() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("global-hook-success-task", "echo hello && exit 0");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let hook = Hook {
        id: String::new(),
        task_id: None,
        hook_type: HookType::Success,
        command: "echo global_success_hook".to_string(),
        timeout_secs: Some(5),
        run_order: 0,
        created_at: String::new(),
    };
    db::hooks::create(&conn, &hook).await.unwrap();

    let db_path_str = path.to_str().unwrap();
    let exit_code = runner::run_task(&created.id, &created.name, db_path_str)
        .await
        .unwrap();
    assert_eq!(exit_code, 0);

    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);

    let hook_runs = db::runs::list_hook_runs(&conn, &runs[0].id).await.unwrap();
    assert_eq!(hook_runs.len(), 1);
    assert_eq!(hook_runs[0].status, HookRunStatus::Success);
    assert!(hook_runs[0].stdout.contains("global_success_hook"));

    cleanup_db(&path);
}

// Retry logic unit tests
#[test]
fn test_should_retry_no_retries() {
    assert!(
        !retry::should_retry(0, 1),
        "0 max_retries means no retries at all"
    );
    assert!(!retry::should_retry(0, 2));
}

#[test]
fn test_should_retry_with_retries() {
    // max_retries=3 means attempts 1,2,3 are ok, attempt 4 is not
    assert!(retry::should_retry(3, 1));
    assert!(retry::should_retry(3, 2));
    assert!(retry::should_retry(3, 3));
    assert!(!retry::should_retry(3, 4));
}

#[test]
fn test_retry_delay_exponential_backoff() {
    // base_delay_secs=5
    assert_eq!(retry::retry_delay_secs(5, 1), 5); // 5 * 2^0
    assert_eq!(retry::retry_delay_secs(5, 2), 10); // 5 * 2^1
    assert_eq!(retry::retry_delay_secs(5, 3), 20); // 5 * 2^2
    assert_eq!(retry::retry_delay_secs(5, 4), 40); // 5 * 2^3
}

#[test]
fn test_retry_delay_capped_at_3600() {
    assert_eq!(retry::retry_delay_secs(1000, 5), 3600);
    assert_eq!(retry::retry_delay_secs(1, 30), 3600);
}

#[test]
fn test_retry_delay_minimum_base() {
    // Base delay of 0 is treated as 1
    assert_eq!(retry::retry_delay_secs(0, 1), 1);
    assert_eq!(retry::retry_delay_secs(-5, 1), 1);
}

// Test executor: command timeout
#[tokio::test]
async fn test_executor_command_timeout() {
    let result = executor::execute_command("sleep 60", Some(1), None)
        .await
        .unwrap();

    assert!(result.timed_out, "Command should have timed out");
    assert_eq!(result.exit_code, -1);
}

// Test executor: successful command
#[tokio::test]
async fn test_executor_successful_command() {
    let result = executor::execute_command("echo test_output", None, None)
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(!result.timed_out);
    assert!(result.stdout.contains("test_output"));
    assert!(result.duration_ms >= 0);
}

// Test executor: failing command
#[tokio::test]
async fn test_executor_failing_command() {
    let result = executor::execute_command("exit 42", None, None)
        .await
        .unwrap();

    assert_eq!(result.exit_code, 42);
    assert!(!result.timed_out);
}

// Executor streams partial output via the progress channel while the command
// is still running, not only once it finishes.
#[tokio::test]
async fn test_executor_streams_progress() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<executor::ProgressSnapshot>(16);

    // START is printed immediately; END only after a 2s sleep. The ~1s tailer
    // tick must flush a snapshot containing START (but not yet END) mid-run.
    let handle = tokio::spawn(async move {
        executor::execute_command("echo START; sleep 2; echo END", None, Some(tx)).await
    });

    let mut snapshots = Vec::new();
    while let Some(snap) = rx.recv().await {
        snapshots.push(snap);
    }

    let result = handle.await.unwrap().unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(
        result.stdout.contains("START") && result.stdout.contains("END"),
        "final output should be complete, got: {:?}",
        result.stdout
    );

    assert!(
        !snapshots.is_empty(),
        "expected at least one mid-run progress snapshot"
    );
    assert!(
        snapshots
            .iter()
            .any(|(out, _)| out.contains("START") && !out.contains("END")),
        "expected an intermediate snapshot before completion, got: {snapshots:?}"
    );
}

// Partial-output flushes persist for a running run, but are a no-op once the
// run is finalized — so a late flush can never truncate the final output.
#[tokio::test]
async fn test_update_job_run_output_guarded_by_status() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("stream-task", "true");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let mut run = JobRun {
        id: String::new(),
        task_id: created.id.clone(),
        started_at: String::new(),
        finished_at: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        status: JobRunStatus::Running,
        attempt: 1,
        duration_ms: None,
    };
    run = db::runs::create_job_run(&conn, &run).await.unwrap();

    // While running, partial output is persisted.
    db::runs::update_job_run_output(&conn, &run.id, "partial out", "partial err")
        .await
        .unwrap();
    let fetched = db::runs::get_job_run_by_id(&conn, &run.id).await.unwrap();
    assert_eq!(fetched.stdout, "partial out");
    assert_eq!(fetched.stderr, "partial err");

    // Finalize the run with its authoritative output.
    run.status = JobRunStatus::Success;
    run.stdout = "final out".to_string();
    run.stderr = "final err".to_string();
    run.exit_code = Some(0);
    run.finished_at = Some("2026-01-01T00:00:00Z".to_string());
    db::runs::update_job_run(&conn, &run).await.unwrap();

    // A flush arriving after finalization must not change anything.
    db::runs::update_job_run_output(&conn, &run.id, "stale out", "stale err")
        .await
        .unwrap();
    let after = db::runs::get_job_run_by_id(&conn, &run.id).await.unwrap();
    assert_eq!(
        after.stdout, "final out",
        "a late flush must not truncate the final output"
    );
    assert_eq!(after.stderr, "final err");
    assert_eq!(after.status, JobRunStatus::Success);

    cleanup_db(&path);
}

// Full runner path: partial output is visible in the DB while the run is still
// in flight (drives the dashboard's live terminal), and the final write is
// complete once it finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_run_task_persists_partial_output_while_running() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    // START prints immediately; END only after a 3s sleep.
    let task = make_test_task("live-task", "echo START; sleep 3; echo END");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let db_path_str = path.to_str().unwrap().to_string();
    let run_handle = {
        let id = created.id.clone();
        let name = created.name.clone();
        tokio::spawn(async move { runner::run_task(&id, &name, &db_path_str).await })
    };

    // After the run starts and the ~1s tailer flushes — but before END at ~3s —
    // the DB row should already carry the partial output.
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
    let mid = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(mid.len(), 1, "run row should exist while running");
    assert_eq!(
        mid[0].status,
        JobRunStatus::Running,
        "run should still be in flight"
    );
    assert!(
        mid[0].stdout.contains("START"),
        "partial output should be visible mid-run, got: {:?}",
        mid[0].stdout
    );
    assert!(
        !mid[0].stdout.contains("END"),
        "END must not appear before the command finishes, got: {:?}",
        mid[0].stdout
    );

    // Let it finish and confirm the authoritative final output landed.
    let exit = run_handle.await.unwrap().unwrap();
    assert_eq!(exit, 0);
    let done = db::runs::get_job_run_by_id(&conn, &mid[0].id)
        .await
        .unwrap();
    assert_eq!(done.status, JobRunStatus::Success);
    assert!(
        done.stdout.contains("START") && done.stdout.contains("END"),
        "final output should be complete, got: {:?}",
        done.stdout
    );

    cleanup_db(&path);
}

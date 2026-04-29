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
    PathBuf::from(format!("/tmp/cron-rs-test-{}.db", id))
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
    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None)
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

    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None)
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
    let result = executor::execute_command("echo stdout_content; echo stderr_content >&2", None)
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
    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None)
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

    let runs = db::runs::list_job_runs(&conn, Some(&created.id), None, None, None)
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
    let result = executor::execute_command("sleep 60", Some(1))
        .await
        .unwrap();

    assert!(result.timed_out, "Command should have timed out");
    assert_eq!(result.exit_code, -1);
}

// Test executor: successful command
#[tokio::test]
async fn test_executor_successful_command() {
    let result = executor::execute_command("echo test_output", None)
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
    let result = executor::execute_command("exit 42", None).await.unwrap();

    assert_eq!(result.exit_code, 42);
    assert!(!result.timed_out);
}

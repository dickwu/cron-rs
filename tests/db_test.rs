use std::path::PathBuf;

use cron_rs::db;
use cron_rs::db::helpers::DbError;
use cron_rs::models::task::ConcurrencyPolicy;
use cron_rs::models::*;

/// Create a temp DB path with a unique name.
fn temp_db_path() -> PathBuf {
    let id = uuid::Uuid::new_v4();
    PathBuf::from(format!("/tmp/cron-rs-test-{}.db", id))
}

/// Create and initialize a fresh test database.
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

/// Clean up a temp database file and its companions (-wal, -shm).
fn cleanup_db(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

/// Helper to create a test task with defaults.
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

// T36: Migrations apply on fresh DB
#[tokio::test]
async fn t36_migrations_apply_on_fresh_db() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    // Verify the tasks table exists by running a query
    let mut rows = conn
        .query("SELECT COUNT(*) FROM tasks", ())
        .await
        .expect("tasks table should exist");
    let row = rows.next().await.unwrap().unwrap();
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 0);

    // Verify hooks table exists
    let mut rows = conn
        .query("SELECT COUNT(*) FROM hooks", ())
        .await
        .expect("hooks table should exist");
    let row = rows.next().await.unwrap().unwrap();
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 0);

    // Verify job_runs table exists
    let mut rows = conn
        .query("SELECT COUNT(*) FROM job_runs", ())
        .await
        .expect("job_runs table should exist");
    let row = rows.next().await.unwrap().unwrap();
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 0);

    // Verify hook_runs table exists
    let mut rows = conn
        .query("SELECT COUNT(*) FROM hook_runs", ())
        .await
        .expect("hook_runs table should exist");
    let row = rows.next().await.unwrap().unwrap();
    let count: i64 = row.get(0).unwrap();
    assert_eq!(count, 0);

    cleanup_db(&path);
}

// T37: Migrations are idempotent (run twice without error)
#[tokio::test]
async fn t37_migrations_are_idempotent() {
    let path = temp_db_path();
    let database = db::Database::new(&path).await.unwrap();

    // First run
    database
        .run_migrations()
        .await
        .expect("First migration should succeed");

    // Second run — should not fail
    database
        .run_migrations()
        .await
        .expect("Second migration should succeed (idempotent)");

    cleanup_db(&path);
}

// T38: WAL mode is enabled
#[tokio::test]
async fn t38_wal_mode_enabled() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let mut rows = conn.query("PRAGMA journal_mode", ()).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let mode: String = row.get(0).unwrap();
    assert_eq!(mode.to_lowercase(), "wal");

    cleanup_db(&path);
}

// T40: CASCADE deletes work (delete task -> runs + hooks deleted)
#[tokio::test]
async fn t40_cascade_deletes() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    // Enable foreign keys
    conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();

    // Create a task
    let task = make_test_task("cascade-test", "echo hello");
    let created_task = db::tasks::create(&conn, &task).await.unwrap();

    // Create a hook for the task
    let hook = Hook {
        id: String::new(),
        task_id: Some(created_task.id.clone()),
        hook_type: HookType::Failure,
        command: "echo fail".to_string(),
        timeout_secs: None,
        run_order: 0,
        created_at: String::new(),
    };
    let created_hook = db::hooks::create(&conn, &hook).await.unwrap();

    // Create a job run for the task
    let run = JobRun {
        id: String::new(),
        task_id: created_task.id.clone(),
        started_at: String::new(),
        finished_at: None,
        exit_code: Some(0),
        stdout: "output".to_string(),
        stderr: String::new(),
        status: JobRunStatus::Success,
        attempt: 1,
        duration_ms: Some(100),
    };
    let created_run = db::runs::create_job_run(&conn, &run).await.unwrap();

    // Create a hook run
    let hook_run = HookRun {
        id: String::new(),
        job_run_id: created_run.id.clone(),
        hook_id: created_hook.id.clone(),
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        started_at: String::new(),
        finished_at: None,
        status: HookRunStatus::Success,
    };
    db::runs::create_hook_run(&conn, &hook_run).await.unwrap();

    // Now delete the task — CASCADE should delete everything
    db::tasks::delete(&conn, &created_task.id).await.unwrap();

    // Verify hooks are deleted
    let hooks = db::hooks::list_for_task(&conn, &created_task.id)
        .await
        .unwrap();
    assert!(hooks.is_empty(), "Hooks should be deleted by CASCADE");

    // Verify job runs are deleted
    let runs = db::runs::list_job_runs(&conn, Some(&created_task.id), None, None, None)
        .await
        .unwrap();
    assert!(runs.is_empty(), "Job runs should be deleted by CASCADE");

    cleanup_db(&path);
}

// Task CRUD: create
#[tokio::test]
async fn task_crud_create() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("my-task", "echo hello");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    assert!(!created.id.is_empty(), "ID should be auto-generated");
    assert_eq!(created.name, "my-task");
    assert_eq!(created.command, "echo hello");
    assert_eq!(created.schedule, "*-*-* *:00:00");
    assert!(created.enabled);
    assert!(!created.created_at.is_empty());
    assert!(!created.updated_at.is_empty());

    cleanup_db(&path);
}

// Task CRUD: get_by_id
#[tokio::test]
async fn task_crud_get_by_id() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("get-test", "echo test");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let fetched = db::tasks::get_by_id(&conn, &created.id).await.unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.name, "get-test");

    // Not found case
    let result = db::tasks::get_by_id(&conn, "nonexistent-id").await;
    assert!(matches!(result, Err(DbError::NotFound)));

    cleanup_db(&path);
}

// Task CRUD: get_by_name
#[tokio::test]
async fn task_crud_get_by_name() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("name-lookup", "echo hello");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let fetched = db::tasks::get_by_name(&conn, "name-lookup").await.unwrap();
    assert_eq!(fetched.id, created.id);

    // Not found case
    let result = db::tasks::get_by_name(&conn, "nonexistent").await;
    assert!(matches!(result, Err(DbError::NotFound)));

    cleanup_db(&path);
}

// Task CRUD: list
#[tokio::test]
async fn task_crud_list() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    // Empty initially
    let tasks = db::tasks::list(&conn).await.unwrap();
    assert!(tasks.is_empty());

    // Create two tasks
    db::tasks::create(&conn, &make_test_task("alpha", "echo a"))
        .await
        .unwrap();
    db::tasks::create(&conn, &make_test_task("beta", "echo b"))
        .await
        .unwrap();

    let tasks = db::tasks::list(&conn).await.unwrap();
    assert_eq!(tasks.len(), 2);
    // Ordered by name
    assert_eq!(tasks[0].name, "alpha");
    assert_eq!(tasks[1].name, "beta");

    cleanup_db(&path);
}

// Task CRUD: update
#[tokio::test]
async fn task_crud_update() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("updatable", "echo old");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    let mut updated = created.clone();
    updated.command = "echo new".to_string();
    updated.max_retries = 3;
    updated.lock_key = Some("staff-api-boot".to_string());
    updated.sandbox_profile = Some("staff-api-hyperf".to_string());

    let saved = db::tasks::update(&conn, &updated).await.unwrap();
    assert_eq!(saved.command, "echo new");
    assert_eq!(saved.max_retries, 3);
    assert_eq!(saved.lock_key.as_deref(), Some("staff-api-boot"));
    assert_eq!(saved.sandbox_profile.as_deref(), Some("staff-api-hyperf"));
    assert_eq!(saved.id, created.id);

    cleanup_db(&path);
}

// Task CRUD: delete
#[tokio::test]
async fn task_crud_delete() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = make_test_task("deletable", "echo bye");
    let created = db::tasks::create(&conn, &task).await.unwrap();

    db::tasks::delete(&conn, &created.id).await.unwrap();

    let result = db::tasks::get_by_id(&conn, &created.id).await;
    assert!(matches!(result, Err(DbError::NotFound)));

    // Delete non-existent should fail
    let result = db::tasks::delete(&conn, "nonexistent").await;
    assert!(matches!(result, Err(DbError::NotFound)));

    cleanup_db(&path);
}

// Task name uniqueness constraint
#[tokio::test]
async fn task_name_unique_constraint() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    db::tasks::create(&conn, &make_test_task("unique-name", "echo 1"))
        .await
        .unwrap();

    let result = db::tasks::create(&conn, &make_test_task("unique-name", "echo 2")).await;
    assert!(matches!(result, Err(DbError::Conflict(_))));

    cleanup_db(&path);
}

// Hook CRUD: create
#[tokio::test]
async fn hook_crud_create() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("hook-test", "echo hi"))
        .await
        .unwrap();

    let hook = Hook {
        id: String::new(),
        task_id: Some(task.id.clone()),
        hook_type: HookType::Failure,
        command: "curl -X POST http://alert".to_string(),
        timeout_secs: Some(30),
        run_order: 1,
        created_at: String::new(),
    };

    let created = db::hooks::create(&conn, &hook).await.unwrap();
    assert!(!created.id.is_empty());
    assert_eq!(created.task_id.as_deref(), Some(task.id.as_str()));
    assert_eq!(created.hook_type, HookType::Failure);
    assert_eq!(created.command, "curl -X POST http://alert");
    assert_eq!(created.timeout_secs, Some(30));
    assert_eq!(created.run_order, 1);

    cleanup_db(&path);
}

// Hook CRUD: list_for_task
#[tokio::test]
async fn hook_crud_list_for_task() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("hook-list-test", "echo hi"))
        .await
        .unwrap();

    // Create multiple hooks
    for i in 0..3 {
        let hook = Hook {
            id: String::new(),
            task_id: Some(task.id.clone()),
            hook_type: HookType::Failure,
            command: format!("echo hook_{}", i),
            timeout_secs: None,
            run_order: i,
            created_at: String::new(),
        };
        db::hooks::create(&conn, &hook).await.unwrap();
    }

    let hooks = db::hooks::list_for_task(&conn, &task.id).await.unwrap();
    assert_eq!(hooks.len(), 3);
    // Should be ordered by run_order
    assert_eq!(hooks[0].run_order, 0);
    assert_eq!(hooks[1].run_order, 1);
    assert_eq!(hooks[2].run_order, 2);

    cleanup_db(&path);
}

// Hook CRUD: list_global
#[tokio::test]
async fn hook_crud_list_global() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("hook-global-test", "echo hi"))
        .await
        .unwrap();

    db::hooks::create(
        &conn,
        &Hook {
            id: String::new(),
            task_id: Some(task.id.clone()),
            hook_type: HookType::Failure,
            command: "echo task_hook".to_string(),
            timeout_secs: None,
            run_order: 0,
            created_at: String::new(),
        },
    )
    .await
    .unwrap();

    db::hooks::create(
        &conn,
        &Hook {
            id: String::new(),
            task_id: None,
            hook_type: HookType::Success,
            command: "echo global_hook".to_string(),
            timeout_secs: Some(15),
            run_order: 1,
            created_at: String::new(),
        },
    )
    .await
    .unwrap();

    let hooks = db::hooks::list_global(&conn).await.unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].task_id, None);
    assert_eq!(hooks[0].command, "echo global_hook");

    cleanup_db(&path);
}

// Hook CRUD: get_by_type
#[tokio::test]
async fn hook_crud_get_by_type() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("hook-type-test", "echo hi"))
        .await
        .unwrap();

    // Create hooks of different types
    for (i, hook_type) in [
        HookType::Failure,
        HookType::Success,
        HookType::RetryExhausted,
    ]
    .iter()
    .enumerate()
    {
        let hook = Hook {
            id: String::new(),
            task_id: Some(task.id.clone()),
            hook_type: hook_type.clone(),
            command: format!("echo hook_{}", i),
            timeout_secs: None,
            run_order: 0,
            created_at: String::new(),
        };
        db::hooks::create(&conn, &hook).await.unwrap();
    }

    let failure_hooks = db::hooks::get_by_type(&conn, &task.id, &HookType::Failure)
        .await
        .unwrap();
    assert_eq!(failure_hooks.len(), 1);
    assert_eq!(failure_hooks[0].hook_type, HookType::Failure);

    let success_hooks = db::hooks::get_by_type(&conn, &task.id, &HookType::Success)
        .await
        .unwrap();
    assert_eq!(success_hooks.len(), 1);

    cleanup_db(&path);
}

// Hook CRUD: delete
#[tokio::test]
async fn hook_crud_delete() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("hook-delete-test", "echo hi"))
        .await
        .unwrap();

    let hook = Hook {
        id: String::new(),
        task_id: Some(task.id.clone()),
        hook_type: HookType::Success,
        command: "echo done".to_string(),
        timeout_secs: None,
        run_order: 0,
        created_at: String::new(),
    };
    let created = db::hooks::create(&conn, &hook).await.unwrap();

    db::hooks::delete(&conn, &created.id).await.unwrap();

    let result = db::hooks::get_by_id(&conn, &created.id).await;
    assert!(matches!(result, Err(DbError::NotFound)));

    // Delete non-existent should fail
    let result = db::hooks::delete(&conn, "nonexistent").await;
    assert!(matches!(result, Err(DbError::NotFound)));

    cleanup_db(&path);
}

// Run CRUD: create_job_run
#[tokio::test]
async fn run_crud_create_job_run() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("run-create-test", "echo hi"))
        .await
        .unwrap();

    let run = JobRun {
        id: String::new(),
        task_id: task.id.clone(),
        started_at: String::new(),
        finished_at: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        status: JobRunStatus::Running,
        attempt: 1,
        duration_ms: None,
    };

    let created = db::runs::create_job_run(&conn, &run).await.unwrap();
    assert!(!created.id.is_empty());
    assert_eq!(created.task_id, task.id);
    assert_eq!(created.status, JobRunStatus::Running);
    assert!(!created.started_at.is_empty());

    cleanup_db(&path);
}

// Run CRUD: update_job_run
#[tokio::test]
async fn run_crud_update_job_run() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("run-update-test", "echo hi"))
        .await
        .unwrap();

    let run = JobRun {
        id: String::new(),
        task_id: task.id.clone(),
        started_at: String::new(),
        finished_at: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        status: JobRunStatus::Running,
        attempt: 1,
        duration_ms: None,
    };
    let mut created = db::runs::create_job_run(&conn, &run).await.unwrap();

    // Update with completion info
    created.status = JobRunStatus::Success;
    created.exit_code = Some(0);
    created.stdout = "hello world".to_string();
    created.finished_at = Some("2024-01-01 12:00:00".to_string());
    created.duration_ms = Some(500);

    db::runs::update_job_run(&conn, &created).await.unwrap();

    let fetched = db::runs::get_job_run_by_id(&conn, &created.id)
        .await
        .unwrap();
    assert_eq!(fetched.status, JobRunStatus::Success);
    assert_eq!(fetched.exit_code, Some(0));
    assert_eq!(fetched.stdout, "hello world");
    assert_eq!(fetched.duration_ms, Some(500));

    cleanup_db(&path);
}

// Run CRUD: list with filters
#[tokio::test]
async fn run_crud_list_with_filters() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task1 = db::tasks::create(&conn, &make_test_task("filter-test-1", "echo 1"))
        .await
        .unwrap();
    let task2 = db::tasks::create(&conn, &make_test_task("filter-test-2", "echo 2"))
        .await
        .unwrap();

    // Create runs for task1
    for status in [JobRunStatus::Success, JobRunStatus::Failed] {
        let run = JobRun {
            id: String::new(),
            task_id: task1.id.clone(),
            started_at: String::new(),
            finished_at: Some("2024-01-01 12:00:00".to_string()),
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            status,
            attempt: 1,
            duration_ms: Some(100),
        };
        db::runs::create_job_run(&conn, &run).await.unwrap();
    }

    // Create a run for task2
    let run = JobRun {
        id: String::new(),
        task_id: task2.id.clone(),
        started_at: String::new(),
        finished_at: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        status: JobRunStatus::Running,
        attempt: 1,
        duration_ms: None,
    };
    db::runs::create_job_run(&conn, &run).await.unwrap();

    // All runs
    let all = db::runs::list_job_runs(&conn, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    // Filter by task_id
    let task1_runs = db::runs::list_job_runs(&conn, Some(&task1.id), None, None, None)
        .await
        .unwrap();
    assert_eq!(task1_runs.len(), 2);

    // Filter by status
    let failed_runs = db::runs::list_job_runs(&conn, None, Some("failed"), None, None)
        .await
        .unwrap();
    assert_eq!(failed_runs.len(), 1);

    // Filter by both
    let task1_success =
        db::runs::list_job_runs(&conn, Some(&task1.id), Some("success"), None, None)
            .await
            .unwrap();
    assert_eq!(task1_success.len(), 1);

    // Limit
    let limited = db::runs::list_job_runs(&conn, None, None, Some(2), None)
        .await
        .unwrap();
    assert_eq!(limited.len(), 2);

    cleanup_db(&path);
}

// Run CRUD: mark_orphaned_runs_crashed
#[tokio::test]
async fn run_crud_mark_orphaned_runs_crashed() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("orphan-test", "echo hi"))
        .await
        .unwrap();

    // Create running and retrying runs (these are "orphaned")
    for status in [JobRunStatus::Running, JobRunStatus::Retrying] {
        let run = JobRun {
            id: String::new(),
            task_id: task.id.clone(),
            started_at: String::new(),
            finished_at: None,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            status,
            attempt: 1,
            duration_ms: None,
        };
        db::runs::create_job_run(&conn, &run).await.unwrap();
    }

    // Create a completed run (should not be affected)
    let done_run = JobRun {
        id: String::new(),
        task_id: task.id.clone(),
        started_at: String::new(),
        finished_at: Some("2024-01-01 12:00:00".to_string()),
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        status: JobRunStatus::Success,
        attempt: 1,
        duration_ms: Some(100),
    };
    db::runs::create_job_run(&conn, &done_run).await.unwrap();

    // Mark orphaned
    let count = db::runs::mark_orphaned_runs_crashed(&conn).await.unwrap();
    assert_eq!(count, 2);

    // Verify statuses
    let all_runs = db::runs::list_job_runs(&conn, Some(&task.id), None, None, None)
        .await
        .unwrap();

    let crashed = all_runs
        .iter()
        .filter(|r| r.status == JobRunStatus::Crashed)
        .count();
    let success = all_runs
        .iter()
        .filter(|r| r.status == JobRunStatus::Success)
        .count();
    assert_eq!(crashed, 2, "Both running/retrying runs should be crashed");
    assert_eq!(success, 1, "Success run should be unchanged");

    // Running again should not crash any more
    let count2 = db::runs::mark_orphaned_runs_crashed(&conn).await.unwrap();
    assert_eq!(count2, 0);

    cleanup_db(&path);
}

// Settings: default + roundtrip
#[tokio::test]
async fn settings_retention_default_and_roundtrip() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    // Default seeded by migration
    let days = db::settings::get_retention_days(&conn).await.unwrap();
    assert_eq!(days, 30);

    db::settings::set_retention_days(&conn, 7).await.unwrap();
    let days = db::settings::get_retention_days(&conn).await.unwrap();
    assert_eq!(days, 7);

    cleanup_db(&path);
}

// Prune: deletes job_runs older than the cutoff and their hook_runs.
#[tokio::test]
async fn prune_runs_older_than_deletes_old() {
    let (database, path) = setup_db().await;
    let conn = database.connect().await.unwrap();

    let task = db::tasks::create(&conn, &make_test_task("prune-test", "echo hi"))
        .await
        .unwrap();

    // Insert a stale run (started 100 days ago) directly so we can backdate it.
    let stale_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO job_runs (id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms)
         VALUES (?1, ?2, datetime('now', '-100 days'), datetime('now', '-100 days'), 0, '', '', 'success', 1, 1)",
        libsql::params![stale_id.clone(), task.id.clone()],
    )
    .await
    .unwrap();

    // Insert a recent run (1 day ago) that must survive.
    let fresh_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO job_runs (id, task_id, started_at, finished_at, exit_code, stdout, stderr, status, attempt, duration_ms)
         VALUES (?1, ?2, datetime('now', '-1 days'), datetime('now', '-1 days'), 0, '', '', 'success', 1, 1)",
        libsql::params![fresh_id.clone(), task.id.clone()],
    )
    .await
    .unwrap();

    // days=0 → no-op
    let deleted = db::runs::prune_runs_older_than(&conn, 0).await.unwrap();
    assert_eq!(deleted, 0);

    let deleted = db::runs::prune_runs_older_than(&conn, 30).await.unwrap();
    assert_eq!(deleted, 1);

    let remaining = db::runs::list_job_runs(&conn, Some(&task.id), None, None, None)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, fresh_id);

    cleanup_db(&path);
}

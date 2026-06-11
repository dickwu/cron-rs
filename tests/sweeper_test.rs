use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cron_rs::db;
use cron_rs::models::{ConcurrencyPolicy, JobRun, JobRunStatus, Task};
use cron_rs::sweeper;
use cron_rs::systemd::SystemdManager;

/// SystemdManager stub where only the named tasks have an active service.
#[derive(Clone, Default)]
struct StubSystemd {
    active_services: Arc<Mutex<HashSet<String>>>,
}

impl StubSystemd {
    fn set_service_active(&self, task_name: &str) {
        self.active_services
            .lock()
            .unwrap()
            .insert(task_name.to_string());
    }
}

#[async_trait::async_trait]
impl SystemdManager for StubSystemd {
    async fn install_task(&self, _task: &Task) -> anyhow::Result<()> {
        Ok(())
    }
    async fn remove_task(&self, _task_name: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn enable_timer(&self, _task_name: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn disable_timer(&self, _task_name: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn start_timer(&self, _task_name: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn stop_timer(&self, _task_name: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn daemon_reload(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn is_service_active(&self, task_name: &str) -> anyhow::Result<bool> {
        Ok(self.active_services.lock().unwrap().contains(task_name))
    }
    async fn active_timer_names(&self) -> anyhow::Result<HashSet<String>> {
        Ok(HashSet::new())
    }
}

fn temp_db_path() -> PathBuf {
    PathBuf::from(format!(
        "/tmp/cron-rs-sweeper-test-{}.db",
        uuid::Uuid::new_v4()
    ))
}

fn cleanup_db(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

async fn setup_db(path: &std::path::Path) -> db::Database {
    let database = db::Database::new(path).await.unwrap();
    database.run_migrations().await.unwrap();
    database
}

async fn seed_task(conn: &libsql::Connection, name: &str) -> Task {
    let task = Task {
        id: String::new(),
        name: name.to_string(),
        command: "/bin/true".to_string(),
        schedule: "*-*-* 03:00:00".to_string(),
        tags: Vec::new(),
        description: String::new(),
        enabled: true,
        max_retries: 0,
        retry_delay_secs: 0,
        timeout_secs: None,
        concurrency_policy: ConcurrencyPolicy::Allow,
        lock_key: None,
        sandbox_profile: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    db::tasks::create(conn, &task).await.unwrap()
}

async fn seed_running_run(
    conn: &libsql::Connection,
    task_id: &str,
    age_secs: i64,
    runner_pid: Option<i64>,
) -> String {
    let started = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
    let run = JobRun {
        id: String::new(),
        task_id: task_id.to_string(),
        started_at: started.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        finished_at: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        status: JobRunStatus::Running,
        attempt: 1,
        duration_ms: None,
    };
    let run = db::runs::create_job_run(conn, &run).await.unwrap();
    if let Some(pid) = runner_pid {
        db::runs::set_runner_pid(conn, &run.id, pid).await.unwrap();
    }
    run.id
}

async fn run_status(conn: &libsql::Connection, id: &str) -> JobRunStatus {
    db::runs::get_job_run_by_id(conn, id).await.unwrap().status
}

/// A PID that cannot exist: above any realistic kernel pid_max.
const DEAD_PID: i64 = 999_999_999;

#[tokio::test]
async fn sweep_marks_run_with_dead_pid_as_crashed() {
    let path = temp_db_path();
    let database = setup_db(&path).await;
    let conn = database.connect().await.unwrap();
    let task = seed_task(&conn, "dead-pid-task").await;
    let run_id = seed_running_run(&conn, &task.id, 600, Some(DEAD_PID)).await;

    let stub = StubSystemd::default();
    let crashed = sweeper::sweep_once(&database, &stub, 120).await.unwrap();

    assert_eq!(crashed, 1);
    assert_eq!(run_status(&conn, &run_id).await, JobRunStatus::Crashed);
    cleanup_db(&path);
}

#[tokio::test]
async fn sweep_spares_run_with_live_pid() {
    let path = temp_db_path();
    let database = setup_db(&path).await;
    let conn = database.connect().await.unwrap();
    let task = seed_task(&conn, "live-pid-task").await;
    // This test process is definitely alive.
    let live_pid = std::process::id() as i64;
    let run_id = seed_running_run(&conn, &task.id, 600, Some(live_pid)).await;

    let stub = StubSystemd::default();
    let crashed = sweeper::sweep_once(&database, &stub, 120).await.unwrap();

    assert_eq!(crashed, 0);
    assert_eq!(run_status(&conn, &run_id).await, JobRunStatus::Running);
    cleanup_db(&path);
}

#[tokio::test]
async fn sweep_spares_fresh_runs_within_grace() {
    let path = temp_db_path();
    let database = setup_db(&path).await;
    let conn = database.connect().await.unwrap();
    let task = seed_task(&conn, "fresh-task").await;
    let run_id = seed_running_run(&conn, &task.id, 5, Some(DEAD_PID)).await;

    let stub = StubSystemd::default();
    let crashed = sweeper::sweep_once(&database, &stub, 120).await.unwrap();

    assert_eq!(crashed, 0);
    assert_eq!(run_status(&conn, &run_id).await, JobRunStatus::Running);
    cleanup_db(&path);
}

#[tokio::test]
async fn sweep_without_pid_uses_service_state() {
    let path = temp_db_path();
    let database = setup_db(&path).await;
    let conn = database.connect().await.unwrap();
    let active_task = seed_task(&conn, "service-active").await;
    let inactive_task = seed_task(&conn, "service-inactive").await;
    let active_run = seed_running_run(&conn, &active_task.id, 600, None).await;
    let inactive_run = seed_running_run(&conn, &inactive_task.id, 600, None).await;

    let stub = StubSystemd::default();
    stub.set_service_active("service-active");
    let crashed = sweeper::sweep_once(&database, &stub, 120).await.unwrap();

    assert_eq!(crashed, 1);
    assert_eq!(run_status(&conn, &active_run).await, JobRunStatus::Running);
    assert_eq!(
        run_status(&conn, &inactive_run).await,
        JobRunStatus::Crashed
    );
    cleanup_db(&path);
}

#[tokio::test]
async fn sweep_marks_runs_of_deleted_tasks_as_crashed() {
    let path = temp_db_path();
    let database = setup_db(&path).await;
    let conn = database.connect().await.unwrap();
    let task = seed_task(&conn, "ghost-task").await;
    let run_id = seed_running_run(&conn, &task.id, 600, None).await;

    // Orphan the run the way a legacy/imported DB can: delete the task with
    // FK enforcement off so the cascade does not remove the run.
    let mut rows = conn.query("PRAGMA foreign_keys=OFF", ()).await.unwrap();
    let _ = rows.next().await;
    conn.execute("DELETE FROM tasks WHERE id = ?1", [task.id.clone()])
        .await
        .unwrap();

    let stub = StubSystemd::default();
    let crashed = sweeper::sweep_once(&database, &stub, 120).await.unwrap();

    assert_eq!(crashed, 1);
    assert_eq!(run_status(&conn, &run_id).await, JobRunStatus::Crashed);
    cleanup_db(&path);
}

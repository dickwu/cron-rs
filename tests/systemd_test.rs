use cron_rs::models::task::ConcurrencyPolicy;
use cron_rs::models::Task;
use cron_rs::systemd::unit_gen;

fn make_test_task(name: &str, schedule: &str) -> Task {
    Task {
        id: "abc-123-def-456".to_string(),
        name: name.to_string(),
        command: "echo hello".to_string(),
        schedule: schedule.to_string(),
        tags: Vec::new(),
        description: "test task".to_string(),
        enabled: true,
        max_retries: 0,
        retry_delay_secs: 5,
        timeout_secs: None,
        concurrency_policy: ConcurrencyPolicy::Skip,
        lock_key: None,
        sandbox_profile: None,
        created_at: "2024-01-01 00:00:00".to_string(),
        updated_at: "2024-01-01 00:00:00".to_string(),
    }
}

// T15: Timer file generated with correct OnCalendar
#[test]
fn t15_timer_has_correct_on_calendar() {
    let task = make_test_task("backup", "*-*-* 02:00:00");
    let content = unit_gen::generate_timer_unit(&task);

    assert!(
        content.contains("OnCalendar=*-*-* 02:00:00"),
        "Timer should contain OnCalendar with the task schedule. Got:\n{}",
        content
    );
    assert!(
        content.contains("[Timer]"),
        "Timer should contain [Timer] section"
    );
    assert!(
        content.contains("Persistent=true"),
        "Timer should have Persistent=true"
    );
    assert!(
        content.contains("[Install]"),
        "Timer should contain [Install] section"
    );
    assert!(
        content.contains("WantedBy=timers.target"),
        "Timer should be wanted by timers.target"
    );
}

// Test with various schedule expressions
#[test]
fn timer_with_minutely_schedule() {
    let content = unit_gen::generate_timer("minutely-task", "*-*-* *:*:00");
    assert!(content.contains("OnCalendar=*-*-* *:*:00"));
}

#[test]
fn timer_with_daily_schedule() {
    let content = unit_gen::generate_timer("daily-task", "*-*-* 00:00:00");
    assert!(content.contains("OnCalendar=*-*-* 00:00:00"));
}

// T16: Service file has TimeoutStartSec=infinity
#[test]
fn t16_service_has_timeout_start_sec_infinity() {
    let task = make_test_task("long-task", "*-*-* *:00:00");
    let content = unit_gen::generate_service_unit(&task, "/home/user/cron-rs.db");

    assert!(
        content.contains("TimeoutStartSec=infinity"),
        "Service should have TimeoutStartSec=infinity. Got:\n{}",
        content
    );
}

// T17: Service file has correct binary path + task args
#[test]
fn t17_service_has_correct_exec_start() {
    let content = unit_gen::generate_service(
        "my-task",
        "abc-123",
        "/usr/local/bin/cron-rs",
        "/home/user/cron-rs.db",
    );

    assert!(
        content.contains("ExecStart=/usr/local/bin/cron-rs run --task-id abc-123 --task-name my-task --db-path /home/user/cron-rs.db"),
        "Service ExecStart should have correct binary path and args. Got:\n{}",
        content
    );

    assert!(
        content.contains("Type=oneshot"),
        "Service should be Type=oneshot"
    );

    assert!(
        content.contains("Environment=CRON_RS_DB=/home/user/cron-rs.db"),
        "Service should set CRON_RS_DB environment variable"
    );
}

// T18: Unit file names sanitized (special chars)
#[test]
fn t18_unit_names_sanitized() {
    // Spaces become dashes
    assert_eq!(unit_gen::unit_name("my task"), "cron-rs-my-task");

    // Special chars become dashes
    assert_eq!(unit_gen::unit_name("task@foo.bar"), "cron-rs-task-foo-bar");

    // Leading/trailing dashes stripped
    assert_eq!(unit_gen::unit_name("--task--"), "cron-rs-task");

    // Already clean name
    assert_eq!(unit_gen::unit_name("clean-name"), "cron-rs-clean-name");

    // Underscores preserved
    assert_eq!(unit_gen::unit_name("task_name"), "cron-rs-task_name");

    // Complex name
    assert_eq!(
        unit_gen::unit_name("my task (v2) @daily!"),
        "cron-rs-my-task--v2---daily"
    );
}

// Timer filename
#[test]
fn timer_filename_format() {
    assert_eq!(unit_gen::timer_filename("backup"), "cron-rs-backup.timer");
    assert_eq!(unit_gen::timer_filename("my task"), "cron-rs-my-task.timer");
}

// Service filename
#[test]
fn service_filename_format() {
    assert_eq!(
        unit_gen::service_filename("backup"),
        "cron-rs-backup.service"
    );
    assert_eq!(
        unit_gen::service_filename("my task"),
        "cron-rs-my-task.service"
    );
}

// Timer description includes task name
#[test]
fn timer_description_includes_task_name() {
    let content = unit_gen::generate_timer("my-backup", "*-*-* 02:00:00");
    assert!(content.contains("Description=cron-rs timer: my-backup"));
}

// Service description includes task name
#[test]
fn service_description_includes_task_name() {
    let content = unit_gen::generate_service(
        "my-backup",
        "id-123",
        "/usr/bin/cron-rs",
        "/home/user/db.sqlite",
    );
    assert!(content.contains("Description=cron-rs task: my-backup"));
}

#[test]
fn daemon_service_runs_daemon_with_bind_args() {
    let content = unit_gen::generate_daemon_service(
        "/usr/local/bin/cron-rs",
        "0.0.0.0",
        9746,
        "/home/user/cron-rs",
        "/home/user/cron-rs/cron-rs.db",
    );

    assert!(content.contains("Description=cron-rs daemon"));
    assert!(content.contains("ExecStart=/usr/local/bin/cron-rs daemon --host 0.0.0.0 --port 9746"));
    assert!(content.contains("Environment=CRON_RS_CONFIG_DIR=/home/user/cron-rs"));
    assert!(content.contains("Environment=CRON_RS_DB=/home/user/cron-rs/cron-rs.db"));
    assert!(content.contains("Restart=on-failure"));
    assert!(content.contains("WantedBy=default.target"));
}

use crate::models::Task;

/// Sanitize a task name into a valid systemd unit name component.
/// Replace any char that isn't alphanumeric, dash, or underscore with a dash.
/// Remove leading/trailing dashes.
pub fn unit_name(task_name: &str) -> String {
    let sanitized: String = task_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    format!("cron-rs-{}", trimmed)
}

/// Return the .timer filename for the given task name.
pub fn timer_filename(task_name: &str) -> String {
    format!("{}.timer", unit_name(task_name))
}

/// Return the .service filename for the given task name.
pub fn service_filename(task_name: &str) -> String {
    format!("{}.service", unit_name(task_name))
}

/// Return the user service filename for the long-running cron-rs daemon.
pub fn daemon_service_filename() -> &'static str {
    "cron-rs-daemon.service"
}

/// Generate the content of a systemd .timer unit file for the given task.
#[allow(dead_code)]
pub fn generate_timer_unit(task: &Task) -> String {
    generate_timer(&task.name, &task.schedule)
}

/// Generate the content of a systemd .service unit file for the given task.
/// The service calls `cron-rs run --task-id <id> --task-name <name> --db-path <path>`.
#[allow(dead_code)]
pub fn generate_service_unit(task: &Task, db_path: &str) -> String {
    let binary_path = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("cron-rs"))
        .to_string_lossy()
        .to_string();
    generate_service(&task.name, &task.id, &binary_path, db_path)
}

/// Generate a .timer unit file content from raw parameters.
pub fn generate_timer(task_name: &str, schedule: &str) -> String {
    format!(
        "[Unit]\n\
         Description=cron-rs timer: {task_name}\n\
         \n\
         [Timer]\n\
         OnCalendar={schedule}\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

/// Generate a .service unit file content from raw parameters.
pub fn generate_service(
    task_name: &str,
    task_id: &str,
    binary_path: &str,
    db_path: &str,
) -> String {
    format!(
        "[Unit]\n\
         Description=cron-rs task: {task_name}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={binary_path} run --task-id {task_id} --task-name {task_name} --db-path {db_path}\n\
         Environment=CRON_RS_DB={db_path}\n\
         TimeoutStartSec=infinity\n"
    )
}

/// Generate a user systemd service for the long-running cron-rs API daemon.
pub fn generate_daemon_service(
    binary_path: &str,
    host: &str,
    port: u16,
    config_dir: &str,
    db_path: &str,
) -> String {
    format!(
        "[Unit]\n\
         Description=cron-rs daemon\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={binary_path} daemon --host {host} --port {port}\n\
         Environment=CRON_RS_CONFIG_DIR={config_dir}\n\
         Environment=CRON_RS_DB={db_path}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_name_simple() {
        assert_eq!(unit_name("backup"), "cron-rs-backup");
    }

    #[test]
    fn test_unit_name_with_spaces() {
        assert_eq!(unit_name("my task"), "cron-rs-my-task");
    }

    #[test]
    fn test_unit_name_with_special_chars() {
        assert_eq!(unit_name("task@foo.bar"), "cron-rs-task-foo-bar");
    }

    #[test]
    fn test_unit_name_leading_trailing_dashes() {
        assert_eq!(unit_name("--task--"), "cron-rs-task");
    }

    #[test]
    fn test_timer_filename() {
        assert_eq!(timer_filename("backup"), "cron-rs-backup.timer");
    }

    #[test]
    fn test_service_filename() {
        assert_eq!(service_filename("backup"), "cron-rs-backup.service");
    }

    #[test]
    fn test_generate_timer() {
        let content = generate_timer("backup", "*-*-* 02:00:00");
        assert!(content.contains("Description=cron-rs timer: backup"));
        assert!(content.contains("OnCalendar=*-*-* 02:00:00"));
        assert!(content.contains("Persistent=true"));
        assert!(content.contains("WantedBy=timers.target"));
    }

    #[test]
    fn test_generate_service() {
        let content = generate_service(
            "backup",
            "abc-123",
            "/usr/bin/cron-rs",
            "/home/user/cron-rs.db",
        );
        assert!(content.contains("Description=cron-rs task: backup"));
        assert!(content.contains("Type=oneshot"));
        assert!(content.contains("ExecStart=/usr/bin/cron-rs run --task-id abc-123 --task-name backup --db-path /home/user/cron-rs.db"));
        assert!(content.contains("Environment=CRON_RS_DB=/home/user/cron-rs.db"));
        assert!(content.contains("TimeoutStartSec=infinity"));
    }
}

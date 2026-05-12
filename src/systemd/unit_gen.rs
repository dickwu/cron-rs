use crate::models::Task;
use std::path::{Path, PathBuf};

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

pub const LOCK_DIR: &str = "/run/cron-rs/locks";
pub const DEFAULT_LOCK_WAIT_SECS: u32 = 120;
pub const STAFF_API_HYPERF_SANDBOX: &str = "staff-api-hyperf";

/// Sanitize a lock key into a stable lock file name component.
/// Lock keys intentionally keep underscores, unlike systemd unit names.
pub fn safe_lock_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Resolve a task lock key to its shared flock path.
pub fn lock_path(key: &str) -> PathBuf {
    PathBuf::from(LOCK_DIR).join(format!("{}.lock", safe_lock_key(key)))
}

/// Ensure the volatile lock directory exists.
///
/// The sticky world-writable mode lets root-managed units and app-specific
/// service users share zero-byte lock files without requiring a deployment
/// specific cron-rs group.
pub fn ensure_lock_dir() -> anyhow::Result<()> {
    std::fs::create_dir_all(LOCK_DIR)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(LOCK_DIR, std::fs::Permissions::from_mode(0o1777))?;
    }

    Ok(())
}

pub fn supported_sandbox_profiles() -> &'static [&'static str] {
    &[STAFF_API_HYPERF_SANDBOX]
}

pub fn is_supported_sandbox_profile(profile: &str) -> bool {
    supported_sandbox_profiles().contains(&profile)
}

fn writable_db_dir(db_path: &str) -> String {
    Path::new(db_path)
        .parent()
        .unwrap_or_else(|| Path::new("/root/cron-rs"))
        .display()
        .to_string()
}

fn sandbox_profile_content(profile: &str, db_path: &str) -> Option<String> {
    match profile {
        STAFF_API_HYPERF_SANDBOX => Some(format!(
            "WorkingDirectory=/server/staff-api\n\
             NoNewPrivileges=true\n\
             PrivateTmp=true\n\
             PrivateDevices=true\n\
             ProtectSystem=strict\n\
             ProtectKernelTunables=true\n\
             ProtectKernelModules=true\n\
             ProtectControlGroups=true\n\
             ProtectClock=true\n\
             ProtectHostname=true\n\
             RestrictSUIDSGID=true\n\
             RestrictRealtime=true\n\
             LockPersonality=true\n\
             SystemCallArchitectures=native\n\
             ReadWritePaths={} {} /server/staff-api/runtime\n",
            writable_db_dir(db_path),
            LOCK_DIR
        )),
        _ => None,
    }
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
    generate_service_for_task(task, &binary_path, db_path)
}

/// Detect whether an OnCalendar expression already carries an explicit timezone
/// suffix. systemd accepts an optional zone as the last whitespace-separated
/// token (e.g. `*-*-* 09:00:00 America/Vancouver`). Region/city zones contain
/// `/`; the bare `UTC` alias is also recognized.
fn schedule_has_timezone(schedule: &str) -> bool {
    match schedule.split_whitespace().next_back() {
        Some(last) => last.contains('/') || last.eq_ignore_ascii_case("UTC"),
        None => false,
    }
}

/// Append `tz` to an OnCalendar expression so timers fire in that zone instead
/// of the host's system zone. No-op when `tz` is empty/whitespace or when the
/// schedule already carries a zone.
fn apply_timezone_with(schedule: &str, tz: &str) -> String {
    let tz = tz.trim();
    if tz.is_empty() || schedule_has_timezone(schedule) {
        schedule.to_string()
    } else {
        format!("{} {}", schedule, tz)
    }
}

/// Convenience wrapper that reads `CRON_RS_TIMEZONE` from the environment.
fn apply_timezone(schedule: &str) -> String {
    let tz = std::env::var("CRON_RS_TIMEZONE").unwrap_or_default();
    apply_timezone_with(schedule, &tz)
}

/// Generate a .timer unit file content from raw parameters.
pub fn generate_timer(task_name: &str, schedule: &str) -> String {
    let schedule = apply_timezone(schedule);
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
#[allow(dead_code)]
pub fn generate_service(
    task_name: &str,
    task_id: &str,
    binary_path: &str,
    db_path: &str,
) -> String {
    generate_service_with_options(task_name, task_id, binary_path, db_path, None, None)
}

/// Generate a .service unit file content for a task, including optional lock wrapping.
pub fn generate_service_for_task(task: &Task, binary_path: &str, db_path: &str) -> String {
    generate_service_with_options(
        &task.name,
        &task.id,
        binary_path,
        db_path,
        task.lock_key.as_deref(),
        task.sandbox_profile.as_deref(),
    )
}

/// Generate a .service unit file content from raw parameters with optional flock wrapping.
pub fn generate_service_with_options(
    task_name: &str,
    task_id: &str,
    binary_path: &str,
    db_path: &str,
    lock_key: Option<&str>,
    sandbox_profile: Option<&str>,
) -> String {
    let run_command = format!(
        "{binary_path} run --task-id {task_id} --task-name {task_name} --db-path {db_path}"
    );
    let exec_start = match lock_key {
        Some(key) if !key.trim().is_empty() => {
            let path = lock_path(key);
            format!(
                "/usr/bin/flock --exclusive --wait {DEFAULT_LOCK_WAIT_SECS} {} {run_command}",
                path.display()
            )
        }
        _ => run_command,
    };
    let sandbox = sandbox_profile
        .and_then(|profile| sandbox_profile_content(profile, db_path))
        .unwrap_or_default();

    format!(
        "[Unit]\n\
         Description=cron-rs task: {task_name}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={exec_start}\n\
         Environment=CRON_RS_DB={db_path}\n\
         {sandbox}\
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
    fn test_lock_path_sanitizes_key() {
        assert_eq!(
            lock_path("staff api/boot").display().to_string(),
            "/run/cron-rs/locks/staff_api_boot.lock"
        );
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
    fn test_schedule_has_timezone() {
        assert!(schedule_has_timezone("*-*-* 09:00:00 America/Vancouver"));
        assert!(schedule_has_timezone("Mon..Fri *-*-* 09:00:00 Europe/Berlin"));
        assert!(schedule_has_timezone("*-*-* 09:00:00 utc"));
        assert!(!schedule_has_timezone("*-*-* 09:00:00"));
        assert!(!schedule_has_timezone("Mon..Fri *-*-* 09:00:00"));
    }

    #[test]
    fn test_apply_timezone_with_appends() {
        assert_eq!(
            apply_timezone_with("*-*-* 09:00:00", "America/Vancouver"),
            "*-*-* 09:00:00 America/Vancouver"
        );
    }

    #[test]
    fn test_apply_timezone_with_skips_when_already_zoned() {
        assert_eq!(
            apply_timezone_with("*-*-* 09:00:00 Europe/Berlin", "America/Vancouver"),
            "*-*-* 09:00:00 Europe/Berlin"
        );
    }

    #[test]
    fn test_apply_timezone_with_empty_is_noop() {
        assert_eq!(apply_timezone_with("*-*-* 09:00:00", ""), "*-*-* 09:00:00");
        assert_eq!(
            apply_timezone_with("*-*-* 09:00:00", "   "),
            "*-*-* 09:00:00"
        );
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

    #[test]
    fn test_generate_service_with_lock() {
        let content = generate_service_with_options(
            "backup",
            "abc-123",
            "/usr/bin/cron-rs",
            "/home/user/cron-rs.db",
            Some("staff-api-boot"),
            None,
        );
        assert!(content.contains("ExecStart=/usr/bin/flock --exclusive --wait 120 /run/cron-rs/locks/staff-api-boot.lock /usr/bin/cron-rs run --task-id abc-123 --task-name backup --db-path /home/user/cron-rs.db"));
    }

    #[test]
    fn test_generate_service_with_staff_api_hyperf_sandbox() {
        let content = generate_service_with_options(
            "sync-patient",
            "abc-123",
            "/usr/bin/cron-rs",
            "/root/cron-rs/cron-rs.db",
            Some("staff-api-boot"),
            Some(STAFF_API_HYPERF_SANDBOX),
        );

        assert!(content.contains("WorkingDirectory=/server/staff-api"));
        assert!(content.contains("ProtectSystem=strict"));
        assert!(content
            .contains("ReadWritePaths=/root/cron-rs /run/cron-rs/locks /server/staff-api/runtime"));
    }
}

use crate::models::Task;
use std::collections::HashMap;
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

/// Split the single `H:M[:S]` token of a calendar expression into components.
/// Returns None when there is no time token, more than one, or it has more
/// than three `:`-separated parts — callers should then leave the schedule
/// alone rather than guess.
fn time_parts(schedule: &str) -> Option<(&str, &str, Option<&str>)> {
    let mut time_tokens = schedule.split_whitespace().filter(|t| t.contains(':'));
    let token = time_tokens.next()?;
    if time_tokens.next().is_some() {
        return None;
    }
    let mut parts = token.split(':');
    let hour = parts.next()?;
    let minute = parts.next()?;
    let second = parts.next();
    if parts.next().is_some() {
        return None;
    }
    Some((hour, minute, second))
}

/// True when a schedule fires every minute at second zero: the `minutely`
/// shorthand, or a calendar expression whose minute component is `*` and
/// whose seconds component is absent or zero. These are the timers that all
/// collide at :00 of every minute unless staggered.
pub fn is_every_minute_schedule(schedule: &str) -> bool {
    let schedule = schedule.trim();
    if schedule.eq_ignore_ascii_case("minutely") {
        return true;
    }
    match time_parts(schedule) {
        Some((_, minute, second)) => {
            minute == "*" && second.is_none_or(|s| !s.is_empty() && s.chars().all(|c| c == '0'))
        }
        None => false,
    }
}

/// Rewrite an every-minute schedule so it fires at `second` instead of `:00`.
/// Schedules that do not fire every minute are returned unchanged.
pub fn apply_stagger_second(schedule: &str, second: u8) -> String {
    if !is_every_minute_schedule(schedule) {
        return schedule.to_string();
    }
    let trimmed = schedule.trim();
    if trimmed.eq_ignore_ascii_case("minutely") {
        return format!("*-*-* *:*:{:02}", second);
    }
    let tokens: Vec<String> = trimmed
        .split_whitespace()
        .map(|token| {
            if !token.contains(':') {
                return token.to_string();
            }
            // is_every_minute_schedule guaranteed exactly one time token
            // with two or three components.
            let parts: Vec<&str> = token.split(':').collect();
            format!("{}:{}:{:02}", parts[0], parts[1], second)
        })
        .collect();
    tokens.join(" ")
}

/// Evenly-spread stagger seconds for every-minute tasks, keyed by task id.
///
/// Slot `i` of `n` is `(2i+1)*30/n`: slots are centered within the minute so
/// no task lands on second 0 (where hourly and every-N-minute timers fire)
/// while staying distinct for up to 60 tasks. Assignment is ordered by task
/// id, and disabled tasks keep their slot, so a task's offset only moves when
/// the every-minute group itself gains or loses a member.
pub fn stagger_assignments(tasks: &[Task]) -> HashMap<String, u8> {
    let mut ids: Vec<&str> = tasks
        .iter()
        .filter(|t| is_every_minute_schedule(&t.schedule))
        .map(|t| t.id.as_str())
        .collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.into_iter()
        .enumerate()
        .map(|(i, id)| (id.to_string(), (((2 * i + 1) * 30 / n).min(59)) as u8))
        .collect()
}

/// Stagger second for one task, when it is an every-minute task.
pub fn stagger_second_for(tasks: &[Task], task_id: &str) -> Option<u8> {
    stagger_assignments(tasks).get(task_id).copied()
}

/// Generate the content of a systemd .timer unit file for the given task.
#[allow(dead_code)]
pub fn generate_timer_unit(task: &Task, stagger_second: Option<u8>) -> String {
    generate_timer(&task.name, &task.schedule, stagger_second)
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
///
/// `stagger_second` (from [`stagger_assignments`]) shifts an every-minute
/// schedule onto its own second. `AccuracySec=1s` is required for the shift
/// to matter: systemd's default 1-minute accuracy window coalesces timer
/// wakeups, which is exactly what makes co-scheduled tasks fire together.
pub fn generate_timer(task_name: &str, schedule: &str, stagger_second: Option<u8>) -> String {
    let schedule = match stagger_second {
        Some(second) => apply_stagger_second(schedule, second),
        None => schedule.to_string(),
    };
    let schedule = apply_timezone(&schedule);
    format!(
        "[Unit]\n\
         Description=cron-rs timer: {task_name}\n\
         \n\
         [Timer]\n\
         OnCalendar={schedule}\n\
         AccuracySec=1s\n\
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
        let content = generate_timer("backup", "*-*-* 02:00:00", None);
        assert!(content.contains("Description=cron-rs timer: backup"));
        assert!(content.contains("OnCalendar=*-*-* 02:00:00"));
        assert!(content.contains("AccuracySec=1s"));
        assert!(content.contains("Persistent=true"));
        assert!(content.contains("WantedBy=timers.target"));
    }

    #[test]
    fn test_generate_timer_staggers_every_minute_schedule() {
        let content = generate_timer("sync", "*-*-* *:*:00", Some(26));
        assert!(content.contains("OnCalendar=*-*-* *:*:26"));
        assert!(content.contains("AccuracySec=1s"));
    }

    #[test]
    fn test_generate_timer_ignores_stagger_for_other_schedules() {
        let content = generate_timer("backup", "*-*-* 02:00:00", Some(26));
        assert!(content.contains("OnCalendar=*-*-* 02:00:00"));
    }

    #[test]
    fn test_is_every_minute_schedule() {
        assert!(is_every_minute_schedule("*-*-* *:*:00"));
        assert!(is_every_minute_schedule("minutely"));
        assert!(is_every_minute_schedule("Minutely"));
        assert!(is_every_minute_schedule("*:*"));
        assert!(is_every_minute_schedule("*:*:00"));
        assert!(is_every_minute_schedule("*-*-* 9..17:*:00"));
        assert!(is_every_minute_schedule("*-*-* *:*:00 Asia/Shanghai"));

        assert!(!is_every_minute_schedule("*-*-* *:0/5:00"));
        assert!(!is_every_minute_schedule("*-*-* 02:00:00"));
        assert!(!is_every_minute_schedule("*-*-* *:*:30"));
        assert!(!is_every_minute_schedule("*-*-* *:*:0/15"));
        assert!(!is_every_minute_schedule("hourly"));
        assert!(!is_every_minute_schedule("daily"));
        assert!(!is_every_minute_schedule(""));
    }

    #[test]
    fn test_apply_stagger_second() {
        assert_eq!(apply_stagger_second("*-*-* *:*:00", 26), "*-*-* *:*:26");
        assert_eq!(apply_stagger_second("minutely", 5), "*-*-* *:*:05");
        assert_eq!(apply_stagger_second("*:*", 7), "*:*:07");
        assert_eq!(
            apply_stagger_second("*-*-* *:*:00 Asia/Shanghai", 56),
            "*-*-* *:*:56 Asia/Shanghai"
        );
        // Non-every-minute schedules are untouched.
        assert_eq!(apply_stagger_second("*-*-* 02:00:00", 9), "*-*-* 02:00:00");
        assert_eq!(apply_stagger_second("*-*-* *:0/5:00", 9), "*-*-* *:0/5:00");
    }

    fn stagger_task(id: &str, schedule: &str, enabled: bool) -> Task {
        use crate::models::task::ConcurrencyPolicy;
        Task {
            id: id.to_string(),
            name: format!("task-{id}"),
            command: "true".to_string(),
            schedule: schedule.to_string(),
            tags: Vec::new(),
            description: String::new(),
            enabled,
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

    #[test]
    fn test_stagger_assignments_spread_and_membership() {
        let tasks = vec![
            stagger_task("a", "*-*-* *:*:00", true),
            stagger_task("b", "*-*-* *:*:00", false), // disabled still holds a slot
            stagger_task("c", "minutely", true),
            stagger_task("d", "*-*-* 02:00:00", true), // not every-minute
        ];
        let map = stagger_assignments(&tasks);
        assert_eq!(map.len(), 3);
        assert!(!map.contains_key("d"));
        // Three members, ordered by id: (2i+1)*30/3 = 10, 30, 50.
        assert_eq!(map["a"], 10);
        assert_eq!(map["b"], 30);
        assert_eq!(map["c"], 50);

        assert_eq!(stagger_second_for(&tasks, "c"), Some(50));
        assert_eq!(stagger_second_for(&tasks, "d"), None);
        assert_eq!(stagger_second_for(&tasks, "missing"), None);
    }

    #[test]
    fn test_stagger_assignments_distinct_up_to_sixty_tasks() {
        for n in 1..=60 {
            let tasks: Vec<Task> = (0..n)
                .map(|i| stagger_task(&format!("id-{i:03}"), "*-*-* *:*:00", true))
                .collect();
            let map = stagger_assignments(&tasks);
            let mut seconds: Vec<u8> = map.values().copied().collect();
            seconds.sort_unstable();
            seconds.dedup();
            assert_eq!(seconds.len(), n, "collision with {n} every-minute tasks");
            assert!(seconds.iter().all(|s| *s < 60));
        }
    }

    #[test]
    fn test_schedule_has_timezone() {
        assert!(schedule_has_timezone("*-*-* 09:00:00 America/Vancouver"));
        assert!(schedule_has_timezone(
            "Mon..Fri *-*-* 09:00:00 Europe/Berlin"
        ));
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

use std::path::PathBuf;

use crate::config::Config;
use crate::db;
use crate::systemd::unit_gen;

/// Run diagnostics on the cron-rs installation.
/// Checks: orphaned runs, unit file presence, binary path consistency.
pub async fn run_doctor() -> anyhow::Result<()> {
    let config = Config::load()?;

    println!("=== cron-rs doctor ===");
    println!();

    // 1. Open DB directly
    println!("Checking database at {} ...", config.db_path.display());
    let database = db::Database::new(&config.db_path).await?;
    let conn = database.connect().await?;
    println!("  Database connection: OK");

    // 2. Mark orphaned runs as crashed
    println!();
    println!("Checking for orphaned runs (status=running/retrying)...");
    let orphaned_count = db::runs::mark_orphaned_runs_crashed(&conn).await?;
    if orphaned_count > 0 {
        println!("  Marked {} orphaned run(s) as crashed.", orphaned_count);
    } else {
        println!("  No orphaned runs found.");
    }

    // 3. Check unit files
    println!();
    println!("Checking systemd unit files...");
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/root"));
    let unit_dir = PathBuf::from(&home)
        .join(".config")
        .join("systemd")
        .join("user");

    let tasks = db::tasks::list(&conn).await.unwrap_or_default();
    let mut missing_units = 0;
    let mut binary_mismatches = 0;
    let mut lock_warnings = 0;

    let current_binary = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "cron-rs".to_string());

    for task in &tasks {
        let timer_path = unit_dir.join(unit_gen::timer_filename(&task.name));
        let service_path = unit_dir.join(unit_gen::service_filename(&task.name));

        if !timer_path.exists() {
            println!(
                "  WARNING: Timer file missing for task '{}': {}",
                task.name,
                timer_path.display()
            );
            missing_units += 1;
        }

        if !service_path.exists() {
            println!(
                "  WARNING: Service file missing for task '{}': {}",
                task.name,
                service_path.display()
            );
            missing_units += 1;
        } else {
            // Check binary path in service file
            if let Ok(content) = std::fs::read_to_string(&service_path) {
                if !content.contains(&current_binary) {
                    println!(
                        "  WARNING: Binary path mismatch in service for task '{}' (expected '{}')",
                        task.name, current_binary
                    );
                    binary_mismatches += 1;
                }
                if let Some(lock_key) = &task.lock_key {
                    let lock_path = unit_gen::lock_path(lock_key).display().to_string();
                    if !content.contains("/usr/bin/flock") || !content.contains(&lock_path) {
                        println!(
                            "  WARNING: Lock key '{}' is set for task '{}' but its service is not flock-wrapped",
                            lock_key, task.name
                        );
                        lock_warnings += 1;
                    }
                }
                if let Some(profile) = &task.sandbox_profile {
                    if profile == unit_gen::STAFF_API_HYPERF_SANDBOX {
                        for expected in [
                            "WorkingDirectory=/server/staff-api",
                            "NoNewPrivileges=true",
                            "ProtectSystem=strict",
                            "ReadWritePaths=",
                        ] {
                            if !content.contains(expected) {
                                println!(
                                    "  WARNING: Sandbox profile '{}' is set for task '{}' but generated service is missing '{}'",
                                    profile, task.name, expected
                                );
                                lock_warnings += 1;
                            }
                        }
                    } else {
                        println!(
                            "  WARNING: Task '{}' uses unsupported sandbox_profile '{}'",
                            task.name, profile
                        );
                        lock_warnings += 1;
                    }
                }
            }
        }

        if task.command.contains("bin/hyperf.php") && task.lock_key.is_none() {
            println!(
                "  WARNING: Hyperf task '{}' has no lock_key; set a shared lock key to avoid scan-cache races",
                task.name
            );
            lock_warnings += 1;
        }
        if task.command.contains("bin/hyperf.php") && task.sandbox_profile.is_none() {
            println!(
                "  WARNING: Hyperf task '{}' has no sandbox_profile; set '{}' to restrict filesystem writes",
                task.name,
                unit_gen::STAFF_API_HYPERF_SANDBOX
            );
            lock_warnings += 1;
        }
    }

    if missing_units == 0 && binary_mismatches == 0 && lock_warnings == 0 {
        println!("  All {} task(s) have valid unit files.", tasks.len());
    }

    if tasks
        .iter()
        .any(|task| task.lock_key.as_deref() == Some("staff-api-boot"))
    {
        let staff_api_unit = std::process::Command::new("systemctl")
            .arg("cat")
            .arg("staff-api.service")
            .output();
        match staff_api_unit {
            Ok(output) if output.status.success() => {
                let content = String::from_utf8_lossy(&output.stdout);
                let lock_path = unit_gen::lock_path("staff-api-boot").display().to_string();
                if !content.contains("ExecStartPre=/usr/bin/flock") || !content.contains(&lock_path)
                {
                    println!(
                        "  WARNING: staff-api.service does not appear to have the staff-api-boot ExecStartPre flock"
                    );
                    lock_warnings += 1;
                }
            }
            _ => {
                println!(
                    "  WARNING: Could not inspect staff-api.service for staff-api-boot companion flock"
                );
                lock_warnings += 1;
            }
        }
    }

    // 4. Check systemctl availability
    println!();
    println!("Checking systemctl...");
    match std::process::Command::new("systemctl")
        .arg("--user")
        .arg("--version")
        .output()
    {
        Ok(output) if output.status.success() => {
            println!("  systemctl: OK");
        }
        _ => {
            println!("  WARNING: systemctl --user not available or not working");
        }
    }

    // 5. Summary
    println!();
    let mut issues = 0;
    if orphaned_count > 0 {
        issues += 1;
    }
    if missing_units > 0 {
        issues += 1;
    }
    if binary_mismatches > 0 {
        issues += 1;
    }
    if lock_warnings > 0 {
        issues += 1;
    }

    if issues == 0 {
        println!("No issues found.");
    } else {
        println!(
            "Found {} issue(s). Run `cron-rs regenerate` to fix unit file problems.",
            issues
        );
    }

    Ok(())
}

/// Regenerate all systemd unit files from the current DB state.
pub async fn run_regenerate(rewrite_all: bool) -> anyhow::Result<()> {
    let config = Config::load()?;

    println!("=== cron-rs regenerate ===");
    println!();
    if rewrite_all {
        println!("Rewrite all requested; cron-rs regenerate always rewrites every unit.");
        println!();
    }

    // Open DB directly
    let database = db::Database::new(&config.db_path).await?;
    let conn = database.connect().await?;

    let tasks = db::tasks::list(&conn).await?;
    if tasks.is_empty() {
        println!("No tasks found in database.");
        return Ok(());
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/root"));
    let unit_dir = PathBuf::from(&home)
        .join(".config")
        .join("systemd")
        .join("user");

    // Ensure the unit directory exists
    std::fs::create_dir_all(&unit_dir)?;

    let current_binary = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "cron-rs".to_string());
    let db_path_str = config.db_path.to_string_lossy().to_string();

    println!("Regenerating unit files for {} task(s)...", tasks.len());

    if tasks
        .iter()
        .any(|task| task.lock_key.is_some() || task.sandbox_profile.is_some())
    {
        unit_gen::ensure_lock_dir()?;
    }

    for task in &tasks {
        let timer_content = unit_gen::generate_timer(&task.name, &task.schedule);
        let service_content =
            unit_gen::generate_service_for_task(task, &current_binary, &db_path_str);

        let timer_path = unit_dir.join(unit_gen::timer_filename(&task.name));
        let service_path = unit_dir.join(unit_gen::service_filename(&task.name));

        std::fs::write(&timer_path, &timer_content)?;
        std::fs::write(&service_path, &service_content)?;

        println!(
            "  Wrote: {} and {}",
            timer_path.display(),
            service_path.display()
        );
    }

    // Reload systemd daemon
    println!();
    println!("Reloading systemd daemon...");
    let output = std::process::Command::new("systemctl")
        .arg("--user")
        .arg("daemon-reload")
        .output();

    match output {
        Ok(o) if o.status.success() => {
            println!("  daemon-reload: OK");
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprintln!("  WARNING: daemon-reload failed: {}", stderr.trim());
        }
        Err(e) => {
            eprintln!("  WARNING: Could not run systemctl: {}", e);
        }
    }

    println!();
    println!(
        "Regeneration complete. {} unit file(s) written.",
        tasks.len() * 2
    );

    Ok(())
}

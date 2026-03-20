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
            }
        }
    }

    if missing_units == 0 && binary_mismatches == 0 {
        println!("  All {} task(s) have valid unit files.", tasks.len());
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
pub async fn run_regenerate() -> anyhow::Result<()> {
    let config = Config::load()?;

    println!("=== cron-rs regenerate ===");
    println!();

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

    println!(
        "Regenerating unit files for {} task(s)...",
        tasks.len()
    );

    for task in &tasks {
        let timer_content = unit_gen::generate_timer(&task.name, &task.schedule);
        let service_content =
            unit_gen::generate_service(&task.name, &task.id, &current_binary, &db_path_str);

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
    println!("Regeneration complete. {} unit file(s) written.", tasks.len() * 2);

    Ok(())
}

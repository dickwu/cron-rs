use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::process::Command;

use crate::cli::ServiceCommands;
use crate::config::Config;
use crate::systemd::unit_gen;

pub async fn handle_service_command(command: ServiceCommands) -> Result<()> {
    match command {
        ServiceCommands::Install { host, port, start } => install_service(host, port, start).await,
        ServiceCommands::Uninstall => uninstall_service().await,
        ServiceCommands::Start => {
            systemctl_checked(&["start", unit_gen::daemon_service_filename()]).await
        }
        ServiceCommands::Stop => {
            systemctl_checked(&["stop", unit_gen::daemon_service_filename()]).await
        }
        ServiceCommands::Restart => {
            systemctl_checked(&["restart", unit_gen::daemon_service_filename()]).await
        }
        ServiceCommands::Status => status_service().await,
    }
}

async fn install_service(host: Option<String>, port: Option<u16>, start: bool) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(host) = host {
        config.host = host;
    }
    if let Some(port) = port {
        config.port = port;
    }

    let unit_dir = user_unit_dir()?;
    let unit_path = unit_dir.join(unit_gen::daemon_service_filename());
    let binary_path = std::env::current_exe()
        .context("failed to resolve current executable path")?
        .to_string_lossy()
        .to_string();
    let config_dir = config.config_dir.to_string_lossy().to_string();
    let db_path = config.db_path.to_string_lossy().to_string();
    let content = unit_gen::generate_daemon_service(
        &binary_path,
        &config.host,
        config.port,
        &config_dir,
        &db_path,
    );

    tokio::fs::write(&unit_path, content)
        .await
        .with_context(|| format!("failed to write service unit: {}", unit_path.display()))?;
    println!("Wrote {}", unit_path.display());

    systemctl_checked(&["daemon-reload"]).await?;

    if start {
        systemctl_checked(&["enable", unit_gen::daemon_service_filename()]).await?;
        systemctl_checked(&["restart", unit_gen::daemon_service_filename()]).await?;
        println!(
            "Installed and started {}",
            unit_gen::daemon_service_filename()
        );
    } else {
        systemctl_checked(&["enable", unit_gen::daemon_service_filename()]).await?;
        println!("Installed {}", unit_gen::daemon_service_filename());
    }

    Ok(())
}

async fn uninstall_service() -> Result<()> {
    let unit_path = user_unit_dir()?.join(unit_gen::daemon_service_filename());

    let _ = systemctl_allow_fail(&["disable", "--now", unit_gen::daemon_service_filename()]).await;

    if unit_path.exists() {
        tokio::fs::remove_file(&unit_path)
            .await
            .with_context(|| format!("failed to remove service unit: {}", unit_path.display()))?;
        println!("Removed {}", unit_path.display());
    }

    systemctl_checked(&["daemon-reload"]).await?;
    Ok(())
}

async fn status_service() -> Result<()> {
    let output =
        systemctl_allow_fail(&["status", "--no-pager", unit_gen::daemon_service_filename()])
            .await?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    Ok(())
}

fn user_unit_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/root"));
    let unit_dir = PathBuf::from(home)
        .join(".config")
        .join("systemd")
        .join("user");
    std::fs::create_dir_all(&unit_dir).with_context(|| {
        format!(
            "failed to create systemd user unit dir: {}",
            unit_dir.display()
        )
    })?;
    Ok(unit_dir)
}

async fn systemctl_checked(args: &[&str]) -> Result<()> {
    let output = systemctl_allow_fail(args).await?;
    if !output.status.success() {
        anyhow::bail!(
            "systemctl --user {} failed with status {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn systemctl_allow_fail(args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .await
        .context("failed to execute systemctl")?;
    Ok(output)
}

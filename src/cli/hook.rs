use std::io::{self, Write};
use std::path::PathBuf;

use crate::cli::HookCommands;
use crate::config::Config;
use crate::db;

/// Build base URL and a reqwest client.
fn api_client(config: &Config) -> (String, reqwest::Client) {
    let base_url = format!("http://{}:{}", config.host, config.port);
    let client = reqwest::Client::new();
    (base_url, client)
}

/// Path to the token file.
fn token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/root"));
    PathBuf::from(home)
        .join(".config")
        .join("cron-rs")
        .join("token")
}

/// Read the saved token, or None if not available.
fn read_token() -> Option<String> {
    std::fs::read_to_string(token_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Save a token to disk.
fn save_token(token: &str) -> anyhow::Result<()> {
    let path = token_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, token)?;
    Ok(())
}

/// Ensure we have a valid token.
async fn ensure_token(base_url: &str, client: &reqwest::Client) -> anyhow::Result<String> {
    if let Some(token) = read_token() {
        let resp = client
            .get(format!("{}/api/v1/tasks", base_url))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;
        match resp {
            Ok(r) if r.status() != reqwest::StatusCode::UNAUTHORIZED => {
                return Ok(token);
            }
            _ => {}
        }
    }

    print!("Username: ");
    io::stdout().flush()?;
    let mut username = String::new();
    io::stdin().read_line(&mut username)?;
    let username = username.trim().to_string();

    print!("Password: ");
    io::stdout().flush()?;
    let mut password = String::new();
    io::stdin().read_line(&mut password)?;
    let password = password.trim().to_string();

    let resp = client
        .post(format!("{}/api/v1/auth/login", base_url))
        .json(&serde_json::json!({
            "username": username,
            "password": password,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Login failed: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    let body: serde_json::Value = resp.json().await?;
    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow::anyhow!("No token in login response"))?
        .to_string();

    save_token(&token)?;
    Ok(token)
}

/// Resolve a task name/id to a task id.
async fn resolve_task_id(
    base_url: &str,
    client: &reqwest::Client,
    token: &str,
    name_or_id: &str,
    config: &Config,
) -> anyhow::Result<String> {
    if uuid::Uuid::parse_str(name_or_id).is_ok() {
        return Ok(name_or_id.to_string());
    }

    let resp = client
        .get(format!("{}/api/v1/tasks", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let tasks: Vec<serde_json::Value> = r.json().await?;
            for task in &tasks {
                if task.get("name").and_then(|n| n.as_str()) == Some(name_or_id) {
                    if let Some(id) = task.get("id").and_then(|i| i.as_str()) {
                        return Ok(id.to_string());
                    }
                }
            }
            anyhow::bail!("Task '{}' not found", name_or_id);
        }
        _ => {
            let database = db::Database::new(&config.db_path).await?;
            let conn = database.connect().await?;
            match db::tasks::get_by_name(&conn, name_or_id).await {
                Ok(task) => Ok(task.id),
                Err(_) => anyhow::bail!("Task '{}' not found", name_or_id),
            }
        }
    }
}

/// Format hooks as a table.
fn print_hook_table(hooks: &[serde_json::Value]) {
    if hooks.is_empty() {
        println!("No hooks found.");
        return;
    }

    println!(
        "{:<36}  {:<20}  {:<10}  {:<10}  {}",
        "ID", "TYPE", "ORDER", "TIMEOUT", "COMMAND"
    );
    println!("{}", "-".repeat(110));

    for hook in hooks {
        let id = hook.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let hook_type = hook.get("hook_type").and_then(|v| v.as_str()).unwrap_or("-");
        let run_order = hook
            .get("run_order")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let timeout = hook
            .get("timeout_secs")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "none".to_string());
        let command = hook.get("command").and_then(|v| v.as_str()).unwrap_or("-");

        let command_display = if command.len() > 40 {
            format!("{}...", &command[..37])
        } else {
            command.to_string()
        };

        println!(
            "{:<36}  {:<20}  {:<10}  {:<10}  {}",
            id, hook_type, run_order, timeout, command_display
        );
    }
}

/// Dispatch hook subcommands.
pub async fn handle_hook_command(cmd: HookCommands) -> anyhow::Result<()> {
    let config = Config::load()?;

    match cmd {
        HookCommands::Add {
            task,
            on,
            command,
            timeout_secs,
            run_order,
        } => hook_add(&config, &task, &on, &command, timeout_secs, run_order).await,
        HookCommands::List { task } => hook_list(&config, &task).await,
        HookCommands::Remove { id } => hook_remove(&config, &id).await,
    }
}

/// POST /tasks/:id/hooks — add a hook.
async fn hook_add(
    config: &Config,
    task: &str,
    on: &str,
    command: &str,
    timeout_secs: Option<i32>,
    run_order: Option<i32>,
) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;
    let task_id = resolve_task_id(&base_url, &client, &token, task, config).await?;

    let mut body = serde_json::json!({
        "hook_type": on,
        "command": command,
    });

    if let Some(ts) = timeout_secs {
        body["timeout_secs"] = serde_json::json!(ts);
    }
    if let Some(ro) = run_order {
        body["run_order"] = serde_json::json!(ro);
    }

    let resp = client
        .post(format!("{}/api/v1/tasks/{}/hooks", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await?;

    if resp.status().is_success() {
        let hook: serde_json::Value = resp.json().await?;
        println!("Hook created successfully.");
        println!(
            "  ID:      {}",
            hook.get("id").and_then(|v| v.as_str()).unwrap_or("-")
        );
        println!(
            "  Type:    {}",
            hook.get("hook_type").and_then(|v| v.as_str()).unwrap_or("-")
        );
        println!(
            "  Command: {}",
            hook.get("command").and_then(|v| v.as_str()).unwrap_or("-")
        );
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to add hook: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

/// GET /tasks/:id/hooks — list hooks for a task.
async fn hook_list(config: &Config, task: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token_str = read_token().unwrap_or_default();

    // Resolve task id
    let task_id = resolve_task_id(&base_url, &client, &token_str, task, config).await?;

    let resp = client
        .get(format!("{}/api/v1/tasks/{}/hooks", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token_str))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let hooks: Vec<serde_json::Value> = r.json().await?;
            print_hook_table(&hooks);
            Ok(())
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let token = ensure_token(&base_url, &client).await?;
            let resp = client
                .get(format!("{}/api/v1/tasks/{}/hooks", base_url, task_id))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await?;
            if resp.status().is_success() {
                let hooks: Vec<serde_json::Value> = resp.json().await?;
                print_hook_table(&hooks);
            } else {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                eprintln!(
                    "Error: {}",
                    body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
                );
            }
            Ok(())
        }
        _ => {
            // Fallback to direct DB
            eprintln!("(daemon not reachable, reading directly from database)");
            let database = db::Database::new(&config.db_path).await?;
            let conn = database.connect().await?;
            let hooks = db::hooks::list_for_task(&conn, &task_id).await?;
            let json_hooks: Vec<serde_json::Value> = hooks
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "id": h.id,
                        "task_id": h.task_id,
                        "hook_type": h.hook_type.to_string(),
                        "command": h.command,
                        "timeout_secs": h.timeout_secs,
                        "run_order": h.run_order,
                        "created_at": h.created_at,
                    })
                })
                .collect();
            print_hook_table(&json_hooks);
            Ok(())
        }
    }
}

/// DELETE /hooks/:id — remove a hook.
async fn hook_remove(config: &Config, hook_id: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;

    let resp = client
        .delete(format!("{}/api/v1/hooks/{}", base_url, hook_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if resp.status().is_success() || resp.status() == reqwest::StatusCode::NO_CONTENT {
        println!("Hook '{}' removed.", hook_id);
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to remove hook: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

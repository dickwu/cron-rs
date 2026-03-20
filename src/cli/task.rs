use std::io::{self, Write};
use std::path::PathBuf;

use crate::cli::TaskCommands;
use crate::config::Config;
use crate::db;
use crate::models::Task;

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
    std::fs::read_to_string(token_path()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
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

/// Ensure we have a valid token. If not, prompt for login.
async fn ensure_token(base_url: &str, client: &reqwest::Client) -> anyhow::Result<String> {
    if let Some(token) = read_token() {
        // Try a quick health check with the token to see if it's valid
        let resp = client
            .get(format!("{}/api/v1/tasks", base_url))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;
        match resp {
            Ok(r) if r.status() != reqwest::StatusCode::UNAUTHORIZED => {
                return Ok(token);
            }
            _ => {
                // Token invalid or expired, re-login
            }
        }
    }

    // Prompt for credentials
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

/// Resolve a name_or_id to a task id. If it parses as UUID, use directly;
/// otherwise look up by name via the API. Falls back to DB if API is down.
async fn resolve_task_id(
    base_url: &str,
    client: &reqwest::Client,
    token: &str,
    name_or_id: &str,
    config: &Config,
) -> anyhow::Result<String> {
    // Check if it looks like a UUID
    if uuid::Uuid::parse_str(name_or_id).is_ok() {
        return Ok(name_or_id.to_string());
    }

    // Try API first
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
            // Fallback to direct DB
            let database = db::Database::new(&config.db_path).await?;
            let conn = database.connect().await?;
            match db::tasks::get_by_name(&conn, name_or_id).await {
                Ok(task) => Ok(task.id),
                Err(_) => anyhow::bail!("Task '{}' not found", name_or_id),
            }
        }
    }
}

/// Format a list of tasks as a table.
fn print_task_table(tasks: &[serde_json::Value]) {
    if tasks.is_empty() {
        println!("No tasks found.");
        return;
    }

    println!(
        "{:<36}  {:<20}  {:<8}  {:<25}  {}",
        "ID", "NAME", "ENABLED", "SCHEDULE", "COMMAND"
    );
    println!("{}", "-".repeat(120));

    for task in tasks {
        let id = task.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let name = task.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let enabled = task.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let schedule = task.get("schedule").and_then(|v| v.as_str()).unwrap_or("-");
        let command = task.get("command").and_then(|v| v.as_str()).unwrap_or("-");

        let enabled_str = if enabled { "yes" } else { "no" };
        let command_display = if command.len() > 40 {
            format!("{}...", &command[..37])
        } else {
            command.to_string()
        };

        println!(
            "{:<36}  {:<20}  {:<8}  {:<25}  {}",
            id, name, enabled_str, schedule, command_display
        );
    }
}

/// Format a single task for detailed display.
fn print_task_detail(task: &serde_json::Value) {
    println!("Task Details:");
    println!("  ID:                 {}", task.get("id").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Name:               {}", task.get("name").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Command:            {}", task.get("command").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Schedule:           {}", task.get("schedule").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Description:        {}", task.get("description").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Enabled:            {}", task.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false));
    println!("  Max Retries:        {}", task.get("max_retries").and_then(|v| v.as_i64()).unwrap_or(0));
    println!("  Retry Delay (s):    {}", task.get("retry_delay_secs").and_then(|v| v.as_i64()).unwrap_or(0));
    let timeout = task.get("timeout_secs").and_then(|v| v.as_i64());
    println!("  Timeout (s):        {}", timeout.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()));
    println!("  Concurrency Policy: {}", task.get("concurrency_policy").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Created At:         {}", task.get("created_at").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Updated At:         {}", task.get("updated_at").and_then(|v| v.as_str()).unwrap_or("-"));
}

/// Convert a DB Task to a serde_json::Value for uniform display.
fn task_to_json(task: &Task) -> serde_json::Value {
    serde_json::json!({
        "id": task.id,
        "name": task.name,
        "command": task.command,
        "schedule": task.schedule,
        "description": task.description,
        "enabled": task.enabled,
        "max_retries": task.max_retries,
        "retry_delay_secs": task.retry_delay_secs,
        "timeout_secs": task.timeout_secs,
        "concurrency_policy": task.concurrency_policy.to_string(),
        "created_at": task.created_at,
        "updated_at": task.updated_at,
    })
}

/// Dispatch task subcommands.
pub async fn handle_task_command(cmd: TaskCommands) -> anyhow::Result<()> {
    let config = Config::load()?;

    match cmd {
        TaskCommands::List => task_list(&config).await,
        TaskCommands::Create {
            name,
            command,
            schedule,
            description,
            max_retries,
            retry_delay_secs,
            timeout_secs,
            concurrency_policy,
        } => {
            task_create(
                &config,
                &name,
                &command,
                &schedule,
                description.as_deref(),
                max_retries,
                retry_delay_secs,
                timeout_secs,
                concurrency_policy.as_deref(),
            )
            .await
        }
        TaskCommands::Show { name_or_id } => task_show(&config, &name_or_id).await,
        TaskCommands::Edit {
            name_or_id,
            command,
            schedule,
            description,
            max_retries,
            retry_delay_secs,
            timeout_secs,
            concurrency_policy,
        } => {
            task_edit(
                &config,
                &name_or_id,
                command.as_deref(),
                schedule.as_deref(),
                description.as_deref(),
                max_retries,
                retry_delay_secs,
                timeout_secs,
                concurrency_policy.as_deref(),
            )
            .await
        }
        TaskCommands::Delete { name_or_id } => task_delete(&config, &name_or_id).await,
        TaskCommands::Enable { name_or_id } => task_enable(&config, &name_or_id).await,
        TaskCommands::Disable { name_or_id } => task_disable(&config, &name_or_id).await,
        TaskCommands::Trigger { name_or_id } => task_trigger(&config, &name_or_id).await,
    }
}

/// GET /tasks — list all tasks. Falls back to direct DB read if API is down.
async fn task_list(config: &Config) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);

    // Try API first
    let token = read_token().unwrap_or_default();
    let resp = client
        .get(format!("{}/api/v1/tasks", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let tasks: Vec<serde_json::Value> = r.json().await?;
            print_task_table(&tasks);
            Ok(())
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            // Need to login
            let token = ensure_token(&base_url, &client).await?;
            let resp = client
                .get(format!("{}/api/v1/tasks", base_url))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await?;
            if resp.status().is_success() {
                let tasks: Vec<serde_json::Value> = resp.json().await?;
                print_task_table(&tasks);
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
            // Fallback to direct DB read
            eprintln!("(daemon not reachable, reading directly from database)");
            let database = db::Database::new(&config.db_path).await?;
            let conn = database.connect().await?;
            let tasks = db::tasks::list(&conn).await?;
            let json_tasks: Vec<serde_json::Value> = tasks.iter().map(task_to_json).collect();
            print_task_table(&json_tasks);
            Ok(())
        }
    }
}

/// POST /tasks — create a new task.
#[allow(clippy::too_many_arguments)]
async fn task_create(
    config: &Config,
    name: &str,
    command: &str,
    schedule: &str,
    description: Option<&str>,
    max_retries: Option<i32>,
    retry_delay_secs: Option<i32>,
    timeout_secs: Option<i32>,
    concurrency_policy: Option<&str>,
) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;

    let mut body = serde_json::json!({
        "name": name,
        "command": command,
        "schedule": schedule,
    });

    if let Some(desc) = description {
        body["description"] = serde_json::json!(desc);
    }
    if let Some(mr) = max_retries {
        body["max_retries"] = serde_json::json!(mr);
    }
    if let Some(rd) = retry_delay_secs {
        body["retry_delay_secs"] = serde_json::json!(rd);
    }
    if let Some(ts) = timeout_secs {
        body["timeout_secs"] = serde_json::json!(ts);
    }
    if let Some(cp) = concurrency_policy {
        body["concurrency_policy"] = serde_json::json!(cp);
    }

    let resp = client
        .post(format!("{}/api/v1/tasks", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await?;

    if resp.status().is_success() {
        let task: serde_json::Value = resp.json().await?;
        println!("Task created successfully.");
        print_task_detail(&task);
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to create task: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

/// GET /tasks/:id — show a task. Falls back to DB if API is down.
async fn task_show(config: &Config, name_or_id: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token_str = read_token().unwrap_or_default();

    // Try to resolve the id
    let task_id = match resolve_task_id(&base_url, &client, &token_str, name_or_id, config).await {
        Ok(id) => id,
        Err(e) => {
            return Err(e);
        }
    };

    // Try API first
    let resp = client
        .get(format!("{}/api/v1/tasks/{}", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token_str))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let task: serde_json::Value = r.json().await?;
            print_task_detail(&task);
            Ok(())
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let token = ensure_token(&base_url, &client).await?;
            let resp = client
                .get(format!("{}/api/v1/tasks/{}", base_url, task_id))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await?;
            if resp.status().is_success() {
                let task: serde_json::Value = resp.json().await?;
                print_task_detail(&task);
            } else {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                anyhow::bail!(
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
            let task = if uuid::Uuid::parse_str(&task_id).is_ok() {
                db::tasks::get_by_id(&conn, &task_id).await
            } else {
                db::tasks::get_by_name(&conn, name_or_id).await
            };
            match task {
                Ok(t) => {
                    let json_task = task_to_json(&t);
                    print_task_detail(&json_task);
                    Ok(())
                }
                Err(db::helpers::DbError::NotFound) => {
                    anyhow::bail!("Task '{}' not found", name_or_id)
                }
                Err(e) => Err(e.into()),
            }
        }
    }
}

/// PUT /tasks/:id — edit a task.
#[allow(clippy::too_many_arguments)]
async fn task_edit(
    config: &Config,
    name_or_id: &str,
    command: Option<&str>,
    schedule: Option<&str>,
    description: Option<&str>,
    max_retries: Option<i32>,
    retry_delay_secs: Option<i32>,
    timeout_secs: Option<i32>,
    concurrency_policy: Option<&str>,
) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;
    let task_id = resolve_task_id(&base_url, &client, &token, name_or_id, config).await?;

    let mut body = serde_json::Map::new();
    if let Some(cmd) = command {
        body.insert("command".to_string(), serde_json::json!(cmd));
    }
    if let Some(sched) = schedule {
        body.insert("schedule".to_string(), serde_json::json!(sched));
    }
    if let Some(desc) = description {
        body.insert("description".to_string(), serde_json::json!(desc));
    }
    if let Some(mr) = max_retries {
        body.insert("max_retries".to_string(), serde_json::json!(mr));
    }
    if let Some(rd) = retry_delay_secs {
        body.insert("retry_delay_secs".to_string(), serde_json::json!(rd));
    }
    if let Some(ts) = timeout_secs {
        body.insert("timeout_secs".to_string(), serde_json::json!(ts));
    }
    if let Some(cp) = concurrency_policy {
        body.insert("concurrency_policy".to_string(), serde_json::json!(cp));
    }

    let resp = client
        .put(format!("{}/api/v1/tasks/{}", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::Value::Object(body))
        .send()
        .await?;

    if resp.status().is_success() {
        let task: serde_json::Value = resp.json().await?;
        println!("Task updated successfully.");
        print_task_detail(&task);
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to update task: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

/// DELETE /tasks/:id — delete a task.
async fn task_delete(config: &Config, name_or_id: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;
    let task_id = resolve_task_id(&base_url, &client, &token, name_or_id, config).await?;

    let resp = client
        .delete(format!("{}/api/v1/tasks/{}", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if resp.status().is_success() || resp.status() == reqwest::StatusCode::NO_CONTENT {
        println!("Task '{}' deleted.", name_or_id);
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to delete task: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

/// POST /tasks/:id/enable — enable a task.
async fn task_enable(config: &Config, name_or_id: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;
    let task_id = resolve_task_id(&base_url, &client, &token, name_or_id, config).await?;

    let resp = client
        .post(format!("{}/api/v1/tasks/{}/enable", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if resp.status().is_success() {
        println!("Task '{}' enabled.", name_or_id);
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to enable task: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

/// POST /tasks/:id/disable — disable a task.
async fn task_disable(config: &Config, name_or_id: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;
    let task_id = resolve_task_id(&base_url, &client, &token, name_or_id, config).await?;

    let resp = client
        .post(format!("{}/api/v1/tasks/{}/disable", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if resp.status().is_success() {
        println!("Task '{}' disabled.", name_or_id);
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to disable task: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

/// POST /tasks/:id/trigger — trigger a task to run immediately.
async fn task_trigger(config: &Config, name_or_id: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token = ensure_token(&base_url, &client).await?;
    let task_id = resolve_task_id(&base_url, &client, &token, name_or_id, config).await?;

    let resp = client
        .post(format!("{}/api/v1/tasks/{}/trigger", base_url, task_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;

    if resp.status().is_success() || resp.status() == reqwest::StatusCode::ACCEPTED {
        println!("Task '{}' triggered.", name_or_id);
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "Failed to trigger task: {}",
            body.get("error").and_then(|e| e.as_str()).unwrap_or("unknown error")
        );
    }

    Ok(())
}

/// GET /status — show system status. Falls back to DB if API is down.
pub async fn show_status(config: &Config) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token_str = read_token().unwrap_or_default();

    let resp = client
        .get(format!("{}/api/v1/status", base_url))
        .header("Authorization", format!("Bearer {}", token_str))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let status: serde_json::Value = r.json().await?;
            println!("=== cron-rs status ===");
            println!(
                "  Tasks:              {}",
                status.get("task_count").and_then(|v| v.as_i64()).unwrap_or(0)
            );
            println!(
                "  Active Timers:      {}",
                status.get("active_timers").and_then(|v| v.as_i64()).unwrap_or(0)
            );
            println!(
                "  Failures (24h):     {}",
                status
                    .get("recent_failures_24h")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
            );
            Ok(())
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let token = ensure_token(&base_url, &client).await?;
            let resp = client
                .get(format!("{}/api/v1/status", base_url))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await?;
            if resp.status().is_success() {
                let status: serde_json::Value = resp.json().await?;
                println!("=== cron-rs status ===");
                println!(
                    "  Tasks:              {}",
                    status.get("task_count").and_then(|v| v.as_i64()).unwrap_or(0)
                );
                println!(
                    "  Active Timers:      {}",
                    status.get("active_timers").and_then(|v| v.as_i64()).unwrap_or(0)
                );
                println!(
                    "  Failures (24h):     {}",
                    status
                        .get("recent_failures_24h")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0)
                );
            } else {
                eprintln!("Failed to get status from API");
            }
            Ok(())
        }
        _ => {
            // Fallback to direct DB read
            eprintln!("(daemon not reachable, reading directly from database)");
            let database = db::Database::new(&config.db_path).await?;
            let conn = database.connect().await?;

            let tasks = db::tasks::list(&conn).await.unwrap_or_default();
            let enabled_count = tasks.iter().filter(|t| t.enabled).count();

            let failures = db::runs::list_job_runs(&conn, None, Some("failed"), Some(1000), Some(0))
                .await
                .unwrap_or_default();
            let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
            let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();
            let recent_failures = failures
                .iter()
                .filter(|r| r.started_at >= cutoff_str)
                .count();

            println!("=== cron-rs status ===");
            println!("  Tasks:              {}", tasks.len());
            println!("  Enabled Tasks:      {}", enabled_count);
            println!("  Failures (24h):     {}", recent_failures);
            println!("  (daemon offline, timer status unavailable)");
            Ok(())
        }
    }
}

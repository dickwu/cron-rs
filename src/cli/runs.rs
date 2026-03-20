use std::io::{self, Write};
use std::path::PathBuf;

use crate::cli::RunsCommands;
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

/// Read the saved token.
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

/// Resolve a task name to a task_id via API (or DB fallback).
async fn resolve_task_id_for_filter(
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

/// Format runs as a table.
fn print_runs_table(runs: &[serde_json::Value]) {
    if runs.is_empty() {
        println!("No runs found.");
        return;
    }

    println!(
        "{:<36}  {:<36}  {:<10}  {:<20}  {:<10}  {:<8}",
        "ID", "TASK_ID", "STATUS", "STARTED", "EXIT", "DURATION"
    );
    println!("{}", "-".repeat(150));

    for run in runs {
        let id = run.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let task_id = run.get("task_id").and_then(|v| v.as_str()).unwrap_or("-");
        let status = run.get("status").and_then(|v| v.as_str()).unwrap_or("-");
        let started = run.get("started_at").and_then(|v| v.as_str()).unwrap_or("-");
        let exit_code = run
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        let duration = run
            .get("duration_ms")
            .and_then(|v| v.as_i64())
            .map(|v| format!("{}ms", v))
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<36}  {:<36}  {:<10}  {:<20}  {:<10}  {:<8}",
            id, task_id, status, started, exit_code, duration
        );
    }
}

/// Format a single run for detailed display.
fn print_run_detail(run: &serde_json::Value) {
    println!("Run Details:");
    println!("  ID:          {}", run.get("id").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Task ID:     {}", run.get("task_id").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Status:      {}", run.get("status").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("  Attempt:     {}", run.get("attempt").and_then(|v| v.as_i64()).unwrap_or(0));
    println!("  Started At:  {}", run.get("started_at").and_then(|v| v.as_str()).unwrap_or("-"));
    let finished = run.get("finished_at").and_then(|v| v.as_str()).unwrap_or("-");
    println!("  Finished At: {}", finished);
    let exit_code = run
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());
    println!("  Exit Code:   {}", exit_code);
    let duration = run
        .get("duration_ms")
        .and_then(|v| v.as_i64())
        .map(|v| format!("{}ms", v))
        .unwrap_or_else(|| "-".to_string());
    println!("  Duration:    {}", duration);

    let stdout = run.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    let stderr = run.get("stderr").and_then(|v| v.as_str()).unwrap_or("");

    if !stdout.is_empty() {
        println!();
        println!("--- stdout ---");
        println!("{}", stdout);
    }
    if !stderr.is_empty() {
        println!();
        println!("--- stderr ---");
        println!("{}", stderr);
    }
}

/// Dispatch runs subcommands.
pub async fn handle_runs_command(cmd: RunsCommands) -> anyhow::Result<()> {
    let config = Config::load()?;

    match cmd {
        RunsCommands::List {
            task,
            status,
            limit,
        } => runs_list(&config, task.as_deref(), status.as_deref(), limit).await,
        RunsCommands::Show { id } => runs_show(&config, &id).await,
    }
}

/// GET /runs — list runs. Falls back to DB if API is down.
async fn runs_list(
    config: &Config,
    task: Option<&str>,
    status: Option<&str>,
    limit: i64,
) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token_str = read_token().unwrap_or_default();

    // Build query params
    let mut query_params = vec![("limit", limit.to_string())];
    let mut task_id_str = String::new();
    if let Some(task_name) = task {
        task_id_str = resolve_task_id_for_filter(&base_url, &client, &token_str, task_name, config)
            .await
            .unwrap_or_else(|_| task_name.to_string());
        query_params.push(("task_id", task_id_str.clone()));
    }
    if let Some(st) = status {
        query_params.push(("status", st.to_string()));
    }

    let resp = client
        .get(format!("{}/api/v1/runs", base_url))
        .header("Authorization", format!("Bearer {}", token_str))
        .query(&query_params)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let runs: Vec<serde_json::Value> = r.json().await?;
            print_runs_table(&runs);
            Ok(())
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let token = ensure_token(&base_url, &client).await?;
            let resp = client
                .get(format!("{}/api/v1/runs", base_url))
                .header("Authorization", format!("Bearer {}", token))
                .query(&query_params)
                .send()
                .await?;
            if resp.status().is_success() {
                let runs: Vec<serde_json::Value> = resp.json().await?;
                print_runs_table(&runs);
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

            let task_id_filter = if !task_id_str.is_empty() {
                Some(task_id_str.as_str())
            } else {
                None
            };

            let runs =
                db::runs::list_job_runs(&conn, task_id_filter, status, Some(limit), Some(0))
                    .await?;
            let json_runs: Vec<serde_json::Value> = runs
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "task_id": r.task_id,
                        "started_at": r.started_at,
                        "finished_at": r.finished_at,
                        "exit_code": r.exit_code,
                        "stdout": r.stdout,
                        "stderr": r.stderr,
                        "status": r.status.to_string(),
                        "attempt": r.attempt,
                        "duration_ms": r.duration_ms,
                    })
                })
                .collect();
            print_runs_table(&json_runs);
            Ok(())
        }
    }
}

/// GET /runs/:id — show a specific run. Falls back to DB if API is down.
async fn runs_show(config: &Config, run_id: &str) -> anyhow::Result<()> {
    let (base_url, client) = api_client(config);
    let token_str = read_token().unwrap_or_default();

    let resp = client
        .get(format!("{}/api/v1/runs/{}", base_url, run_id))
        .header("Authorization", format!("Bearer {}", token_str))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let run: serde_json::Value = r.json().await?;
            print_run_detail(&run);
            Ok(())
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            let token = ensure_token(&base_url, &client).await?;
            let resp = client
                .get(format!("{}/api/v1/runs/{}", base_url, run_id))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await?;
            if resp.status().is_success() {
                let run: serde_json::Value = resp.json().await?;
                print_run_detail(&run);
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
            match db::runs::get_job_run_by_id(&conn, run_id).await {
                Ok(run) => {
                    let json_run = serde_json::json!({
                        "id": run.id,
                        "task_id": run.task_id,
                        "started_at": run.started_at,
                        "finished_at": run.finished_at,
                        "exit_code": run.exit_code,
                        "stdout": run.stdout,
                        "stderr": run.stderr,
                        "status": run.status.to_string(),
                        "attempt": run.attempt,
                        "duration_ms": run.duration_ms,
                    });
                    print_run_detail(&json_run);
                    Ok(())
                }
                Err(db::helpers::DbError::NotFound) => {
                    anyhow::bail!("Run '{}' not found", run_id)
                }
                Err(e) => Err(e.into()),
            }
        }
    }
}

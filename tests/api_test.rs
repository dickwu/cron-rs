use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{self, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cron_rs::api;
use cron_rs::config::Config;
use cron_rs::db;
use cron_rs::models::{HookRun, HookRunStatus, JobRun, JobRunStatus, Task};
use cron_rs::systemd::SystemdManager;

// --- MockSystemdManager ---

#[derive(Clone, Default)]
struct MockSystemdManager {
    calls: Arc<Mutex<Vec<String>>>,
}

impl MockSystemdManager {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn get_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl SystemdManager for MockSystemdManager {
    async fn install_task(&self, task: &Task) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("install_task:{}", task.name));
        Ok(())
    }

    async fn remove_task(&self, task_name: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("remove_task:{}", task_name));
        Ok(())
    }

    async fn enable_timer(&self, task_name: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("enable_timer:{}", task_name));
        Ok(())
    }

    async fn disable_timer(&self, task_name: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("disable_timer:{}", task_name));
        Ok(())
    }

    async fn start_timer(&self, task_name: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("start_timer:{}", task_name));
        Ok(())
    }

    async fn stop_timer(&self, task_name: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("stop_timer:{}", task_name));
        Ok(())
    }

    async fn daemon_reload(&self) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push("daemon_reload".to_string());
        Ok(())
    }

    async fn is_timer_active(&self, task_name: &str) -> anyhow::Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("is_timer_active:{}", task_name));
        Ok(true)
    }
}

// --- Test helpers ---

fn temp_db_path() -> PathBuf {
    let id = uuid::Uuid::new_v4();
    PathBuf::from(format!("/tmp/cron-rs-test-{}.db", id))
}

fn cleanup_db(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

/// Generate an argon2 password hash for "testpass123".
fn hash_test_password() -> String {
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::SaltString;
    use argon2::{Argon2, PasswordHasher};

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(b"testpass123", &salt)
        .unwrap()
        .to_string()
}

fn test_config(db_path: &Path) -> Config {
    Config {
        username: "admin".to_string(),
        password_hash: hash_test_password(),
        jwt_secret: "test-secret-key-for-jwt-signing-minimum-length".to_string(),
        host: "127.0.0.1".to_string(),
        port: 9746,
        db_path: db_path.to_path_buf(),
        token_expiry: "24h".to_string(),
        config_dir: PathBuf::from("/tmp"),
        timezone: String::new(),
    }
}

async fn setup_app() -> (axum::Router, PathBuf, MockSystemdManager) {
    let db_path = temp_db_path();
    let database = db::Database::new(&db_path).await.unwrap();
    database.run_migrations().await.unwrap();

    let config = test_config(&db_path);
    let mock_systemd = MockSystemdManager::new();

    let state = api::AppState {
        db: Arc::new(database),
        systemd: Arc::new(mock_systemd.clone()),
        config: Arc::new(config),
        event_bus: cron_rs::event_bus::new(16),
        dashboard_cache: Arc::new(tokio::sync::RwLock::new(
            api::dashboard::DashboardCache::default(),
        )),
    };

    let app = api::router(state);
    (app, db_path, mock_systemd)
}

/// Helper to get a JWT token by logging in.
async fn login(app: &axum::Router) -> String {
    let body = serde_json::json!({
        "username": "admin",
        "password": "testpass123"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/auth/login")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["token"].as_str().unwrap().to_string()
}

fn auth_header(token: &str) -> String {
    format!("Bearer {}", token)
}

// --- Tests ---

#[tokio::test]
async fn root_serves_management_page_fallback() {
    let (app, path, _mock) = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("cron-rs"),
        "root route should serve the management page or dashboard fallback"
    );

    cleanup_db(&path);
}

#[tokio::test]
async fn runtime_config_uses_request_host_for_embedded_dashboard() {
    let (app, path, _mock) = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/runtime-config.js")
                .header(http::header::HOST, "10.101.0.18:9746")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains(r#""http://10.101.0.18:9746""#));

    cleanup_db(&path);
}

#[cfg(feature = "embed-web")]
#[tokio::test]
async fn login_route_serves_exported_login_page() {
    let (app, path, _mock) = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("Sign In"));

    cleanup_db(&path);
}

// T20: POST /auth/login -> valid JWT
#[tokio::test]
async fn t20_login_returns_valid_jwt() {
    let (app, path, _mock) = setup_app().await;

    let body = serde_json::json!({
        "username": "admin",
        "password": "testpass123"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/auth/login")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json.get("token").is_some(),
        "Response should contain a token"
    );
    let token = json["token"].as_str().unwrap();
    assert!(!token.is_empty(), "Token should not be empty");

    cleanup_db(&path);
}

// T21: Invalid credentials -> 401
#[tokio::test]
async fn t21_invalid_credentials_returns_401() {
    let (app, path, _mock) = setup_app().await;

    // Wrong password
    let body = serde_json::json!({
        "username": "admin",
        "password": "wrong_password"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/auth/login")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Wrong username
    let body = serde_json::json!({
        "username": "wrong_user",
        "password": "testpass123"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/auth/login")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    cleanup_db(&path);
}

// T23: CRUD tasks (create -> list -> get -> update -> delete)
#[tokio::test]
async fn t23_crud_tasks() {
    let (app, path, mock) = setup_app().await;
    let token = login(&app).await;

    // --- CREATE ---
    let create_body = serde_json::json!({
        "name": "test-task",
        "command": "echo hello",
        "schedule": "*-*-* *:00:00",
        "description": "A test task",
        "max_retries": 2,
        "retry_delay_secs": 10,
        "lock_key": " staff-api-boot ",
        "sandbox_profile": "staff-api-hyperf"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/tasks")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let task_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["name"].as_str().unwrap(), "test-task");
    assert_eq!(created["command"].as_str().unwrap(), "echo hello");
    assert_eq!(created["max_retries"].as_i64().unwrap(), 2);
    assert_eq!(created["lock_key"].as_str().unwrap(), "staff-api-boot");
    assert_eq!(
        created["sandbox_profile"].as_str().unwrap(),
        "staff-api-hyperf"
    );

    // Verify systemd install was called
    let calls = mock.get_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("install_task:")),
        "install_task should have been called. Calls: {:?}",
        calls
    );

    // --- LIST ---
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/tasks")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let tasks: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["name"].as_str().unwrap(), "test-task");

    // --- GET ---
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("/api/v1/tasks/{}", task_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let fetched: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(fetched["id"].as_str().unwrap(), &task_id);

    // --- UPDATE ---
    let update_body = serde_json::json!({
        "command": "echo updated",
        "description": "Updated description",
        "lock_key": null,
        "sandbox_profile": null
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::PUT)
                .uri(format!("/api/v1/tasks/{}", task_id))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&update_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(updated["command"].as_str().unwrap(), "echo updated");
    assert_eq!(
        updated["description"].as_str().unwrap(),
        "Updated description"
    );
    assert!(updated["lock_key"].is_null());
    assert!(updated["sandbox_profile"].is_null());
    // Name should be unchanged
    assert_eq!(updated["name"].as_str().unwrap(), "test-task");

    // --- DELETE ---
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::DELETE)
                .uri(format!("/api/v1/tasks/{}", task_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify systemd remove was called
    let calls = mock.get_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("remove_task:")),
        "remove_task should have been called. Calls: {:?}",
        calls
    );

    // Verify task is gone
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("/api/v1/tasks/{}", task_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    cleanup_db(&path);
}

#[tokio::test]
async fn tasks_accept_and_persist_tags() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    let create_body = serde_json::json!({
        "name": "tagged-task",
        "command": "echo hello",
        "schedule": "*-*-* 02:00:00",
        "tags": ["prod", " backup ", "PROD", ""]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/tasks")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let task_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["tags"][0].as_str().unwrap(), "prod");
    assert_eq!(created["tags"][1].as_str().unwrap(), "backup");
    assert_eq!(created["tags"].as_array().unwrap().len(), 2);

    let update_body = serde_json::json!({
        "tags": ["ops", "nightly"]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::PUT)
                .uri(format!("/api/v1/tasks/{}", task_id))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&update_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(updated["tags"][0].as_str().unwrap(), "ops");
    assert_eq!(updated["tags"][1].as_str().unwrap(), "nightly");

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/tasks")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let tasks: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(tasks[0]["tags"][0].as_str().unwrap(), "ops");
    assert_eq!(tasks[0]["tags"][1].as_str().unwrap(), "nightly");

    cleanup_db(&path);
}

// T28: CRUD hooks
#[tokio::test]
async fn t28_crud_hooks() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    // Create a task first
    let create_task = serde_json::json!({
        "name": "hook-task",
        "command": "echo hello",
        "schedule": "*-*-* *:00:00"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/tasks")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_task).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let task: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let task_id = task["id"].as_str().unwrap();

    // --- CREATE HOOK ---
    let create_hook = serde_json::json!({
        "hook_type": "on_failure",
        "command": "curl http://alert",
        "timeout_secs": 30,
        "run_order": 1
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("/api/v1/tasks/{}/hooks", task_id))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_hook).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let hook: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let hook_id = hook["id"].as_str().unwrap().to_string();
    assert_eq!(hook["hook_type"].as_str().unwrap(), "on_failure");
    assert_eq!(hook["command"].as_str().unwrap(), "curl http://alert");
    assert_eq!(hook["run_order"].as_i64().unwrap(), 1);

    // --- LIST HOOKS ---
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("/api/v1/tasks/{}/hooks", task_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let hooks: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(hooks.len(), 1);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/hooks")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let all_hooks: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(all_hooks.len(), 1);
    assert_eq!(all_hooks[0]["task_id"].as_str().unwrap(), task_id);

    // --- UPDATE HOOK ---
    let update_hook = serde_json::json!({
        "command": "curl http://new-alert",
        "run_order": 2
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::PUT)
                .uri(format!("/api/v1/hooks/{}", hook_id))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&update_hook).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        updated["command"].as_str().unwrap(),
        "curl http://new-alert"
    );
    assert_eq!(updated["run_order"].as_i64().unwrap(), 2);

    // --- DELETE HOOK ---
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::DELETE)
                .uri(format!("/api/v1/hooks/{}", hook_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify hook is gone
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("/api/v1/tasks/{}/hooks", task_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let hooks: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(hooks.is_empty());

    cleanup_db(&path);
}

#[tokio::test]
async fn global_hooks_api_crud() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    let create_hook = serde_json::json!({
        "hook_type": "on_success",
        "command": "echo global-success",
        "timeout_secs": 20,
        "run_order": 2
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/hooks/global")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_hook).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let hook: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let hook_id = hook["id"].as_str().unwrap().to_string();
    assert!(hook["task_id"].is_null());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/hooks/global")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let hooks: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(hooks.len(), 1);
    assert!(hooks[0]["task_id"].is_null());

    let update_hook = serde_json::json!({
        "command": "echo global-updated"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::PUT)
                .uri(format!("/api/v1/hooks/{}", hook_id))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&update_hook).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(updated["command"].as_str().unwrap(), "echo global-updated");
    assert!(updated["task_id"].is_null());

    cleanup_db(&path);
}

// T29: Runs list with filters
#[tokio::test]
async fn t29_runs_list_with_filters() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    // Create a task via API
    let create_task = serde_json::json!({
        "name": "runs-test",
        "command": "echo hello",
        "schedule": "*-*-* *:00:00"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/tasks")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_task).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let task: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let task_id = task["id"].as_str().unwrap().to_string();

    // Insert runs directly into DB for testing filters
    let db_path = path.clone();
    let database = db::Database::new(&db_path).await.unwrap();
    let conn = database.connect().await.unwrap();

    for status in [
        cron_rs::models::JobRunStatus::Success,
        cron_rs::models::JobRunStatus::Failed,
    ] {
        let run = cron_rs::models::JobRun {
            id: String::new(),
            task_id: task_id.clone(),
            started_at: String::new(),
            finished_at: Some("2024-01-01 12:00:00".to_string()),
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            status,
            attempt: 1,
            duration_ms: Some(100),
        };
        db::runs::create_job_run(&conn, &run).await.unwrap();
    }

    // List all runs
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/runs")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let runs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(runs.len(), 2);

    // Filter by task_id
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("/api/v1/runs?task_id={}", task_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let runs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(runs.len(), 2);

    // Filter by status
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/runs?status=failed")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let runs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"].as_str().unwrap(), "failed");

    // List task-specific runs
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("/api/v1/tasks/{}/runs", task_id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let runs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(runs.len(), 2);

    cleanup_db(&path);
}

#[tokio::test]
async fn run_hook_runs_route_returns_hook_runs() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    let create_task = serde_json::json!({
        "name": "hook-run-task",
        "command": "echo hello",
        "schedule": "*-*-* *:00:00"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/tasks")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_task).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let task: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let task_id = task["id"].as_str().unwrap().to_string();

    let create_hook = serde_json::json!({
        "hook_type": "on_failure",
        "command": "echo alert",
        "timeout_secs": 30,
        "run_order": 0
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri(format!("/api/v1/tasks/{}/hooks", task_id))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&create_hook).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let hook: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let hook_id = hook["id"].as_str().unwrap().to_string();

    let database = db::Database::new(&path).await.unwrap();
    let conn = database.connect().await.unwrap();
    let run = db::runs::create_job_run(
        &conn,
        &JobRun {
            id: String::new(),
            task_id: task_id.clone(),
            started_at: String::new(),
            finished_at: Some("2026-04-28 12:00:05".to_string()),
            exit_code: Some(1),
            stdout: "stdout".to_string(),
            stderr: "stderr".to_string(),
            status: JobRunStatus::Failed,
            attempt: 1,
            duration_ms: Some(5000),
        },
    )
    .await
    .unwrap();

    db::runs::create_hook_run(
        &conn,
        &HookRun {
            id: String::new(),
            job_run_id: run.id.clone(),
            hook_id: hook_id.clone(),
            exit_code: Some(0),
            stdout: "hook stdout".to_string(),
            stderr: String::new(),
            started_at: String::new(),
            finished_at: Some("2026-04-28 12:00:06".to_string()),
            status: HookRunStatus::Success,
        },
    )
    .await
    .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri(format!("/api/v1/runs/{}/hooks", run.id))
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let runs: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["hook_id"].as_str().unwrap(), hook_id);
    assert_eq!(runs[0]["status"].as_str().unwrap(), "success");

    cleanup_db(&path);
}

// T30: Health endpoint returns ok
#[tokio::test]
async fn t30_health_endpoint_returns_ok() {
    let (app, path, _mock) = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"].as_str().unwrap(), "ok");

    cleanup_db(&path);
}

#[tokio::test]
async fn events_route_returns_sse_headers() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/events")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(content_type.starts_with("text/event-stream"));

    cleanup_db(&path);
}

// Auth middleware: request without token -> 401
#[tokio::test]
async fn auth_middleware_no_token_returns_401() {
    let (app, path, _mock) = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/tasks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    cleanup_db(&path);
}

// Auth middleware: request with invalid token -> 401
#[tokio::test]
async fn auth_middleware_invalid_token_returns_401() {
    let (app, path, _mock) = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/tasks")
                .header(http::header::AUTHORIZATION, "Bearer invalid-token-xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    cleanup_db(&path);
}

// Task not found -> 404
#[tokio::test]
async fn task_not_found_returns_404() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    let response = app
        .oneshot(
            Request::builder()
                .method(http::Method::GET)
                .uri("/api/v1/tasks/nonexistent-task-id")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    cleanup_db(&path);
}

// Duplicate task name -> 409 conflict
#[tokio::test]
async fn duplicate_task_name_returns_409() {
    let (app, path, _mock) = setup_app().await;
    let token = login(&app).await;

    let body = serde_json::json!({
        "name": "dup-task",
        "command": "echo 1",
        "schedule": "*-*-* *:00:00"
    });

    // First creation
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/tasks")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // Second creation with same name
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(http::Method::POST)
                .uri("/api/v1/tasks")
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth_header(&token))
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    cleanup_db(&path);
}

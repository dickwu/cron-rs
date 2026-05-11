pub mod helpers;
pub mod hooks;
pub mod runs;
pub mod settings;
pub mod tasks;

use std::path::Path;
use std::sync::Arc;

use tracing::info;

/// Embedded migration SQL files.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_initial",
        include_str!("../../migrations/001_initial.sql"),
    ),
    (
        "002_task_tags",
        include_str!("../../migrations/002_task_tags.sql"),
    ),
    (
        "003_global_hooks",
        include_str!("../../migrations/003_global_hooks.sql"),
    ),
    (
        "004_settings",
        include_str!("../../migrations/004_settings.sql"),
    ),
    (
        "005_task_lock_key",
        include_str!("../../migrations/005_task_lock_key.sql"),
    ),
    (
        "006_task_sandbox_profile",
        include_str!("../../migrations/006_task_sandbox_profile.sql"),
    ),
];

struct DatabaseInner {
    db: libsql::Database,
}

#[derive(Clone)]
pub struct Database {
    inner: Arc<DatabaseInner>,
}

impl Database {
    /// Create a new Database instance from a file path.
    /// Creates parent directories if they don't exist.
    pub async fn new(path: &Path) -> anyhow::Result<Self> {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid database path"))?;
        let db = libsql::Builder::new_local(path_str).build().await?;
        Ok(Database {
            inner: Arc::new(DatabaseInner { db }),
        })
    }

    /// Get a connection to the database.
    /// Enables WAL mode and sets busy_timeout=5000ms on each new connection.
    pub async fn connect(&self) -> anyhow::Result<libsql::Connection> {
        let conn = self.inner.db.connect()?;
        // Use query for PRAGMAs that return rows (libsql execute() errors on statements
        // that return rows). Consume result rows to complete each statement.
        let mut rows = conn.query("PRAGMA journal_mode=WAL", ()).await?;
        let _ = rows.next().await;
        let mut rows = conn.query("PRAGMA busy_timeout=5000", ()).await?;
        let _ = rows.next().await;
        Ok(conn)
    }

    /// Run all pending migrations.
    /// Creates the _migrations tracking table if it doesn't exist, then
    /// applies any migrations that haven't been run yet.
    pub async fn run_migrations(&self) -> anyhow::Result<()> {
        let conn = self.connect().await?;

        // Ensure the _migrations table exists
        conn.execute(
            "CREATE TABLE IF NOT EXISTS _migrations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
            (),
        )
        .await?;

        for (name, sql) in MIGRATIONS {
            // Check if this migration has already been applied
            let mut rows = conn
                .query("SELECT COUNT(*) FROM _migrations WHERE name = ?1", [*name])
                .await?;

            let already_applied = if let Some(row) = rows.next().await? {
                row.get::<i64>(0)? > 0
            } else {
                false
            };
            drop(rows);

            if already_applied {
                info!("Migration '{}' already applied, skipping", name);
                continue;
            }

            info!("Applying migration '{}'...", name);
            conn.execute_batch(sql).await?;

            // Record the migration
            conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [*name])
                .await?;

            info!("Migration '{}' applied successfully", name);
        }

        Ok(())
    }
}

use libsql::Connection;

use crate::db::helpers::{now_timestamp, DbError};

pub const KEY_RETENTION_DAYS: &str = "retention_days";
pub const DEFAULT_RETENTION_DAYS: u32 = 30;

/// Read a raw setting value by key.
pub async fn get(conn: &Connection, key: &str) -> Result<Option<String>, DbError> {
    let mut rows = conn
        .query("SELECT value FROM settings WHERE key = ?1", [key])
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get::<String>(0)?)),
        None => Ok(None),
    }
}

/// Upsert a setting value by key.
pub async fn set(conn: &Connection, key: &str, value: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        libsql::params![key.to_string(), value.to_string(), now_timestamp()],
    )
    .await?;
    Ok(())
}

/// Read retention_days, falling back to the default if missing or unparseable.
pub async fn get_retention_days(conn: &Connection) -> Result<u32, DbError> {
    match get(conn, KEY_RETENTION_DAYS).await? {
        Some(v) => Ok(v.parse::<u32>().unwrap_or(DEFAULT_RETENTION_DAYS)),
        None => Ok(DEFAULT_RETENTION_DAYS),
    }
}

/// Persist retention_days.
pub async fn set_retention_days(conn: &Connection, days: u32) -> Result<(), DbError> {
    set(conn, KEY_RETENTION_DAYS, &days.to_string()).await
}

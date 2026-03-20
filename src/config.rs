use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub username: String,
    pub password_hash: String,
    pub jwt_secret: String,
    pub host: String,
    pub port: u16,
    pub db_path: PathBuf,
    pub token_expiry: String,
    pub config_dir: PathBuf,
}

impl Config {
    /// Load configuration from environment variables (via dotenvy).
    /// DB path resolution order:
    /// 1. CRON_RS_DB environment variable
    /// 2. ~/cron-rs/cron-rs.db default
    pub fn load() -> anyhow::Result<Self> {
        // Attempt to load .env file; ignore if missing
        let _ = dotenvy::dotenv();

        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/root"));

        let db_path = match std::env::var("CRON_RS_DB") {
            Ok(p) => {
                let expanded = p.replace('~', &home);
                PathBuf::from(expanded)
            }
            Err(_) => {
                let mut path = PathBuf::from(&home);
                path.push("cron-rs");
                path.push("cron-rs.db");
                path
            }
        };

        let config_dir = match std::env::var("CRON_RS_CONFIG_DIR") {
            Ok(p) => {
                let expanded = p.replace('~', &home);
                PathBuf::from(expanded)
            }
            Err(_) => {
                let mut path = PathBuf::from(&home);
                path.push("cron-rs");
                path
            }
        };

        Ok(Config {
            username: std::env::var("CRON_RS_USERNAME").unwrap_or_else(|_| "admin".to_string()),
            password_hash: std::env::var("CRON_RS_PASSWORD").unwrap_or_default(),
            jwt_secret: std::env::var("CRON_RS_JWT_SECRET").unwrap_or_default(),
            host: std::env::var("CRON_RS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: std::env::var("CRON_RS_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(9746),
            db_path,
            token_expiry: std::env::var("CRON_RS_TOKEN_EXPIRY")
                .unwrap_or_else(|_| "24h".to_string()),
            config_dir,
        })
    }
}

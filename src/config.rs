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
    #[allow(dead_code)]
    pub config_dir: PathBuf,
    /// Optional IANA zone (e.g. `America/Vancouver`) appended to every
    /// generated systemd `OnCalendar=` expression so timers fire in the
    /// user's zone instead of the host system zone. Empty = honor system zone.
    #[allow(dead_code)]
    pub timezone: String,
}

impl Config {
    pub fn default_config_dir_for_home(home: &str) -> PathBuf {
        PathBuf::from(home).join("cron-rs")
    }

    pub fn expand_path(path: &str, home: &str) -> PathBuf {
        match path {
            "~" => PathBuf::from(home),
            p if p.starts_with("~/") => PathBuf::from(home).join(&p[2..]),
            p => PathBuf::from(p),
        }
    }

    fn load_dotenv_files(home: &str) {
        // Preserve standard dotenv behavior for local development first.
        let _ = dotenvy::dotenv();

        if let Ok(config_dir) = std::env::var("CRON_RS_CONFIG_DIR") {
            let path = Self::expand_path(&config_dir, home).join(".env");
            let _ = dotenvy::from_path(path);
        }

        // `cron-rs init` writes here. Load it regardless of the current working
        // directory so SSH sessions can start the daemon from anywhere.
        let default_env = Self::default_config_dir_for_home(home).join(".env");
        let _ = dotenvy::from_path(default_env);

        // Compatibility with older docs that mentioned ~/.cron-rs.
        let legacy_env = PathBuf::from(home).join(".cron-rs").join(".env");
        let _ = dotenvy::from_path(legacy_env);
    }

    /// Load configuration from environment variables (via dotenvy).
    /// DB path resolution order:
    /// 1. CRON_RS_DB environment variable
    /// 2. ~/cron-rs/cron-rs.db default
    pub fn load() -> anyhow::Result<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/root"));
        Self::load_dotenv_files(&home);

        let db_path = match std::env::var("CRON_RS_DB") {
            Ok(p) => Self::expand_path(&p, &home),
            Err(_) => {
                let mut path = Self::default_config_dir_for_home(&home);
                path.push("cron-rs.db");
                path
            }
        };

        let config_dir = match std::env::var("CRON_RS_CONFIG_DIR") {
            Ok(p) => Self::expand_path(&p, &home),
            Err(_) => Self::default_config_dir_for_home(&home),
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
            timezone: std::env::var("CRON_RS_TIMEZONE")
                .unwrap_or_default()
                .trim()
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::Config;

    #[test]
    fn expands_only_home_prefix() {
        assert_eq!(
            Config::expand_path("~/cron-rs", "/tmp/home"),
            PathBuf::from("/tmp/home/cron-rs")
        );
        assert_eq!(
            Config::expand_path("/tmp/~literal", "/tmp/home"),
            PathBuf::from("/tmp/~literal")
        );
    }
}

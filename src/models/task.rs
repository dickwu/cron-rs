use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyPolicy {
    Skip,
    Allow,
    Queue,
}

impl std::fmt::Display for ConcurrencyPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConcurrencyPolicy::Skip => write!(f, "skip"),
            ConcurrencyPolicy::Allow => write!(f, "allow"),
            ConcurrencyPolicy::Queue => write!(f, "queue"),
        }
    }
}

impl FromStr for ConcurrencyPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "skip" => Ok(ConcurrencyPolicy::Skip),
            "allow" => Ok(ConcurrencyPolicy::Allow),
            "queue" => Ok(ConcurrencyPolicy::Queue),
            other => Err(format!("unknown concurrency policy: {}", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub command: String,
    pub schedule: String,
    pub tags: Vec<String>,
    pub description: String,
    pub enabled: bool,
    pub max_retries: i32,
    pub retry_delay_secs: i32,
    pub timeout_secs: Option<i32>,
    pub concurrency_policy: ConcurrencyPolicy,
    pub created_at: String,
    pub updated_at: String,
}

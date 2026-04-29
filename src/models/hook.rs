use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HookType {
    #[serde(rename = "on_failure")]
    Failure,
    #[serde(rename = "on_success")]
    Success,
    #[serde(rename = "on_retry_exhausted")]
    RetryExhausted,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookType::Failure => write!(f, "on_failure"),
            HookType::Success => write!(f, "on_success"),
            HookType::RetryExhausted => write!(f, "on_retry_exhausted"),
        }
    }
}

impl FromStr for HookType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "on_failure" => Ok(HookType::Failure),
            "on_success" => Ok(HookType::Success),
            "on_retry_exhausted" => Ok(HookType::RetryExhausted),
            other => Err(format!("unknown hook type: {}", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub id: String,
    pub task_id: Option<String>,
    pub hook_type: HookType,
    pub command: String,
    pub timeout_secs: Option<i32>,
    pub run_order: i32,
    pub created_at: String,
}

use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookType {
    OnFailure,
    OnSuccess,
    OnRetryExhausted,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookType::OnFailure => write!(f, "on_failure"),
            HookType::OnSuccess => write!(f, "on_success"),
            HookType::OnRetryExhausted => write!(f, "on_retry_exhausted"),
        }
    }
}

impl FromStr for HookType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "on_failure" => Ok(HookType::OnFailure),
            "on_success" => Ok(HookType::OnSuccess),
            "on_retry_exhausted" => Ok(HookType::OnRetryExhausted),
            other => Err(format!("unknown hook type: {}", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub id: String,
    pub task_id: String,
    pub hook_type: HookType,
    pub command: String,
    pub timeout_secs: Option<i32>,
    pub run_order: i32,
    pub created_at: String,
}

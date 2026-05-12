use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobRunStatus {
    Running,
    Success,
    Failed,
    Retrying,
    Timeout,
    Skipped,
    Crashed,
}

impl std::fmt::Display for JobRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobRunStatus::Running => write!(f, "running"),
            JobRunStatus::Success => write!(f, "success"),
            JobRunStatus::Failed => write!(f, "failed"),
            JobRunStatus::Retrying => write!(f, "retrying"),
            JobRunStatus::Timeout => write!(f, "timeout"),
            JobRunStatus::Skipped => write!(f, "skipped"),
            JobRunStatus::Crashed => write!(f, "crashed"),
        }
    }
}

impl FromStr for JobRunStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(JobRunStatus::Running),
            "success" => Ok(JobRunStatus::Success),
            "failed" => Ok(JobRunStatus::Failed),
            "retrying" => Ok(JobRunStatus::Retrying),
            "timeout" => Ok(JobRunStatus::Timeout),
            "skipped" => Ok(JobRunStatus::Skipped),
            "crashed" => Ok(JobRunStatus::Crashed),
            other => Err(format!("unknown job run status: {}", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRun {
    pub id: String,
    pub task_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub status: JobRunStatus,
    pub attempt: i32,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRunSummary {
    pub id: String,
    pub task_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub status: JobRunStatus,
    pub attempt: i32,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookRunStatus {
    Success,
    Failed,
    Timeout,
}

impl std::fmt::Display for HookRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookRunStatus::Success => write!(f, "success"),
            HookRunStatus::Failed => write!(f, "failed"),
            HookRunStatus::Timeout => write!(f, "timeout"),
        }
    }
}

impl FromStr for HookRunStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "success" => Ok(HookRunStatus::Success),
            "failed" => Ok(HookRunStatus::Failed),
            "timeout" => Ok(HookRunStatus::Timeout),
            other => Err(format!("unknown hook run status: {}", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRun {
    pub id: String,
    pub job_run_id: String,
    pub hook_id: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: HookRunStatus,
}

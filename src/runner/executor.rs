use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// Result of executing a command.
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: i64,
    pub timed_out: bool,
}

/// Maximum bytes to capture from stdout/stderr (1 MB).
const MAX_OUTPUT_BYTES: u64 = 1_048_576;

/// How often the progress tailer flushes partial output while a command runs.
const PROGRESS_FLUSH_INTERVAL_MS: u64 = 1000;

/// A snapshot of captured output `(stdout, stderr)` emitted while a command is
/// still running. The runner persists these so the dashboard streams output.
pub type ProgressSnapshot = (String, String);

/// Cheap size probe used to detect whether a temp file has grown since the last
/// flush. Returns `None` if the file is missing (e.g. already cleaned up).
async fn file_len(path: &std::path::Path) -> Option<u64> {
    tokio::fs::metadata(path).await.ok().map(|m| m.len())
}

/// Aborts the spawned progress tailer when `execute_command` returns on any
/// path, so it never reads a temp file after cleanup or outlives the command.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Read the last MAX_OUTPUT_BYTES from a file into a String, lossy.
async fn read_tail(path: &std::path::Path) -> anyhow::Result<String> {
    let metadata = tokio::fs::metadata(path).await?;
    let file_size = metadata.len();

    let mut file = tokio::fs::File::open(path).await?;

    if file_size > MAX_OUTPUT_BYTES {
        // Seek to file_size - MAX_OUTPUT_BYTES
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::End(-(MAX_OUTPUT_BYTES as i64)))
            .await?;
    }

    let capacity = file_size.min(MAX_OUTPUT_BYTES) as usize;
    let mut buf = Vec::with_capacity(capacity);
    file.read_to_end(&mut buf).await?;

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Execute a command, capturing stdout/stderr to temp files.
/// If timeout_secs is Some(n) with n > 0, the command will be killed after n seconds.
///
/// When `progress_tx` is `Some`, a background tailer periodically sends growing
/// snapshots of the captured output while the command runs, letting the caller
/// stream partial output. Snapshots are monotonic — a post-exit read of a
/// cleaned-up temp file can never regress what was already sent.
pub async fn execute_command(
    command: &str,
    timeout_secs: Option<i32>,
    progress_tx: Option<tokio::sync::mpsc::Sender<ProgressSnapshot>>,
) -> anyhow::Result<CommandResult> {
    let task_uuid = uuid::Uuid::new_v4();
    let tmp_dir = std::env::temp_dir();
    let stdout_path = tmp_dir.join(format!("cron-rs-{}-stdout", task_uuid));
    let stderr_path = tmp_dir.join(format!("cron-rs-{}-stderr", task_uuid));

    // Open temp files for stdout/stderr
    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let stdout_stdio: std::process::Stdio = stdout_file.into();
    let stderr_stdio: std::process::Stdio = stderr_file.into();

    let start = Instant::now();

    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(stdout_stdio)
        .stderr(stderr_stdio)
        .spawn()?;

    // While the command runs, periodically flush the captured output so callers
    // can show it live. Kept alive (and auto-aborted) for the rest of this fn.
    let _progress_tailer = progress_tx.map(|tx| {
        let stdout_path = stdout_path.clone();
        let stderr_path = stderr_path.clone();
        AbortOnDrop(tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await; // consume the immediate first tick
            let mut last_total: u64 = 0;
            loop {
                ticker.tick().await;
                // If either temp file is gone, the command has finished and
                // cleanup ran — stop rather than emit a regressed snapshot.
                let (out_len, err_len) =
                    match (file_len(&stdout_path).await, file_len(&stderr_path).await) {
                        (Some(o), Some(e)) => (o, e),
                        _ => break,
                    };
                let total = out_len.saturating_add(err_len);
                if total <= last_total {
                    continue; // no new output since the last flush
                }
                last_total = total;
                let stdout = read_tail(&stdout_path).await.unwrap_or_default();
                let stderr = read_tail(&stderr_path).await.unwrap_or_default();
                if tx.send((stdout, stderr)).await.is_err() {
                    break; // consumer dropped
                }
            }
        }))
    });

    let effective_timeout = match timeout_secs {
        Some(t) if t > 0 => Some(std::time::Duration::from_secs(t as u64)),
        _ => None,
    };

    let (exit_code, timed_out) = if let Some(timeout_duration) = effective_timeout {
        match tokio::time::timeout(timeout_duration, child.wait()).await {
            Ok(Ok(status)) => {
                // Completed within timeout
                (status.code().unwrap_or(-1), false)
            }
            Ok(Err(e)) => {
                // Error waiting for child
                cleanup_temp_files(&stdout_path, &stderr_path).await;
                return Err(e.into());
            }
            Err(_) => {
                // Timeout expired — send SIGTERM
                let pid = child.id();
                if let Some(pid) = pid {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                }

                // Grace period: wait 5 seconds for graceful shutdown
                match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                    Ok(Ok(_)) => {
                        // Process exited after SIGTERM
                    }
                    _ => {
                        // Still running — SIGKILL
                        if let Some(pid) = pid {
                            unsafe {
                                libc::kill(pid as i32, libc::SIGKILL);
                            }
                        }
                        // Wait for the child to be reaped
                        let _ = child.wait().await;
                    }
                }

                (-1, true)
            }
        }
    } else {
        // No timeout
        let status = child.wait().await?;
        (status.code().unwrap_or(-1), false)
    };

    let duration_ms = start.elapsed().as_millis() as i64;

    // Read captured output
    let stdout = read_tail(&stdout_path).await.unwrap_or_default();
    let stderr = read_tail(&stderr_path).await.unwrap_or_default();

    // Clean up temp files
    cleanup_temp_files(&stdout_path, &stderr_path).await;

    Ok(CommandResult {
        exit_code,
        stdout,
        stderr,
        duration_ms,
        timed_out,
    })
}

async fn cleanup_temp_files(stdout_path: &std::path::Path, stderr_path: &std::path::Path) {
    let _ = tokio::fs::remove_file(stdout_path).await;
    let _ = tokio::fs::remove_file(stderr_path).await;
}

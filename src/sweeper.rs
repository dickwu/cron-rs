use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::db::helpers::parse_run_ts;
use crate::db::{self, Database};
use crate::systemd::SystemdManager;

const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// Runs younger than this are never swept, so a run is not marked crashed
/// between its DB insert and its process/service becoming observable.
pub const SWEEP_GRACE_SECS: i64 = 120;

/// Absolute backstop: a run stuck in running/retrying longer than this is
/// marked crashed even if a liveness signal says otherwise (e.g. PID reuse).
const MAX_STUCK_SECS: i64 = 7 * 86_400;

/// Spawn the background orphan sweep: once at startup, then periodically.
pub fn spawn(db: Arc<Database>, systemd: Arc<dyn SystemdManager>) {
    tokio::spawn(async move { run(db, systemd).await });
}

async fn run(db: Arc<Database>, systemd: Arc<dyn SystemdManager>) {
    let mut interval = tokio::time::interval(SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        match sweep_once(&db, systemd.as_ref(), SWEEP_GRACE_SECS).await {
            Ok(0) => {}
            Ok(n) => info!("orphan sweep: marked {} run(s) as crashed", n),
            Err(e) => warn!("orphan sweep failed: {}", e),
        }
    }
}

/// Mark runs as crashed when they claim to be running/retrying but their
/// runner is provably gone (recorded PID no longer exists, or no PID and the
/// task's service unit is inactive). Runs younger than `grace_secs`, and runs
/// whose runner is still alive — e.g. across a daemon restart — are left
/// untouched. Returns the number of runs marked crashed.
pub async fn sweep_once(
    db: &Database,
    systemd: &dyn SystemdManager,
    grace_secs: i64,
) -> anyhow::Result<u64> {
    let conn = db.connect().await?;
    let stuck = db::runs::list_stuck_run_summaries(&conn).await?;
    if stuck.is_empty() {
        return Ok(0);
    }

    // task_id -> name lookup; runs of deleted tasks have no live runner.
    let tasks = db::tasks::list(&conn).await?;
    let names: std::collections::HashMap<String, String> =
        tasks.into_iter().map(|t| (t.id, t.name)).collect();

    let now = chrono::Utc::now();
    let mut crashed = 0u64;
    for (run, runner_pid) in stuck {
        let age_secs = parse_run_ts(&run.started_at).map(|ts| (now - ts).num_seconds());
        if let Some(age) = age_secs {
            if age < grace_secs {
                continue;
            }
        }

        let past_backstop = age_secs.is_some_and(|age| age > MAX_STUCK_SECS);
        let alive = if past_backstop {
            false
        } else {
            match runner_pid {
                Some(pid) if pid > 0 => pid_alive(pid),
                // No PID recorded (legacy rows): fall back to the service
                // unit. Errors count as alive so we never sweep on a hiccup.
                _ => match names.get(&run.task_id) {
                    Some(name) => systemd.is_service_active(name).await.unwrap_or(true),
                    None => false,
                },
            }
        };
        if alive {
            continue;
        }

        if db::runs::mark_run_crashed(&conn, &run.id).await? {
            crashed += 1;
        }
    }

    Ok(crashed)
}

fn pid_alive(pid: i64) -> bool {
    Path::new(&format!("/proc/{}", pid)).exists()
}

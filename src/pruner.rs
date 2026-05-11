use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::db::{self, Database};

const PRUNE_INTERVAL: Duration = Duration::from_secs(86_400);

/// Spawn a background task that prunes old job_runs once on startup
/// and then daily, re-reading retention_days from the settings table
/// each tick so changes take effect without a daemon restart.
pub fn spawn(db: Arc<Database>) {
    tokio::spawn(async move { run(db).await });
}

async fn run(db: Arc<Database>) {
    let mut interval = tokio::time::interval(PRUNE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        prune_once(&db).await;
    }
}

async fn prune_once(db: &Database) {
    let conn = match db.connect().await {
        Ok(c) => c,
        Err(e) => {
            warn!("pruner: db connect failed: {e}");
            return;
        }
    };

    let days = match db::settings::get_retention_days(&conn).await {
        Ok(d) => d,
        Err(e) => {
            warn!("pruner: failed to read retention_days: {e}");
            return;
        }
    };

    match db::runs::prune_runs_older_than(&conn, days).await {
        Ok(0) => {}
        Ok(n) => info!("pruner: deleted {n} job_run(s) older than {days} day(s)"),
        Err(e) => warn!("pruner: prune failed: {e}"),
    }
}

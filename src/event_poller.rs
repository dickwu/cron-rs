use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::warn;

use crate::db::{self, Database};
use crate::event_bus::{publish, EventBus};

pub fn spawn(db: Arc<Database>, bus: EventBus) {
    tokio::spawn(async move { run(db, bus).await });
}

async fn run(db: Arc<Database>, bus: EventBus) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut seen_runs: HashMap<String, String> = HashMap::new();
    let mut seen_tasks: HashMap<String, String> = HashMap::new();

    if let Ok(conn) = db.connect().await {
        if let Ok(runs) = db::runs::list_job_runs(&conn, None, None, None, Some(500), None).await {
            for run in runs {
                seen_runs.insert(run.id, run.status.to_string());
            }
        }
        if let Ok(tasks) = db::tasks::list(&conn).await {
            for task in tasks {
                seen_tasks.insert(task.id, task.updated_at);
            }
        }
    }

    loop {
        interval.tick().await;
        if bus.receiver_count() == 0 {
            continue;
        }
        let conn = match db.connect().await {
            Ok(c) => c,
            Err(e) => {
                warn!("event_poller: db connect failed: {e}");
                continue;
            }
        };

        if let Ok(runs) = db::runs::list_job_runs(&conn, None, None, None, Some(500), None).await {
            for run in runs {
                let status = run.status.to_string();
                let key = run.id.clone();
                let prev = seen_runs.get(&key).cloned();
                match prev {
                    None => {
                        publish(&bus, "run_started", json!({ "run": run }));
                    }
                    Some(p) if p != status => match status.as_str() {
                        "success" => publish(&bus, "run_completed", json!({ "run": run })),
                        "failed" | "crashed" | "timeout" => {
                            publish(&bus, "run_failed", json!({ "run": run }))
                        }
                        _ => {}
                    },
                    _ => {}
                }
                seen_runs.insert(key, status);
            }
        }

        if let Ok(tasks) = db::tasks::list(&conn).await {
            let current_ids: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
            for task in tasks {
                let prev = seen_tasks.get(&task.id).cloned();
                let event = match prev {
                    None => Some("task_created"),
                    Some(ref ts) if ts != &task.updated_at => Some("task_updated"),
                    _ => None,
                };
                if let Some(ev) = event {
                    publish(&bus, ev, json!({ "task": task }));
                    seen_tasks.insert(task.id.clone(), task.updated_at.clone());
                }
            }
            let stale: Vec<String> = seen_tasks
                .keys()
                .filter(|id| !current_ids.contains(*id))
                .cloned()
                .collect();
            for id in stale {
                publish(&bus, "task_deleted", json!({ "task_id": id }));
                seen_tasks.remove(&id);
            }
        }
    }
}

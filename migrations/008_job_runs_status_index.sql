-- Composite index so status-scoped scans (orphan sweep, failed-run lists,
-- dashboard aggregation) avoid walking the whole job_runs table.
CREATE INDEX IF NOT EXISTS idx_job_runs_status_started ON job_runs(status, started_at);

-- PID of the `cron-rs run` process executing the run. Lets the orphan sweep
-- distinguish a dead runner from one that is still alive, including runs
-- triggered outside systemd (API trigger spawns a detached process).
ALTER TABLE job_runs ADD COLUMN runner_pid INTEGER;

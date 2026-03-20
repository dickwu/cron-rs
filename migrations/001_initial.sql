-- Migrations tracking table
CREATE TABLE IF NOT EXISTS _migrations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Tasks table
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    command TEXT NOT NULL,
    schedule TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    max_retries INTEGER NOT NULL DEFAULT 0,
    retry_delay_secs INTEGER NOT NULL DEFAULT 60,
    timeout_secs INTEGER,
    concurrency_policy TEXT NOT NULL DEFAULT 'skip' CHECK (concurrency_policy IN ('skip', 'allow', 'queue')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Hooks table
CREATE TABLE IF NOT EXISTS hooks (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    hook_type TEXT NOT NULL CHECK (hook_type IN ('on_failure', 'on_success', 'on_retry_exhausted')),
    command TEXT NOT NULL,
    timeout_secs INTEGER,
    run_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_hooks_task_id ON hooks(task_id);

-- Job runs table
CREATE TABLE IF NOT EXISTS job_runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT,
    exit_code INTEGER,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'success', 'failed', 'retrying', 'timeout', 'skipped', 'crashed')),
    attempt INTEGER NOT NULL DEFAULT 1,
    duration_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_job_runs_task_id ON job_runs(task_id);
CREATE INDEX IF NOT EXISTS idx_job_runs_started_at ON job_runs(started_at);

-- Hook runs table
CREATE TABLE IF NOT EXISTS hook_runs (
    id TEXT PRIMARY KEY,
    job_run_id TEXT NOT NULL REFERENCES job_runs(id) ON DELETE CASCADE,
    hook_id TEXT NOT NULL REFERENCES hooks(id) ON DELETE CASCADE,
    exit_code INTEGER,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT,
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('success', 'failed', 'timeout'))
);

CREATE INDEX IF NOT EXISTS idx_hook_runs_job_run_id ON hook_runs(job_run_id);

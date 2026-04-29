PRAGMA foreign_keys=OFF;

CREATE TABLE hooks_new (
    id TEXT PRIMARY KEY,
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    hook_type TEXT NOT NULL CHECK (hook_type IN ('on_failure', 'on_success', 'on_retry_exhausted')),
    command TEXT NOT NULL,
    timeout_secs INTEGER,
    run_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO hooks_new (id, task_id, hook_type, command, timeout_secs, run_order, created_at)
SELECT id, task_id, hook_type, command, timeout_secs, run_order, created_at
FROM hooks;

DROP TABLE hooks;
ALTER TABLE hooks_new RENAME TO hooks;

CREATE INDEX IF NOT EXISTS idx_hooks_task_id ON hooks(task_id);

PRAGMA foreign_keys=ON;

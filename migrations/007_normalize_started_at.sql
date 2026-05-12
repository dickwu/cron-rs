-- Normalize legacy timestamps (space-separated, no zone marker) into RFC 3339
-- with the `Z` suffix so the `idx_job_runs_started_at` index can drive
-- date-range scans via plain lex comparison.

UPDATE job_runs
SET started_at = REPLACE(started_at, ' ', 'T') || 'Z'
WHERE started_at GLOB '????-??-?? ??:??:??';

UPDATE job_runs
SET finished_at = REPLACE(finished_at, ' ', 'T') || 'Z'
WHERE finished_at GLOB '????-??-?? ??:??:??';

UPDATE hook_runs
SET started_at = REPLACE(started_at, ' ', 'T') || 'Z'
WHERE started_at GLOB '????-??-?? ??:??:??';

UPDATE hook_runs
SET finished_at = REPLACE(finished_at, ' ', 'T') || 'Z'
WHERE finished_at GLOB '????-??-?? ??:??:??';

-- Same for tasks created_at/updated_at if applicable (no-op when columns missing).
UPDATE tasks
SET created_at = REPLACE(created_at, ' ', 'T') || 'Z'
WHERE created_at GLOB '????-??-?? ??:??:??';

UPDATE tasks
SET updated_at = REPLACE(updated_at, ' ', 'T') || 'Z'
WHERE updated_at GLOB '????-??-?? ??:??:??';

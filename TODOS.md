# TODOS

## Post-v1

### DB Retention Policy
Auto-delete job_runs and hook_runs older than N days. Without cleanup, the SQLite DB
grows unbounded (~500MB/year for a task running every minute). Configurable via
`CRON_RS_RETENTION_DAYS=30` in `.env`. Implement as a periodic cleanup in the daemon
(e.g., once per hour). Depends on: DB layer complete.

### OnCalendar Validation
Validate OnCalendar expressions at task creation time by shelling out to
`systemd-analyze calendar "expression"`. Currently, invalid expressions create
timer files that fail silently — the error only appears in journalctl while the
API returns 200. The validation response could also include the next trigger time.
Depends on: API + systemd layer.

### Password Change Command
Add `cron-rs passwd` to change admin password without manually editing `.env`.
Currently requires: stop daemon, manually hash password, edit file, restart.
Could also support `cron-rs init --reset` to regenerate JWT secret (invalidates
all tokens). Depends on: `cron-rs init` implementation.

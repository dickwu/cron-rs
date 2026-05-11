# cron-rs

A systemd timer management platform with a REST API, CLI, hooks, retry logic, and real-time event streaming.

## Architecture

```
                        +-----------------------+
                        |    cron-rs daemon      |
                        |    (REST API + SSE)    |
                        |    :9746               |
                        +----------+------------+
                                   |
                    +--------------+--------------+
                    |                             |
          +---------v--------+         +---------v--------+
          |  systemd timers  |         |  SQLite (libsql) |
          |  .timer + .service|        |  ~/.cron-rs/     |
          +--------+---------+         +------------------+
                   |
          +--------v---------+
          |   task runner     |
          |   (cron-rs run)   |
          |   retries, hooks, |
          |   output capture  |
          +-------------------+
```

The **daemon** exposes the API and manages systemd unit files. When a timer fires, systemd invokes the **task runner** (`cron-rs run`), which executes the command, handles retries with backoff, fires hooks, captures output, and writes results to the **SQLite** database.

## Features

- **REST API** -- full CRUD for tasks, hooks, and runs
- **CLI** -- manage everything from the terminal
- **JWT authentication** -- token-based auth with configurable expiry
- **Hooks** -- `on_failure`, `on_success`, `on_retry_exhausted` per task
- **Embedded web dashboard** -- serve the management UI from the daemon on the same host/port
- **Retry with backoff** -- configurable max retries and delay
- **Concurrency control** -- skip, queue, or allow parallel runs
- **Shared lock keys** -- optional `flock` guards for tasks that must not overlap app boot/cache generation
- **Systemd sandbox profiles** -- optional hardening for tasks that should only write approved paths
- **SQLite via libsql** -- zero-config embedded database
- **SSE real-time events** -- stream task and run state changes to clients
- **Schedule preview** -- validate OnCalendar expressions via `systemd-analyze`

## Quick Start

```bash
# 1. Initialize (creates config, DB, and admin credentials)
cron-rs init

# Or initialize from a non-interactive SSH session
cron-rs init \
  --username admin \
  --password 'change-this-password' \
  --host 0.0.0.0 \
  --port 9746

# 2. Import existing user crontab and user systemd timers into cron-rs
cron-rs import

# 3. Install and start the daemon as a user systemd service
cron-rs service install --host 0.0.0.0 --start

# 4. Create a task
cron-rs task create my-backup \
  --command "/usr/local/bin/backup.sh" \
  --schedule "*-*-* 02:00:00"

# 5. Check status
cron-rs status
```

## Installation

**From releases:**

Download the latest binary from [GitHub Releases](https://github.com/dickwu/cron-rs/releases).

**From source:**

```bash
cargo install --git https://github.com/dickwu/cron-rs
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `cron-rs init` | Interactive first-time setup |
| `cron-rs init --username <u> --password <p> --host 0.0.0.0` | Non-interactive SSH/server setup |
| `cron-rs daemon` | Start the API server in the foreground |
| `cron-rs daemon --host 0.0.0.0 --port 9746` | Start the API server with runtime bind overrides |
| `cron-rs service install --host 0.0.0.0 --start` | Install and start the daemon with `systemctl --user` |
| `cron-rs service restart` | Restart the user systemd daemon service |
| `cron-rs service status` | Show `systemctl --user status cron-rs-daemon.service` |
| `cron-rs service uninstall` | Disable and remove the user systemd daemon service |
| `cron-rs import` | Import user crontab and user systemd timers into cron-rs as disabled tasks |
| `cron-rs import --include-system` | Also inspect system-wide systemd timers |
| `cron-rs import --enable` | Enable imported tasks and install cron-rs timers immediately |
| `cron-rs import --dry-run` | Preview import candidates without changing the DB |
| `cron-rs status` | Show status of all tasks |
| `cron-rs doctor` | Diagnose common issues |
| `cron-rs regenerate` | Regenerate systemd units from DB |
| `cron-rs regenerate --rewrite-all` | Explicitly rewrite every generated systemd unit |
| `cron-rs task list` | List all tasks |
| `cron-rs task create <name>` | Create a task (`--command`, `--schedule`) |
| `cron-rs task show <name\|id>` | Show task details |
| `cron-rs task edit <name\|id>` | Edit a task |
| `cron-rs task delete <name\|id>` | Delete a task |
| `cron-rs task enable <name\|id>` | Enable a task |
| `cron-rs task disable <name\|id>` | Disable a task |
| `cron-rs task trigger <name\|id>` | Trigger an immediate run |
| `cron-rs hook add <task>` | Add a hook (`--on`, `--command`) |
| `cron-rs hook list <task>` | List hooks for a task |
| `cron-rs hook remove <id>` | Remove a hook |
| `cron-rs runs list` | List recent runs (`--task`, `--status`, `--limit`) |
| `cron-rs runs show <id>` | Show run details |

## API Endpoints

All endpoints under `/api/v1/` require JWT auth (`Authorization: Bearer <token>`) unless noted.

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/auth/login` | Login (public) |
| GET | `/api/v1/health` | Health check (public) |
| GET | `/api/v1/status` | System status overview |
| GET | `/api/v1/tasks` | List tasks |
| POST | `/api/v1/tasks` | Create task |
| GET | `/api/v1/tasks/{id}` | Get task |
| PUT | `/api/v1/tasks/{id}` | Update task |
| DELETE | `/api/v1/tasks/{id}` | Delete task |
| POST | `/api/v1/tasks/{id}/enable` | Enable task |
| POST | `/api/v1/tasks/{id}/disable` | Disable task |
| POST | `/api/v1/tasks/{id}/trigger` | Trigger immediate run |
| GET | `/api/v1/hooks` | List all configured hooks |
| GET | `/api/v1/tasks/{id}/hooks` | List hooks for task |
| POST | `/api/v1/tasks/{id}/hooks` | Create hook |
| PUT | `/api/v1/hooks/{id}` | Update hook |
| DELETE | `/api/v1/hooks/{id}` | Delete hook |
| GET | `/api/v1/runs` | List runs |
| GET | `/api/v1/runs/{id}` | Get run details |
| GET | `/api/v1/runs/{id}/hooks` | List hook runs for a run |
| GET | `/api/v1/tasks/{id}/runs` | List runs for task |
| GET | `/api/v1/schedule/preview` | Preview schedule times (`?expr=...&count=5`) |
| GET | `/api/v1/events` | SSE event stream |

## Configuration

Configuration is stored in `~/cron-rs/.env` (created by `cron-rs init`).

| Variable | Default | Description |
|----------|---------|-------------|
| `CRON_RS_USERNAME` | `admin` | Login username |
| `CRON_RS_PASSWORD` | -- | Login password (set by init) |
| `CRON_RS_JWT_SECRET` | -- | JWT signing secret (set by init) |
| `CRON_RS_HOST` | `127.0.0.1` | Bind address |
| `CRON_RS_PORT` | `9746` | Bind port |
| `CRON_RS_DB` | `~/cron-rs/cron-rs.db` | Database path |
| `CRON_RS_TOKEN_EXPIRY` | `24h` | JWT token expiry |
| `CRON_RS_CONFIG_DIR` | `~/cron-rs` | Config directory |
| `CRON_RS_CORS_ORIGIN` | -- | CORS origin (for external web UI) |

## Importing Existing Schedules

`cron-rs import` reads the current user's crontab (`crontab -l`) and user
systemd timers (`systemctl --user list-unit-files --type=timer`) into the
cron-rs database. Imported tasks are disabled by default so the original
crontab/timer and cron-rs do not both run the same command. Use
`cron-rs import --enable` only when you are ready for cron-rs to install and
start its own timers.

Cron lines with both day-of-month and day-of-week restrictions are skipped
because cron treats those fields as an OR, while one systemd `OnCalendar`
expression cannot preserve that behavior safely.

## PHP/Hyperf Integration Notes

Hyperf writes `runtime/container/` scan-cache files during boot. Older Hyperf
versions write these files non-atomically, so a long-running Swoole service and
a cron-rs PHP oneshot can race during cold boot. Set the same `lock_key` on all
Hyperf-related tasks so generated units use a shared `flock`:

```bash
cron-rs task edit staff-api-sync-patient-increment --lock-key staff-api-boot
cron-rs task edit staff-api-sync-patient-increment --sandbox-profile staff-api-hyperf
cron-rs regenerate --rewrite-all
```

With a lock key set, the generated service wraps `cron-rs run`:

```ini
ExecStart=/usr/bin/flock --exclusive --wait 120 /run/cron-rs/locks/staff-api-boot.lock \
          /path/to/cron-rs run --task-id ... --task-name ... --db-path ...
```

The daemon creates `/run/cron-rs/locks` on startup. The directory is volatile
and only stores zero-byte lock sentinels.

For Hyperf tasks, `--sandbox-profile staff-api-hyperf` adds systemd hardening to
the generated service. It keeps the service read-only by default while allowing
writes to:

- the cron-rs database directory, for run records and concurrency locks
- `/run/cron-rs/locks`, for shared boot locks
- `/server/staff-api/runtime`, for Hyperf's runtime/cache files

The profile also enables settings such as `NoNewPrivileges=true`,
`PrivateTmp=true`, `PrivateDevices=true`, and `ProtectSystem=strict`.

The Swoole master service is outside cron-rs, so it must participate in the
same lock once:

```ini
# /etc/systemd/system/staff-api.service
[Service]
ExecStartPre=/usr/bin/flock --exclusive --wait 120 \
             /run/cron-rs/locks/staff-api-boot.lock \
             /usr/bin/php /server/staff-api/bin/hyperf.php list >/dev/null
ExecStart=/usr/bin/php /server/staff-api/bin/hyperf.php start
```

`bin/hyperf.php list` warms the scan cache while holding the lock. The actual
Swoole `ExecStart` then boots against a hot cache. `cron-rs doctor` warns when
Hyperf-looking tasks do not have a lock key, and when the `staff-api-boot`
companion service lock is missing.

## Web Dashboard

The daemon can serve the exported management dashboard directly on the same
host and port as the API when built with the `embed-web` feature. The embedded
UI includes the dashboard, task list/detail views, a read-only hooks catalog at
`/hooks`, and a settings screen at `/settings`. Hook add/edit/delete stays on
the task detail page so each hook remains scoped to its task.

```bash
# Build the static dashboard from the sibling cron-rs-web checkout
cd ../cron-rs-web
npm ci
npm run build

# Build and install a daemon binary that embeds the dashboard
cd ../cron-rs
cargo build --release --features embed-web
./target/release/cron-rs service install --host 0.0.0.0 --port 9746 --start

# Open the management page
open http://10.101.0.18:9746/
```

The API remains available under `/api/v1/*`, and all other browser routes fall
back to the dashboard. This is the simplest way to expose a single management
entry point from an SSH-hosted Linux server.

## License

MIT

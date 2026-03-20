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
- **Retry with backoff** -- configurable max retries and delay
- **Concurrency control** -- skip, queue, or allow parallel runs
- **SQLite via libsql** -- zero-config embedded database
- **SSE real-time events** -- stream task and run state changes to clients
- **Schedule preview** -- validate OnCalendar expressions via `systemd-analyze`

## Quick Start

```bash
# 1. Initialize (creates config, DB, and admin credentials)
cron-rs init

# 2. Start the daemon
cron-rs daemon

# 3. Create a task
cron-rs task create my-backup \
  --command "/usr/local/bin/backup.sh" \
  --schedule "*-*-* 02:00:00"

# 4. Check status
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
| `cron-rs daemon` | Start the API server |
| `cron-rs status` | Show status of all tasks |
| `cron-rs doctor` | Diagnose common issues |
| `cron-rs regenerate` | Regenerate systemd units from DB |
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
| GET | `/api/v1/tasks/{id}/hooks` | List hooks for task |
| POST | `/api/v1/tasks/{id}/hooks` | Create hook |
| PUT | `/api/v1/hooks/{id}` | Update hook |
| DELETE | `/api/v1/hooks/{id}` | Delete hook |
| GET | `/api/v1/runs` | List runs |
| GET | `/api/v1/runs/{id}` | Get run details |
| GET | `/api/v1/tasks/{id}/runs` | List runs for task |
| GET | `/api/v1/schedule/preview` | Preview schedule times (`?expr=...&count=5`) |
| GET | `/api/v1/events` | SSE event stream |

## Configuration

Configuration is stored in `~/.cron-rs/.env` (created by `cron-rs init`).

| Variable | Default | Description |
|----------|---------|-------------|
| `CRON_RS_USERNAME` | `admin` | Login username |
| `CRON_RS_PASSWORD` | -- | Login password (set by init) |
| `CRON_RS_JWT_SECRET` | -- | JWT signing secret (set by init) |
| `CRON_RS_HOST` | `127.0.0.1` | Bind address |
| `CRON_RS_PORT` | `9746` | Bind port |
| `CRON_RS_DB` | `~/.cron-rs/cron-rs.db` | Database path |
| `CRON_RS_TOKEN_EXPIRY` | `24h` | JWT token expiry |
| `CRON_RS_CONFIG_DIR` | `~/.cron-rs` | Config directory |
| `CRON_RS_CORS_ORIGIN` | -- | CORS origin (for external web UI) |

## Web Dashboard

For a web dashboard, see [cron-rs-web](https://github.com/dickwu/cron-rs-web).

## License

MIT

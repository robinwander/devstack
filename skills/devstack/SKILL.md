---
name: devstack
description: Use the devstack CLI/daemon to initialize devstack.toml, run stacks, inspect runs, and manage local dev environments in a repo.
metadata:
  short-description: Local dev orchestration with devstack
---

# Devstack

## When to use
Use this skill when a user asks to set up or operate devstack in a repo: create or edit a devstack config, start services, inspect status/logs, or manage runs/globals.

## Quick workflow
1) Discover config
- Look for `devstack.toml`, `devstack.yaml`, or `devstack.yml` in the project root.
- If none exists, run `devstack init` in the project root to create a starter `devstack.toml`.

2) Ensure the daemon is available
- Recommended: `devstack install` (installs and starts the user-level systemd service).
- Debug mode: `devstack daemon` (foreground in a terminal).
- Health check: `devstack doctor`.

3) Start a stack
- `devstack up [<stack>] [--stack <name>] [--all] [--new] [--force] [--project <path>] [--run <id>] [--file <path>] [--no-wait]`
- Use `--no-wait` if you only want to kick off services without waiting on readiness.

4) Inspect and operate
- `devstack ls [--all]`
- `devstack status [--run <id>]`
- `devstack watch` — show auto-restart watcher status per service
- `devstack watch pause [--service <name>]` / `devstack watch resume [--service <name>]`
- `devstack diagnose [--run <id>] [--service <name>]` — deep diagnostics including port binding, systemd state, and recent errors
- `devstack logs [<target>] [--run <id>] [--service <svc>] [--last N] [--follow] [--follow-for 15s]` — `<target>` is a positional service or source name (equivalent to `--service <name>`)
- `devstack logs --service <svc> --no-noise` — filter noisy health check requests (alias: `--no-health`)
- `devstack logs --service <svc> --errors` — alias for `--level error`
- `devstack logs --source <name> [--last N] [--search <query>] [--level <level>] [--stream <stream>] [--since <duration|iso8601>]`
- `devstack logs --source <name> --facets` — show available field values (services, levels, streams) with counts
- `devstack logs --all --facets` — discover queryable fields across all services in the run
- `devstack logs --task <name>` — show logs for a task
- `devstack lint [--project <path>] [--file <path>]` — validate config without starting anything

5) Run tasks
- `devstack run` — list available tasks
- `devstack run <task>` — execute a named task from [tasks] in config
- `devstack run <task> --verbose` — stream stdout/stderr to the terminal (default: captured)
- `devstack run <task> --detach` — hand the task to the daemon and return immediately with an `execution_id` + resolved `run_id`
- `devstack run --status <task-id>` — query a detached task execution (state, started/finished, exit code, duration)
- `devstack run --init` — run all init tasks for the current stack
- `devstack run --init --stack <name>` — run init tasks for a specific stack
- Tasks support `watch` patterns for skip-if-unchanged semantics
- `--detach` is the right primitive for long-running tasks from inside an agent workflow — don't block a tool call on them, poll `--status` instead

6) Manage projects
- `devstack projects ls` — list all registered projects
- `devstack projects add [<path>]` — register a project (default: current directory)
- `devstack projects remove <id|path|name>` — remove a project from the ledger

7) Manage external log sources
- `devstack sources add <name> <path>...` — register external JSONL files (globs supported)
- **Quote globs** to prevent shell expansion: `devstack sources add mem '/tmp/*.jsonl'`. If you pass an unquoted `*.jsonl`, your shell expands it first and only the matched-at-that-moment files are registered as literal paths — new files matching the glob won't be picked up later.
- `devstack sources rm <name>` — remove a source
- `devstack sources ls` — list registered sources
- External sources are JSONL files not managed by devstack (e.g. app logs, agent logs). The shim writes JSON lines; external sources must also be JSONL. When an app outputs JSON, fields are merged into the envelope. When plain text, the shim wraps it in a JSON object with `time`, `stream`, and `msg`.
- The daemon periodically re-ingests registered sources so **new files** matching globs are picked up automatically. **Appended content to already-ingested files is not re-read** by the periodic re-ingest — if you need to refresh, `sources rm` + `sources add` to re-index everything matching the glob.
- The envelope `service` field is derived from the source name when the source resolves to a single path, and from the matched filename stem (e.g. `app.jsonl` → `app`) when the source is a glob matching multiple files. The original `service` field inside the JSONL payload is moved to attributes and not promoted.
- Query with `devstack logs --source <name>` — same flags as run logs (`--last`, `--search`, `--level`, `--since`, `--service`).
- Discover available facets with `devstack logs --source <name> --facets` before querying.

8) Open dashboard
- `devstack ui` — opens the devstack dashboard in browser at http://localhost:47832

9) Share a log view with the user (agent → user)
- `devstack show` — posts a navigation intent to the daemon (`POST /v1/navigation/intent`) and opens the dashboard at a pre-filtered log view
- Use this to **show the user** something interesting — errors, specific service output, search results
- `devstack show --service api --level error` — show api errors
- `devstack show --service worker --search "timeout"` — show worker logs matching "timeout"
- `devstack show --run <id> --service api --since 5m` — show recent api logs for a specific run
- The dashboard polls the intent and applies it on arrival, then clears it, so refresh won't re-apply
- **This is the preferred way to share log context with the user** instead of dumping raw log output

10) Stop or clean up
- `devstack down [--run <id>] [--purge]`
- `devstack kill [--run <id>]` (if hung)
- `devstack gc [--older-than 7d] [--all]` — no dry-run flag; check `devstack ls --all` first to see what would be removed

## Log format
Devstack writes all logs as JSON lines. The shim wraps each line of service output:
- **Plain text** → `{"time":"...","stream":"stdout","msg":"server started on port 3000"}`
- **JSON output** → fields merged into envelope. App fields win except `time` and `stream` (shim always sets those).
- Pino numeric levels are normalized: 10=trace, 20=debug, 30=info, 40=warn, 50=error, 60=fatal.
- Parsing falls back to bracket format `[ts] [stream] msg` for backward compat with older log files.

## Config essentials
Project config lives at `devstack.toml` (or `.yaml`/`.yml`). Minimal example:
Relative paths (e.g. `cwd`, `env_file`, `watch`, `ignore`) are resolved against the directory containing the config file.

```toml
version = 1

[stacks.dev.services.api]
cmd = "pnpm api"
env_file = ".env.local"
watch = ["src/**", "Cargo.toml"]
ignore = ["**/*.tmp"]
auto_restart = true

[stacks.dev.services.api.readiness.http]
path = "/health"
expect_status = [200, 399]

[stacks.dev.services.web]
cmd = "pnpm dev"
deps = ["api"]

[stacks.dev.services.web.env]
VITE_API_URL = "{{ services.api.url }}"

[globals.db]
cmd = "docker compose up"
readiness = { tcp = {} }

[tasks.build]
cmd = "cargo build"
watch = ["src/**", "Cargo.toml"]

[tasks.migrate]
cmd = "pnpm db:migrate"
```

Defaults:
- `scheme`: `http`
- `port_env`: `PORT`
- `port`: auto-allocated unless `port: none`
- `readiness`: TCP connect to `localhost:PORT`
- `env_file`: `.env` in the service `cwd` (if present)

Env load order:
- `DEV_*` vars are always set by devstack and cannot be overridden by env files.
- `env` from config overrides values from `env_file`.

Readiness options (exactly one per service):
- `tcp`, `http`, `log_regex`, `cmd`
- `delay_ms`: wait a fixed delay before marking ready
- `exit`: wait for a one-shot command to exit successfully
- `timeout_ms`: override the default 30s readiness timeout
Note: TCP/HTTP readiness probes `127.0.0.1`. If a service binds only to `localhost`/`::1`, pass a host like `--host 127.0.0.1` (Vite) or `--host 0.0.0.0`.
Note: `delay_ms` does not validate process health; prefer `tcp`/`http`/`log_regex` when possible.

### Tasks configuration

Tasks are defined in the `[tasks]` section. They support two forms:

**Short form** (command only):
```toml
[tasks.lint]
cmd = "pnpm lint"
```

**Structured form** (all options):
```toml
[tasks.build]
cmd = "cargo build"
cwd = "packages/api"
env = { RUST_LOG = "debug" }
env_file = ".env.build"
watch = ["src/**", "Cargo.toml"]
```

Task fields:
- `cmd` (required): Shell command to run
- `cwd`: Working directory (relative to config file)
- `env`: Map of environment variables
- `env_file`: Path to dotenv file (relative to config file)
- `watch`: List of file patterns; if provided, task computes a hash and skips if unchanged

Services can reference init tasks in their `init` field:
```toml
[stacks.dev.services.api]
cmd = "pnpm dev"
init = ["migrate", "seed"]
```

### Templating

Minijinja templates work in `cmd`, `cwd`, `env_file`, `watch`, `ignore`, and `env` values:
- `{{ run.id }}`, `{{ project.dir }}`, `{{ stack.name }}`
- `{{ services.<name>.port }}`, `{{ services.<name>.url }}`

### Ignore and watch patterns

Ignore sources (applied in order): `.gitignore`, `.ignore`, `.devstackignore`, plus per-service `ignore` (gitignore syntax, `!` supported).
If `watch` is set on a service, only matching paths are considered for change detection.
Set `auto_restart = true` to enable live file watching + automatic service restart; this requires non-empty `watch` patterns.

## CLI flag reference

Flag names below are the canonical form. Common aliases: `--run-id` → `--run`, `--tail` → `--last`, `--q` → `--search`, `--no-health` → `--no-noise`.

### Global flags
- _(none)_ — there is no global `--pretty` or `--json` flag. Output format is per-subcommand.

### devstack up
- `[<stack>]` — Stack name (positional, conflicts with `--stack`)
- `--stack <name>` — Stack name (flag form)
- `--all` — Start every stack in config
- `--new` — Force new run (don't reuse existing)
- `--force` — Restart all services even if unchanged
- `--project <path>` — Project directory
- `--run <id>` — Specific run ID (alias: `--run-id`)
- `--file <path>` — Config file path
- `--no-wait` — Don't wait for readiness

### devstack status
- `--run <id>` — Specific run (alias: `--run-id`)

### devstack watch
- _(no flags)_ — Show auto-restart watcher status per service
- `pause [--service <name>]` — Pause auto-restart for one or all services
- `resume [--service <name>]` — Resume auto-restart for one or all services

### devstack diagnose
- `--run <id>` — Specific run (alias: `--run-id`)
- `--service <name>` — Diagnose specific service only

### devstack logs
- `[<target>]` — Positional service or source name (equivalent to `--service <name>`)
- `--run <id>` — Run scope (alias: `--run-id`)
- `--source <name>` — Query external source (conflicts with `--run`, `--all`, `--task`)
- `--facets` — Show available field values (conflicts with `--follow`, `--last`, `--task`)
- `--all` — Search all services in run
- `--service <name>` — Specific service (works with both run-scoped and source-scoped queries)
- `--task <name>` — Show task logs
- `--last <N>` — Last N lines (default: 500, or 200 with `--follow`; alias: `--tail`)
- `--search <query>` — Tantivy query string (boolean ops, phrases; alias: `--q`)
- `--level <all|warn|error>` — Filter by level
- `--errors` — Alias for `--level error` (hidden from `--help` but supported)
- `--stream <stdout|stderr>` — Filter by stream
- `--since <timestamp|duration>` — RFC3339 or duration like "5m", "1h"
- `--no-noise` — Filter health check noise (alias: `--no-health`)
- `--follow` — Stream new logs (requires `--service`)
- `--follow-for <duration>` — Follow timeout (default: 15s in non-interactive)
- `devstack logs` emits **JSON lines** by default (each line is a self-contained JSON object). Other subcommands (`status`, `ls`, `sources`, etc.) emit a custom structured format, not JSON.

### devstack down
- `--run <id>` — Specific run (alias: `--run-id`)
- `--purge` — Remove run directory after stopping

### devstack kill
- `--run <id>` — Specific run (alias: `--run-id`)

### devstack exec
- `--run <id>` — Run to use for environment (alias: `--run-id`)
- `-- <command...>` — Command and arguments (required)

### devstack lint
- `--project <path>` — Project directory
- `--file <path>` — Config file

### devstack gc
- `--older-than <duration>` — e.g., "7d", "24h"
- `--all` — Remove all stopped runs

### devstack show
- `--run <id>` — Target run (alias: `--run-id`)
- `--service <name>` — Filter to a specific service
- `--search <query>` — Full-text search query (alias: `--q`)
- `--level <all|warn|error>` — Filter by level
- `--stream <stdout|stderr>` — Filter by stream
- `--since <timestamp|duration>` — Time filter (e.g. "5m", "1h", RFC3339)
- `--last <N>` — Show last N lines (alias: `--tail`)

### devstack init
- `--project <path>` — Project directory
- `--file <path>` — Custom config path

### devstack run
- `[<task>]` — Task name (omit to list available)
- `--init` — Run all init tasks for the stack
- `--stack <name>` — Stack for init tasks (requires `--init`)
- `--project <path>` — Project directory
- `--file <path>` — Config file
- `--verbose` — Stream stdout/stderr to terminal (default: capture to log)
- `--detach` — Hand the task to the daemon and return immediately with an `execution_id` (conflicts with `--init`, `--status`, `--verbose`)
- `--status <task-id>` — Query a detached task execution by id (returns `state`, `started_at`, `finished_at`, `exit_code`, `duration_ms`)
- `-- <args...>` — Extra arguments passed to the task command (after `--`)

### devstack agent
- `--auto-share <error|warn>` — Auto-share service logs at this level or above into the wrapped agent's stdin
- `--no-auto-share` — Disable auto-sharing entirely (conflicts with `--auto-share`)
- `--watch <svc1,svc2>` — Comma-separated service list; restrict auto-sharing to these services only (default: all services in the run)
- `--run <id>` — Target run id (alias: `--run-id`); default is the latest non-stopped run for the current project
- `-- <command...>` — Agent command and arguments (required, after `--`)

### devstack projects
- `ls` — List registered projects
- `add [<path>]` — Register project (default: current directory)
- `remove <id|path|name>` — Remove project

### devstack sources
- `ls` — List registered sources
- `add <name> <path>...` — Register source with file patterns
- `rm <name>` — Remove source

## Agent guidance
- Prefer `devstack init` to create a baseline config, then fill in services based on repo signals (package.json scripts, docker-compose, etc.).
- On macOS, LaunchAgents inherit a minimal PATH; use absolute command paths or ensure tools like `pnpm`/`poetry` are on PATH for the daemon.
- `devstack ls` filters to the current project by default; use `--all` to see everything.
- If `--run` is omitted, the most recent run for the current project is used.
- If `--stack` is omitted and the config defines exactly one stack, that stack is used.
- You can set `default_stack = "<name>"` to choose the default when multiple stacks exist.
- `devstack up --all` starts every stack in the config.
- `devstack up` reuses an existing run for the same stack/project and restarts only services whose watched files or config changed.
- Use `--force` to restart everything, or `--new` to run in parallel.
- Use `devstack ls` and `devstack status` to avoid guessing current run IDs.
- Use `devstack diagnose` when services fail to start — it checks port binding, systemd state, and recent logs.
- Use `devstack lint` to validate config changes without starting services.
- Output format is per-subcommand: `devstack logs` emits JSON lines; `status`, `ls`, `sources`, `projects`, etc. emit a custom structured format (`runs[2]{run_id,stack,...}:` style). **There is no global `--pretty` or `--json` flag.**
- `devstack logs --follow` defaults to a 15s timeout in non-interactive shells; use `--follow-for` to override.
- Use `--facets` to discover what's queryable before writing `--search` filters. Works with both `--source` and run-scoped logs.
- Use `--no-noise` (alias `--no-health`) to filter out repetitive health check requests from logs.
- Use `--errors` as a quick alias for `--level error`.
- **Search field syntax:** field values containing `:` must be double-quoted in the query string: `--search 'stream:"post_init:moto-init"'`. Backslash escaping (`stream:post_init\:moto-init`) does not work. Querying a field that doesn't exist in the index returns `400 Bad Request Field does not exist: '<name>'`, not an empty result set.
- **Pipe guardrail:** devstack blocks piping its own output to `head`/`tail` (`"Use devstack's own limiting flags instead of piping to head/tail"`). Use `--last <N>` / `--follow-for <dur>` to bound output, or redirect to a file if you genuinely need to process the full output.
- **Use `devstack show` to share log views with the user.** Instead of pasting log output, send a filtered dashboard view — the user sees it live in their browser. Example: `devstack show --service api --level error --since 5m`.
- **Use `devstack agent -- <cmd>` when running an interactive agent.** It enables the full bidirectional channel: `--auto-share error` surfaces new errors into the agent's session automatically, and the dashboard's Share button lights up so the user can push refined log queries back into your terminal. Without the wrapper, only the agent→user direction (`show`) works.

## When to restart (and when not to)

- **Changed source code?** → Do nothing. All dev servers use HMR.
- **Need to check health?** → `devstack status`
- **Something broken?** → `devstack up` (converges, doesn't recreate)
- **Never use:** `devstack down` or `devstack kill` unless explicitly asked
- **Don't loop** `devstack logs` per service — `devstack status` shows recent errors inline

## Shell completions
- `devstack completions bash`, `devstack completions zsh`, or `devstack completions fish` prints a completion script that uses the daemon for dynamic suggestions.
- Keep config edits minimal and targeted; avoid changing unrelated services without explicit user request.
- When testing, clean up with `devstack down` and optionally `devstack gc`.

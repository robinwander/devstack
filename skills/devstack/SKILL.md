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
- `devstack up [<stack>] [--stack <name>] [--all] [--new] [--force] [--project <path>] [--run-id <id>] [--file <path>] [--no-wait]`
- Use `--no-wait` if you only want to kick off services without waiting on readiness.

4) Inspect and operate
- `devstack ls [--all]`
- `devstack status [--run-id <id>]`
- `devstack watch` — show auto-restart watcher status per service
- `devstack watch pause [--service <name>]` / `devstack watch resume [--service <name>]`
- `devstack diagnose [--run-id <id>] [--service <name>]` — deep diagnostics including port binding, systemd state, and recent errors
- `devstack logs [--run-id <id>] --service <svc> [--tail N] [--follow] [--follow-for 15s] [--json]`
- `devstack logs --service <svc> --no-health` — filter noisy health check requests
- `devstack logs --service <svc> --errors` — alias for `--level error`
- `devstack logs --source <name> [--tail N] [--q <query>] [--level <level>] [--stream <stream>] [--since <iso8601>]`
- `devstack logs --source <name> --facets` — show available field values (services, levels, streams) with counts
- `devstack logs --task <name>` — show logs for a task
- `devstack lint [--project <path>] [--file <path>]` — validate config without starting anything

5) Run tasks
- `devstack run` — list available tasks
- `devstack run <task>` — execute a named task from [tasks] in config
- `devstack run --init` — run all init tasks for the current stack
- `devstack run --init --stack <name>` — run init tasks for a specific stack
- Tasks support `watch` patterns for skip-if-unchanged semantics

6) Manage projects
- `devstack projects ls` — list all registered projects
- `devstack projects add [<path>]` — register a project (default: current directory)
- `devstack projects remove <id|path|name>` — remove a project from the ledger

7) Manage external log sources
- `devstack sources add <name> <path>...` — register external JSONL files (globs supported)
- `devstack sources rm <name>` — remove a source
- `devstack sources ls` — list registered sources
- External sources are JSONL files not managed by devstack (e.g. app logs, agent logs). The shim writes JSON lines; external sources must also be JSONL. When an app outputs JSON, fields are merged into the envelope. When plain text, the shim wraps it in a JSON object with `time`, `stream`, and `msg`.
- The daemon periodically re-ingests registered sources so new files matching globs are picked up automatically.
- Query with `devstack logs --source <name>` — same flags as run logs (`--tail`, `--q`, `--level`, `--since`).
- Discover available facets with `devstack logs --source <name> --facets` before querying.

8) Open dashboard
- `devstack ui` — opens the devstack dashboard in browser at http://localhost:47832

9) Stop or clean up
- `devstack down [--run-id <id>] [--purge]`
- `devstack kill [--run-id <id>]` (if hung)
- `devstack gc [--older-than 7d] [--all]`

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

### Global flags
- `--pretty` — Force pretty JSON even when non-interactive

### devstack up
- `[<stack>]` — Stack name (positional, conflicts with `--stack`)
- `--stack <name>` — Stack name (flag form)
- `--all` — Start every stack in config
- `--new` — Force new run (don't reuse existing)
- `--force` — Restart all services even if unchanged
- `--project <path>` — Project directory
- `--run-id <id>` — Specific run ID
- `--file <path>` — Config file path
- `--no-wait` — Don't wait for readiness

### devstack status
- `--run-id <id>` — Specific run
- `--json` — Force JSON output (even on TTY)

### devstack diagnose
- `--run-id <id>` — Specific run
- `--service <name>` — Diagnose specific service only

### devstack logs
- `--run-id <id>` — Run scope
- `--source <name>` — Query external source (conflicts with run flags)
- `--facets` — Show available field values (conflicts with follow/tail/q/task)
- `--all` — Search all services in run
- `--service <name>` — Specific service
- `--task <name>` — Show task logs
- `--tail <N>` — Last N lines (default: 500, or 200 with --follow)
- `--q <query>` — Tantivy query string (boolean ops, phrases)
- `--level <all|warn|error>` — Filter by level
- `--errors` — Alias for `--level error`
- `--stream <stdout|stderr>` — Filter by stream
- `--since <timestamp|duration>` — RFC3339 or duration like "5m", "1h"
- `--no-health` — Filter health check noise
- `--follow` — Stream new logs (requires --service)
- `--follow-for <duration>` — Follow timeout (default: 15s in non-interactive)
- `--json` — Output JSON

### devstack down
- `--run-id <id>` — Specific run
- `--purge` — Remove run directory after stopping

### devstack kill
- `--run-id <id>` — Specific run

### devstack exec
- `--run-id <id>` — Run to use for environment
- `-- <command...>` — Command and arguments (required)

### devstack lint
- `--project <path>` — Project directory
- `--file <path>` — Config file

### devstack gc
- `--older-than <duration>` — e.g., "7d", "24h"
- `--all` — Remove all stopped runs

### devstack init
- `--project <path>` — Project directory
- `--file <path>` — Custom config path

### devstack run
- `[<task>]` — Task name (omit to list available)
- `--init` — Run all init tasks for the stack
- `--stack <name>` — Stack for init tasks (requires --init)
- `--project <path>` — Project directory
- `--file <path>` — Config file
- `--verbose` — Stream stdout/stderr to terminal (default: capture to log)
- `--json` — Output JSON result

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
- If `--run-id` is omitted, the most recent run for the current project is used.
- If `--stack` is omitted and the config defines exactly one stack, that stack is used.
- You can set `default_stack = "<name>"` to choose the default when multiple stacks exist.
- `devstack up --all` starts every stack in the config.
- `devstack up` reuses an existing run for the same stack/project and restarts only services whose watched files or config changed.
- Use `--force` to restart everything, or `--new` to run in parallel.
- Use `devstack ls` and `devstack status` to avoid guessing current run IDs.
- Use `devstack diagnose` when services fail to start — it checks port binding, systemd state, and recent logs.
- Use `devstack lint` to validate config changes without starting services.
- Default output is pretty JSON on a TTY and compact JSON when non-interactive (`--pretty` forces pretty).
- `devstack logs --follow` defaults to a 15s timeout in non-interactive shells; use `--follow-for` to override.
- Use `--facets` to discover what's queryable before writing `--q` filters. Works with both `--source` and run-scoped logs.
- Use `--no-health` to filter out repetitive health check requests from logs.
- Use `--errors` as a quick alias for `--level error`.

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
---
name: devstack
description: Use when you are working in a devstack enabled project and need to manage applications, query logs, debug local dev issues, etc.
metadata:
  short-description: Local dev orchestration with devstack
---

# Devstack

Devstack makes the running, managing, and debugging of locally running projects easy. All devstack commands run in the context of the project for the cwd (devstack does project discovery similar to npm)

## Quick workflow
1) Check if a stack is already running
- Generally the user will already be running a stack, so you can just use that one. You should generally not be manually starting and stopping stacks. Most services are running in HMR or in watch mode for devstack, so you don't need to restart things for changes to take effect.
- `devstack status`
- this will tell you the state of services and the urls + ports of each service in the stack which you can use to test

### Start a stack if one isn't running already
- `devstack up` will start the stack for the project of your cwd, so you don't need to specify flags in most cases
- If its already running, it will update and restart any services with changes that aren't in watch mode
- This will wait for services to become ready or fail readiness before returning, so generally its better to use this than manually poll for services being ready

### Inspect and operate
**Operations**
- `devstack ls` shows the current runs (parallel running stacks) for the project
- `devstack status` shows the status of the currently running stack, recent errors, and the urls of the apps for testing
- `devstack watch` — show auto-restart watcher status per service
- `devstack watch pause [--service <name>]` / `devstack watch resume [--service <name>]`
- `devstack diagnose [--run <id>] [--service <name>]` — deep diagnostics including port binding, systemd state, and recent errors
- `devstack logs [<target>] [--run <id>] [--service <svc>] [--last N] [--follow] [--follow-for 15s]` — `<target>` is a positional service or source name (equivalent to `--service <name>`)
**Logs**
Logs are json always, and its better to use native devstack filters 
- `devstack logs <svc> --no-noise` — filter noisy health check requests (alias: `--no-health`)
- `devstack logs <svc> --errors` — alias for `--level error`
- `devstack logs <svc> [--last N] [--search <query>] [--level <level>] [--stream <stream>] [--since <duration|iso8601>]`
- `devstack logs <svc> --facets` — show available field values (services, levels, streams) with counts
- `devstack logs --all --facets` — discover queryable fields across all services in the run
- `devstack logs --task <name>` — show logs for a task
- `devstack lint [--project <path>] [--file <path>]` — validate config without starting anything

### Run tasks
- `devstack run` — list available tasks
- `devstack run <task>` — execute a named task from [tasks] in config
- `devstack run <task> --verbose` — stream stdout/stderr to the terminal (default: captured)
- `devstack run <task> --detach` — hand the task to the daemon and return immediately with an `execution_id` + resolved `run_id`
- `devstack run --status <task-id>` — query a detached task execution (state, started/finished, exit code, duration)
- `devstack run --init` — run all init tasks for the current stack
- `devstack run --init --stack <name>` — run init tasks for a specific stack
- Tasks support `watch` patterns for skip-if-unchanged semantics
- `--detach` is the right primitive for long-running tasks from inside an agent workflow — don't block a tool call on them, poll `--status` instead

5) Manage projects
- `devstack projects ls` — list all registered projects
- `devstack projects add [<path>]` — register a project (default: current directory)
- `devstack projects remove <id|path|name>` — remove a project from the ledger

### Manage external log sources
- `devstack sources add <name> <path>...` — register external JSONL files (globs supported)
- **Quote globs** to prevent shell expansion: `devstack sources add mem '/tmp/*.jsonl'`. If you pass an unquoted `*.jsonl`, your shell expands it first and only the matched-at-that-moment files are registered as literal paths — new files matching the glob won't be picked up later.
- `devstack sources rm <name>` — remove a source
- `devstack sources ls` — list registered sources
- External sources are JSONL files not managed by devstack (e.g. app logs, agent logs). The shim writes JSON lines; external sources must also be JSONL. When an app outputs JSON, fields are merged into the envelope. When plain text, the shim wraps it in a JSON object with `time`, `stream`, and `msg`.
- The daemon periodically re-ingests registered sources so **new files** matching globs are picked up automatically. **Appended content to already-ingested files is not re-read** by the periodic re-ingest — if you need to refresh, `sources rm` + `sources add` to re-index everything matching the glob.
- The envelope `service` field is derived from the source name when the source resolves to a single path, and from the matched filename stem (e.g. `app.jsonl` → `app`) when the source is a glob matching multiple files. The original `service` field inside the JSONL payload is moved to attributes and not promoted.
- Query with `devstack logs --source <name>` — same flags as run logs (`--last`, `--search`, `--level`, `--since`, `--service`).
- Discover available facets with `devstack logs --source <name> --facets` before querying.

### Open dashboard
- `devstack ui` — opens the devstack dashboard in browser at http://localhost:47832

### Share a log view with the user (agent → user)
- `devstack show` — posts a navigation intent to the daemon (`POST /v1/navigation/intent`) and opens the dashboard at a pre-filtered log view
- Use this to **show the user** something interesting — errors, specific service output, search results
- `devstack show --service api --level error` — show api errors
- `devstack show --service worker --search "timeout"` — show worker logs matching "timeout"
- `devstack show --run <id> --service api --since 5m` — show recent api logs for a specific run
- The dashboard polls the intent and applies it on arrival, then clears it, so refresh won't re-apply
- **This is the preferred way to share log context with the user** instead of dumping raw log output


## Agent guidance
- Prefer `devstack init` to create a baseline config, then fill in services based on repo signals (package.json scripts, docker-compose, etc.).
- On macOS, LaunchAgents inherit a minimal PATH; use absolute command paths or ensure tools like `pnpm`/`poetry` are on PATH for the daemon.
- `devstack ls` filters to the current project by default; use `--all` to see everything.
- Most flags are optional and devstack will use the most reasonable defaults if not specified.
- If `--run` is omitted, the most recent run for the current project is used.
- If `--stack` is omitted and the config defines exactly one stack, that stack is used.
- You can set `default_stack = "<name>"` to choose the default when multiple stacks exist.
- `devstack up` reuses an existing run for the same stack/project and restarts only services whose watched files or config changed.
- Use `--force` to restart everything, or `--new` to run in parallel.
- Use `devstack ls` and `devstack status` to avoid guessing current run IDs.
- Use `devstack diagnose` when services fail to start — it checks port binding, systemd state, and recent logs.
- Use `devstack lint` to validate config changes without starting services.
- Output format is per-subcommand: `devstack logs` return JSON lines; `status`, `ls`, `sources`, `projects`, etc. return a custom structured format (`runs[2]{run_id,stack,...}:` style).
- `devstack logs --follow` defaults to a 15s timeout in non-interactive shells; use `--follow-for` to override.
- Use `--facets` to discover what's queryable before writing `--search` filters. Works with both `--source` and run-scoped logs.
- Use `--no-noise` (alias `--no-health`) to filter out repetitive health check requests from logs.
- Use `--errors` as a quick alias for `--level error`.
- **Search field syntax:** field values containing `:` must be double-quoted in the query string: `--search 'stream:"post_init:moto-init"'`. Backslash escaping (`stream:post_init\:moto-init`) does not work. Querying a field that doesn't exist in the index returns `400 Bad Request Field does not exist: '<name>'`, not an empty result set.
- **Pipe guardrail:** devstack blocks piping its own output to `head`/`tail` (`"Use devstack's own limiting flags instead of piping to head/tail"`). Use `--last <N>` / `--follow-for <dur>` to bound output, or redirect to a file if you genuinely need to process the full output.
- **Use `devstack show` to share log views with the user.** Instead of pasting log output, send a filtered dashboard view — the user sees it live in their browser. Example: `devstack show --service api --level error --since 5m`.


## When to restart (and when not to)
- **Changed source code?** → Do nothing generally. Most dev servers use HMR or are in watch mode. So it will only slow you down and waste tokens to manually stop and start services.
- **Need to check health?** → `devstack status`
- **Something broken?** → `devstack up` (converges, doesn't recreate)
- **Never use:** `devstack down` or `devstack kill` unless explicitly asked



## Config essentials
Project config lives at `devstack.toml` (or `.yaml`/`.yml`). 
Relative paths (e.g. `cwd`, `env_file`, `watch`, `ignore`) are resolved against the directory containing the config file.


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
- `--run <id>` — Specific run ID
- `--file <path>` — Config file path
- `--no-wait` — Don't wait for readiness

### devstack status
- `--run <id>` — Specific run

### devstack watch
- _(no flags)_ — Show auto-restart watcher status per service
- `pause [--service <name>]` — Pause auto-restart for one or all services
- `resume [--service <name>]` — Resume auto-restart for one or all services

### devstack diagnose
- `--run <id>` — Specific run
- `--service <name>` — Diagnose specific service only

### devstack logs
- `[<target>]` — Positional service or source name (equivalent to `--service <name>`)
- `--run <id>` — Run scope
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
- `--run <id>` — Specific run
- `--purge` — Remove run directory after stopping

### devstack kill
- `--run <id>` — Specific run

### devstack exec
- `--run <id>` — Run to use for environment
- `-- <command...>` — Command and arguments (required)

### devstack lint
- `--project <path>` — Project directory
- `--file <path>` — Config file

### devstack gc
- `--older-than <duration>` — e.g., "7d", "24h"
- `--all` — Remove all stopped runs

### devstack show
- `--run <id>` — Target run
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
- `--run <id>` — Target run id; default is the latest non-stopped run for the current project
- `-- <command...>` — Agent command and arguments (required, after `--`)

### devstack projects
- `ls` — List registered projects
- `add [<path>]` — Register project (default: current directory)
- `remove <id|path|name>` — Remove project

### devstack sources
- `ls` — List registered sources
- `add <name> <path>...` — Register source with file patterns
- `rm <name>` — Remove source

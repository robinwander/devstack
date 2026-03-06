# devstack

Local development orchestration with automatic port allocation, structured logs, and file watching. Think docker-compose for local dev, but services never hardcode ports, dependencies start in order, and you get full-text search across all logs.

## Features

- **Daemon-managed lifecycle** — services start in dependency order, restart automatically, and are guaranteed cleanup via systemd (Linux) or LaunchAgent (macOS)
- **Automatic port allocation** — the daemon picks free ports and injects them via env vars; services reference each other by name
- **Readiness probes** — TCP, HTTP, log regex, custom command, fixed delay, or exit-based
- **File watching** — incremental restarts: only services with changed files restart; powered by gitignore-style globs
- **Structured JSONL logs** — full-text search via tantivy, facet-based filtering, multi-run aggregation
- **External log sources** — register arbitrary JSONL files and query them alongside devstack logs
- **Guaranteed orphan cleanup** — Linux uses systemd transient units with cgroups; macOS uses LaunchAgent
- **Minijinja templating** — reference service URLs and ports in env vars and commands
- **Shell completions** — bash, zsh, fish with dynamic suggestions
- **Dual output modes** — pretty JSON on TTYs, compact JSON for scripting

## Quick Start

**1. Clone and install the CLI** (requires [Rust](https://rustup.rs/)):

```bash
git clone https://github.com/robinwander/devstack.git ~/tools/devstack
cd ~/tools/devstack
./scripts/install-cli.sh   # builds release binary to ~/.local/bin
```

**2. Install and start the daemon:**

```bash
devstack install           # Linux: systemd user service; macOS: LaunchAgent
devstack doctor            # verify everything is working
```

**3. Initialize a config:**

```bash
cd your-project
devstack init
```

**4. Start your stack:**

```bash
devstack up
```

## Configuration

Config can be `devstack.toml`, `devstack.yml`, or `devstack.yaml` (nearest file walking up from cwd). `version = 1` is required.

Example web+api+db stack:

```toml
version = 1
default_stack = "dev"

[stacks.dev.services.db]
cmd = "docker run --rm -p $PORT:5432 postgres:16"
port_env = "PORT"
readiness = { tcp = {} }

[stacks.dev.services.api]
cmd = "pnpm api"
deps = ["db"]
env_file = ".env.local"
watch = ["src/**", "prisma/**"]

[stacks.dev.services.api.env]
DATABASE_URL = "postgres://localhost:{{ services.db.port }}/myapp"

[stacks.dev.services.api.readiness.http]
path = "/health"
expect_status = [200, 299]

[stacks.dev.services.web]
cmd = "pnpm dev"
deps = ["api"]
watch = ["src/**", "vite.config.ts"]

[stacks.dev.services.web.env]
VITE_API_URL = "{{ services.api.url }}"

[globals.redis]
cmd = "redis-server --port $PORT"
readiness = { tcp = {} }

[tasks.migrate]
cmd = "prisma migrate dev"
watch = ["prisma/schema.prisma"]
```

### Stacks and Services

- Each stack defines services that start together
- `default_stack` selects the stack used when `devstack up` omits a stack name
- Services declare dependencies via `deps`; startup order is topologically sorted
- Globals are singleton services shared across all stacks in a project

Service fields:

| Field | Type | Default | Notes |
|------|------|---------|-------|
| `cmd` | string | required | Command run via `/bin/bash -lc` |
| `deps` | string[] | `[]` | Service dependencies in same stack |
| `cwd` | path | project dir | Working directory (templated) |
| `scheme` | string | `http` | Used for generated URLs |
| `port` | int or `"none"` | auto-allocate | Fixed port, dynamic port, or no port |
| `port_env` | string | `PORT` | Env var receiving allocated port |
| `readiness` | table | inferred | See readiness options below |
| `env_file` | path | `<cwd>/.env` | Optional dotenv file (templated) |
| `env` | map | `{}` | Inline env vars (templated values) |
| `watch` | string[] | all files under cwd | Paths/patterns to hash for refresh decisions |
| `ignore` | string[] | `[]` | Extra ignore patterns on top of ignore files |
| `init` | string[] | none | Tasks to run before service start |

### Readiness Options

| Type | Description | Example |
|------|-------------|---------|
| `tcp` | TCP connect to allocated port (default for services with ports) | `readiness = { tcp = {} }` |
| `http` | HTTP GET with status range check | `readiness = { http = { path = "/health", expect_status = [200, 399] } }` |
| `log_regex` | Match pattern in stdout/stderr | `readiness = { log_regex = "listening on" }` |
| `cmd` | Custom shell command exits 0 | `readiness = { cmd = "pg_isready -h localhost -p $PORT" }` |
| `delay_ms` | Fixed delay (use sparingly) | `readiness = { delay_ms = 5000 }` |
| `exit` | One-shot command exits successfully | `readiness = { exit = {} }` |
| `timeout_ms` | Override 30s default | `readiness = { tcp = {}, timeout_ms = 60000 }` |

### Globals

Services under `[globals]` are singletons shared across all stacks in a project. Useful for databases, caches, or message brokers that multiple stacks share. Globals are started on demand and stay running until explicitly stopped, and use the same env-file/env interpolation behavior as normal services.

### Tasks

One-shot commands defined in `[tasks]` that can be run via `devstack run <task>`. Tasks support either shorthand:

```toml
[tasks.format]
cmd = "cargo fmt"
```

or string form:

```toml
tasks = { echo = "echo hello" }
```

Structured task fields:

| Field | Type | Default |
|------|------|---------|
| `cmd` | string | required |
| `cwd` | path | project dir |
| `watch` | string[] | `[]` |
| `env_file` | path | `<cwd>/.env` |
| `env` | map | `{}` |

Example:

```toml
[tasks.seed]
cmd = "tsx scripts/seed.ts"
env_file = ".env.local"

[tasks.lint]
cmd = "pnpm lint"
watch = ["src/**", "eslint.config.js"]
```

Tasks support the same `watch` patterns as services. When watched files haven't changed since the last run, the task is skipped. Tasks can also be declared as `init` for services:

```toml
[stacks.dev.services.api]
cmd = "pnpm api"
init = ["migrate"]  # runs before api starts
```

Run `devstack run --init` to execute all init tasks without starting services.

### Templating Variables

Minijinja templates work in `cmd`, `cwd`, `env_file`, `env` values, `watch`, and `ignore`:

- `{{ run.id }}` — unique run identifier
- `{{ project.dir }}` — absolute path to project directory
- `{{ stack.name }}` — current stack name
- `{{ services.<name>.port }}` — allocated port for service
- `{{ services.<name>.url }}` — full URL (scheme://host:port)

### Environment Injection Order

All services receive these automatically:

- `DEV_RUN_ID`, `DEV_STACK`, `DEV_PROJECT_DIR`
- `DEV_PORT_<SERVICE>`, `DEV_URL_<SERVICE>` for every service with a port
- `DEV_DEP_<DEP>_PORT`, `DEV_DEP_<DEP>_URL` shortcuts for direct dependencies

Merge order (later entries may override earlier ones):

1. Generated base `DEV_*` variables
2. `env_file` values (`env_file` path or `<cwd>/.env` by default); `DEV_*` keys from file are ignored
3. Generated dependency shortcuts (`DEV_DEP_*`)
4. Service port env (`port_env`, default `PORT`)
5. Inline `env` from config

After merge, `$VAR` and `${VAR}` references are resolved from the devstack process environment for all values. Missing variables are left as-is.

### Ignore and Watch Patterns

Ignore sources are applied in order: `.gitignore`, `.ignore`, `.devstackignore`, then per-service `ignore`. Patterns use gitignore syntax with `!` negation supported.

By default, services watch all files under their `cwd` (filtered by ignores). Set explicit `watch` to limit to specific paths:

```toml
watch = ["src/**", "Cargo.toml", "Cargo.lock"]
ignore = ["**/*.test.ts", "**/node_modules"]
```

## CLI Reference

### Lifecycle

| Command | Key flags |
|---------|-----------|
| `devstack up [STACK]` | `--stack`, `--all`, `--new`, `--force`, `--no-wait`, `--run-id`, `--project`, `--file` |
| `devstack down` | `--run-id`, `--purge` |
| `devstack kill` | `--run-id` |
| `devstack daemon` | Run daemon in foreground (useful for debugging) |

### Inspection & Logs

| Command | Key flags |
|---------|-----------|
| `devstack status` | `--run-id`, `--json` |
| `devstack ls` | `--all` |
| `devstack diagnose` | `--run-id`, `--service` |
| `devstack logs` | `--service`, `--task`, `--all`, `--source`, `--tail`, `--q`, `--level`, `--errors`, `--stream`, `--since`, `--facets`, `--follow`, `--follow-for`, `--no-health`, `--json` |

### Sources & Projects

| Command | Description |
|---------|-------------|
| `devstack sources add <name> <path>...` | Register JSONL files (globs supported) |
| `devstack sources rm <name>` / `sources ls` | Remove/list external sources |
| `devstack projects add [path]` / `projects ls` / `projects remove <id|path>` | Manage registered projects |

### Setup & Utilities

| Command | Description |
|---------|-------------|
| `devstack init` | Create starter config in current directory |
| `devstack install` | Install + start daemon (systemd user service / LaunchAgent) |
| `devstack doctor` | Verify daemon health and prerequisites |
| `devstack lint` | Validate config |
| `devstack exec -- <command>` | Run command in run environment |
| `devstack run [task]` | Run task, or list tasks when omitted |
| `devstack run --init` | Run all init tasks (`--stack` supported) |
| `devstack run --verbose --json` | Stream task output / structured result |
| `devstack gc` | Cleanup old runs/globals (`--older-than`, `--all`) |
| `devstack ui` | Open dashboard in browser |
| `devstack completions <shell>` | Generate shell completions |
| `devstack openapi --out openapi.json` | Emit OpenAPI spec (`--watch` supported) |

### Common flags

- `--pretty` is available on all commands.
- `--run-id`, `--project`, and `--file` are command-specific (not global), mainly on lifecycle/config-sensitive commands.

## Architecture

devstack has three runtime pieces:

- **CLI** — resolves config/project context and sends local HTTP requests over a Unix socket.
- **Daemon** (`devstack daemon`) — owns orchestration: dependency order, port allocation, readiness, health checks, run/global state, and log indexing.
- **Shim** (`devstack __shim`) — spawned as each service entrypoint; runs the real command, captures stdout/stderr, strips ANSI, writes JSONL.

Key behavior:

- `devstack up` is incremental: it computes a watch hash per service and only restarts changed/failed services on refresh.
- Logs are indexed with Tantivy for full-text queries and facets.
- External JSONL sources are ingested into the same search pipeline.

Runtime layout:

```
~/.local/share/devstack/                        # Linux
~/Library/Application Support/devstack/         # macOS
  daemon/
    devstackd.sock
    state.json
  runs/
    <run_id>/
      manifest.json
      devstack.yml.snapshot
      logs/<service>.log
      tasks/<task>.log
  globals/
    <project_hash>__<global>/
      manifest.json
      logs/<global>.log
  logs_index/
  dashboard/
```

Linux uses systemd transient units (cgroup-scoped lifecycle). macOS runs services under a LaunchAgent-managed daemon.

## Known Limitations / Caveats

- External source queries (`devstack logs --source <name>`) currently return source-level labels; per-file service identity is not preserved in CLI output.
- Change detection is hash-based (metadata + rendered config), not an always-on filesystem watcher.
- On non-systemd process-manager paths, exited-unit status visibility may be short-lived.

## Development

```bash
cargo test              # run tests
cargo build             # debug build
cargo build --release   # release build
./scripts/install-cli.sh  # install to ~/.local/bin
```

For foreground daemon debugging, run:

```bash
devstack daemon
```

(when developing the binary itself, `cargo run -- daemon` is equivalent).

Useful local checks:

```bash
cargo run -- status
cargo run -- logs --service api --tail 50
cargo run -- ui
```

## Platform Notes

**Linux**
- Requires a working systemd user session (`systemctl --user`).
- `devstack install` registers `devstackd` as a user service and enables login startup.
- Service lifecycle uses transient systemd units with control-group kill semantics.

**macOS**
- `devstack install` configures a LaunchAgent for the daemon.
- LaunchAgent environments often have a minimal `PATH`; prefer absolute command paths (or explicitly set PATH in env files).
- Runtime/state paths are under `~/Library/Application Support/devstack` instead of `~/.local/share/devstack`.



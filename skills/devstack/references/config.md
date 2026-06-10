# Devstack config format and settings
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
- `capture_api`: `true` for stack HTTP services with a port; set `false` to opt out. Requests to the published service URL are proxied and logged with parsed JSON request/response bodies.
- `capture_api_body_limit`: `256kb`; max request/response body bytes captured per API log entry, as bytes or strings like `"512kb"`/`"2mb"`
- `capture_api_ignore`: service HTTP readiness path plus common health endpoints by default; add request paths to proxy without capture, with `*` suffix for prefix matches such as `"/assets/*"`
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
- `{{ services.<name>.port }}` and `{{ services.<name>.url }}` are the public service port/URL
- `{{ services.<name>.listen_port }}` is the backend port the service process binds when API capture is active

### Ignore and watch patterns

Ignore sources (applied in order): `.gitignore`, `.ignore`, `.devstackignore`, plus per-service `ignore` (gitignore syntax, `!` supported).
If `watch` is set on a service, only matching paths are considered for change detection.
Set `auto_restart = true` to enable live file watching + automatic service restart; this requires non-empty `watch` patterns.

## 1) Core design

* **Linux only (Ubuntu)**, **local only**, **privileged setup acceptable**
* **No TLS** (HTTP only)
* **No reverse-proxy requirement**; **dynamic ports are fine** as long as every service learns them reliably
* **Must eliminate “pick your own port” thinking inside services**
* **Strong orphan cleanup** (your biggest pain) using **cgroups** via **systemd units**
* **Global “attach” dependencies** supported (single instance shared across runs)
* **Readiness via TCP/HTTP/log-regex/custom command**, no “healthy vs ready” distinction (but we still keep a single “ready” concept)


### What it is

A tool named **`devstack`** with:

1. **A long-lived daemon** `devstackd` (same binary, `devstack daemon`) that owns all state and process lifecycle.
2. A **CLI** that talks to the daemon over a **Unix domain socket** using **HTTP+JSON** (so it’s scriptable and fast).
3. Each managed service runs as a **systemd transient service unit** (user-level systemd), so:

   * Every service gets its own **cgroup**
   * `devstack down` reliably kills **the entire process tree** (no orphans), because systemd stops the unit’s cgroup
4. Services get **ports and dependency URLs injected via env vars** (`PORT`, `DEV_URL_*`, `DEV_PORT_*`, `DEV_RUN_ID`, etc.).

This approach is intentionally “boring Linux”: systemd+cgroups for lifecycle correctness; your code handles port allocation + env injection + readiness + UX.

### Why

* **Orphans**: systemd unit stop kills the cgroup. This is exactly what you want.
* **No port hunting**: daemon allocates and injects ports; everything else uses env vars.
* **Parallel runs**: each run gets its own run-scoped units + its own port map.
* **Global deps**: you can run a shared dependency once, and all runs point at it.

---

## 2) Tech stack choices

### Language

* **Rust**

### Key crates / components

* CLI parsing: **clap**
* JSON/YAML: **serde**, **serde_json**, **serde_yaml**
* Daemon async runtime: **tokio**
* Daemon HTTP server: **axum** (and `hyper` underneath), including Unix socket serving patterns ([GitHub][1])
* CLI HTTP client over Unix socket: **hyper + hyperlocal** ([GitHub][2])
* systemd DBus control: **systemd-zbus** (DBus bindings) ([Docs.rs][3])
* systemd notification (daemon readiness): **sd-notify** ([Docs.rs][4])
* Readiness regex: **regex**
* Template rendering in config/env: **minijinja** ([Crates][5])
* File watching for `logs --follow`: **notify** (inotify backend on Linux)

### Why systemd transient units specifically

To start transient units correctly you must set `ExecStart` as **`a(sasb)`**, not a plain string ([Gist][6]). `systemd-zbus` gives you the method surface we need to do that cleanly from Rust. ([Docs.rs][3])

---

## 3) User-facing CLI contract

### Commands

We’ll implement exactly these (no ambiguity):

* `devstack install`

  * Installs and enables the systemd **user** service for the daemon.
* `devstack up --stack <name> [--project <path>] [--run-id <id>] [--no-wait]`
* `devstack status --run-id <id>`
* `devstack ls`
* `devstack logs --run-id <id> --service <svc> [--tail N] [--follow]`
* `devstack down --run-id <id> [--purge]`
* `devstack kill --run-id <id>`
* `devstack exec --run-id <id> -- <command...>`
* `devstack doctor`
* `devstack gc [--older-than 7d] [--all]`

### Output format rules

* Default output is **JSON**.
* `--pretty` pretty prints JSON (indent).
* `devstack logs` prints raw log lines to stdout (not JSON), unless `--json` is explicitly provided.

### `up` output manifest (machine-readable)

Example (HTTP only, dynamic ports):

```json
{
  "run_id": "e2e-7f3a12bc",
  "project_dir": "/home/me/myapp",
  "stack": "e2e",
  "manifest_path": "/home/me/.local/share/devstack/runs/e2e-7f3a12bc/manifest.json",
  "services": {
    "web":   { "port": 43121, "url": "http://localhost:43121", "state": "ready" },
    "api":   { "port": 43122, "url": "http://localhost:43122", "state": "ready" },
    "ws":    { "port": 43123, "url": "ws://localhost:43123",   "state": "ready" },
    "mock":  { "port": 43124, "url": "http://localhost:43124", "state": "ready" }
  },
  "env": {
    "DEV_RUN_ID": "e2e-7f3a12bc",
    "DEV_URL_WEB": "http://localhost:43121",
    "DEV_URL_API": "http://localhost:43122",
    "DEV_URL_WS":  "ws://localhost:43123",
    "DEV_URL_MOCK":"http://localhost:43124",
    "DEV_PORT_WEB":"43121",
    "DEV_PORT_API":"43122",
    "DEV_PORT_WS":"43123",
    "DEV_PORT_MOCK":"43124"
  }
}
```

The `env` object is important: it’s a canonical map that external consumers (Playwright, scripts) can ingest directly.

---

## 4) Config file (project-local) and conventions

### Config discovery

* Default config filename in project root: **`devstack.yml`**
* Override with `--file <path>`

### Schema (v1)

A single YAML file can define multiple stacks.

```yaml
version: 1

stacks:
  e2e:
    services:
      web:
        cmd: "pnpm dev"
        deps: ["api", "ws"]
        scheme: "http"        # default http
        port_env: "PORT"      # default PORT
        readiness:
          tcp: {}             # default if omitted
        env:
          VITE_API_URL: "{{ services.api.url }}"
          VITE_WS_URL: "{{ services.ws.url }}"

      api:
        cmd: "pnpm api"
        deps: ["mock"]
        readiness:
          http:
            path: "/health"
            expect_status: [200, 399]
        env:
          LLM_BASE_URL: "{{ services.mock.url }}"

      ws:
        cmd: "pnpm ws"
        scheme: "ws"
        readiness:
          tcp: {}

      mock:
        cmd: "python -m mockserver"
        readiness:
          log_regex: "listening on"

  voice:
    services:
      worker:
        cmd: "pnpm worker"
        port: none            # no PORT injected

      voice:
        cmd: "pnpm voice"
        deps: ["mock"]
        readiness:
          http: { path: "/ready", expect_status: [200, 399] }

      mock:
        cmd: "uvicorn mock:app --reload"
        readiness: { tcp: {} }

globals:
  # global services that may be shared across runs
  db:
    cmd: "docker compose up"   # leave running
    scheme: "http"
    readiness: { tcp: {} }
```

### Defaults (to minimize config)

If a field is missing:

* `project_dir`: cwd
* `cwd` for each service: project root
* `scheme`: `http`
* `port_env`: `PORT`
* `port`: auto-allocated unless `port: none`
* `readiness`: default **TCP connect to localhost:PORT** within timeout
* termination: SIGINT, 2s grace, then SIGKILL (configurable)

### Template context (minijinja)

We support templating only in **env values** (not keys) and a few string fields.

Available variables:

* `run.id`
* `project.dir`
* `stack.name`
* `services.<name>.port`
* `services.<name>.url`

This is how you bridge existing conventions without changing app code (e.g. set `VITE_API_URL`).

---

## 5) Runtime directory layout (global and per-run)

Base dir: `~/.local/share/devstack`

```
~/.local/share/devstack/
  daemon/
    devstackd.sock
    state.json
  runs/
    <run_id>/
      manifest.json
      devstack.yml.snapshot
      logs/
        <service>.log
      state/
        <service>/...   (optional per-service)
  globals/
    <global_name>/
      manifest.json
      logs/<service>.log
```

### Log retention

* Logs are **never deleted automatically**.
* `devstack down` keeps logs/state by default.
* `devstack down --purge` deletes the run directory.
* `devstack gc` deletes stopped runs older than N days (default 7 days unless `--older-than`).

---

## 6) Daemon responsibilities and API

### Daemon responsibilities

The daemon is the sole authority for:

* run_id generation
* port allocation
* starting/stopping services
* readiness waiting
* health monitoring (optional, see below)
* tracking status + failure reasons
* producing/storing manifest.json

### Daemon transport: Unix socket HTTP+JSON

* Socket path: `~/.local/share/devstack/daemon/devstackd.sock`
* HTTP server on UDS using axum patterns ([GitHub][1])
* Client uses hyperlocal to connect ([GitHub][2])

### API endpoints

* `POST /v1/runs/up`
* `POST /v1/runs/down`
* `POST /v1/runs/kill`
* `GET  /v1/runs/{run_id}/status`
* `GET  /v1/runs`
* `GET  /v1/globals` (optional but useful)
* `POST /v1/runs/{run_id}/restart-service` (internal; exposed for UX)

The CLI is thin: it forwards command intent and prints JSON.

---

## 7) How we start services: systemd transient units + shim

### The critical pattern

Each service is run as a **systemd transient service unit** (user manager), with:

* `ExecStart` = run our internal shim:

  * `devstack __shim --run-id <id> --service <svc> --cmd "<shell>" --cwd "<path>" --log-file "<...>"`
* The shim then launches your actual command via `/bin/bash -lc "<cmd>"`.

Why:

* systemd provides cgroup and kill semantics.
* shim provides consistent logging, timestamps, signal forwarding, and better error messages.

### systemd DBus calls we will use

Using `systemd-zbus` ManagerProxy methods (not shelling out):

* `StartTransientUnit`
* `StopUnit`
* `RestartUnit`
* `KillUnit` ([Docs.rs][3])

### ExecStart encoding detail (important)

When calling `StartTransientUnit`, set `ExecStart` as an array of `(path, argv, ignore_failure)` = `a(sasb)` ([Gist][6])

### Unit properties we will set (v1)

For each run-scoped service unit:

* `Description`: `"devstack <run_id> <service>"`
* `Type`: `"exec"` (better start failure visibility) ([Arch Manual Pages][7])
* `WorkingDirectory`: project dir (or service override)
* `Environment`: the injected env list (see section 9)
* `ExecStart`: shim invocation (a(sasb))
* `KillMode`: `"control-group"` (kill the whole cgroup)
* `KillSignal`: `2` (SIGINT) (KillSignal is i32) ([Docs.rs][8])
* `TimeoutStopUSec`: `2_000_000` by default (TimeoutStopUSec is u64) ([Docs.rs][8])
* `SendSIGKILL`: `true` (bool) ([Docs.rs][8])
* `Restart`: `"on-failure"`
* `RestartUSec`: `250_000` (0.25s) (RestartUSec is u64) ([Docs.rs][8])
* `StartLimitIntervalUSec`: `30_000_000` (30s)
* `StartLimitBurst`: `20`

This directly implements your default: restart on crash/nonzero, with backoff + max attempts, then degraded.

---

## 8) Port allocation (run-scoped + global)

### Policy

* All listening services get `PORT` injected (or `port_env` override).
* Ports are **allocated by the daemon** at stack-up time.
* Ports are **stable for the lifetime of the run**, and stored in the manifest.

### Allocation algorithm (simple, reliable enough for local dev)

For each service needing a port:

1. Loop:

   * Bind `127.0.0.1:0` to let the OS choose a free port.
   * Read `local_addr.port()`
   * Close the socket immediately
   * Record the port
2. Start the service quickly after allocation.
3. If the service fails to start due to `EADDRINUSE` (rare race), the daemon treats it as “failed before ready”, allocates a new port, updates the manifest-in-progress, and retries **only during initial bring-up**.

Once a service is **ready**, its port never changes (even on restarts). If the port later becomes unusable, we surface degraded state; we do not silently rotate ports because that breaks dependents.

### Globals (“attach”)

Global services have:

* One port map stored under `~/.local/share/devstack/globals/<name>/manifest.json`
* Restart uses the same port
* Runs reference global ports in their env injection

---

## 9) Environment injection contract (the main value)

### Always injected into every run-scoped service

* `DEV_RUN_ID=<run_id>`
* `DEV_STACK=<stack_name>`
* `DEV_PROJECT_DIR=<project_dir>`

For each service `X`:

* `DEV_PORT_X=<port>` (if it has one)
* `DEV_URL_X=<scheme>://localhost:<port>` (if it has one)

Also inject each service’s own port env:

* `PORT=<its_port>` (or `port_env`)

### Dependency env for convenience

Even though you’re ok with “just give everyone all ports”, we also inject:

* `DEV_DEP_<DEP>_URL=...`
* `DEV_DEP_<DEP>_PORT=...`
  for only the declared deps, so service logs/config stay tidy.

### User-defined env mappings (templated)

Service config `env:` values are rendered with minijinja (so you can set `VITE_API_URL`, `DATABASE_URL`, etc. without code changes). ([Crates][5])

This is the “very little per app/service setup” lever: you map whatever env names the app already expects.

---

## 10) Readiness + “up waits” behavior

### Single “ready” concept

No separate “healthy vs ready”. A service is “ready” when its readiness check passes.

### Supported readiness checks

Configured per service:

1. **tcp** (default): connect to `localhost:PORT`
2. **http**: GET `http://localhost:PORT/<path>` expecting status in range
3. **log_regex**: ready when log file matches regex
4. **cmd**: run a command; exit code 0 means ready

### `devstack up` wait semantics

* Default: waits until all services are ready (or timeout).
* `--no-wait`: returns immediately after spawning.

### Implementation detail

During `up`, the daemon:

* topologically sorts services by `deps`
* starts deps first
* waits for dep readiness
* then starts dependent service

Timeout defaults:

* per service readiness timeout: **30s**
* poll interval: **200ms** (TCP/HTTP)
* log regex: file watcher with fallback polling

---

## 11) Restart policy + degraded state (crash loops you can see)

### Automatic restarts on crash

Handled by systemd:

* `Restart=on-failure`
* `RestartUSec=250ms`
* `StartLimitBurst=20` per `StartLimitIntervalUSec=30s`

When start limit is hit, the daemon marks service **degraded** and includes systemd’s failure result in status output.

### Restart on failed readiness/health checks

We implement a simple monitor loop in the daemon:

* Once a service is “ready”, continue checking **the same readiness check** as a periodic health probe.
* Default health interval: 5s
* Failure threshold: 3 consecutive failures
* On threshold breach: daemon calls `RestartUnit(service_unit)`

To prevent thrash:

* Keep a per-service ring buffer of health-triggered restarts.
* If more than 5 health restarts in 60s: mark degraded and stop health restarts until user intervenes.

This matches your “degraded and visible via CLI” requirement.

---

## 12) Shutdown semantics (down vs kill) + orphan guarantee

### `devstack down --run-id X`

For each service unit in the run:

* systemd stop unit → sends SIGINT (KillSignal=2)
* waits `TimeoutStopUSec` (default 2s)
* then SIGKILL the cgroup if needed (SendSIGKILL=true)

Because it’s a cgroup kill, forked/disowned children die too.

### `devstack kill --run-id X`

Immediate:

* systemd `KillUnit(..., who="all", signal=9)` then stop
* no grace

This maps to exactly what you asked for.

---

## 13) Logging (capture everything, keep forever, purge later)

### Log capture method (shim)

The shim:

* spawns the service command
* captures stdout+stderr
* writes to a single log file per service:
* services will control their own log format, some might do json, others will do plain text

### `devstack logs`

* `--tail N` reads last N lines (implement using reverse scan or a small “line index” file updated by shim).
* `--follow` uses inotify (notify crate) to stream appended lines.

### Purging

* `devstack down --purge` removes the run directory.
* `devstack gc` removes old run dirs.

---

## 14) Global dependencies (“attach”)

### Config: `globals:`

A global service is started once and kept running until explicitly stopped (we’ll add `devstack globals down <name>` later, but v1 can simply never stop them automatically).

### Name scoping

Global services are keyed by:

* project directory hash + global name
  So two different projects can both have a `db` global without collision.

### How runs reference globals

When a run starts:

* daemon ensures globals are running first
* globals are injected into the run env exactly like normal services:

  * `DEV_URL_DB=...`
  * `DEV_PORT_DB=...`

---

## 15) `doctor` and `gc`

### `devstack doctor`

Checks:

* daemon socket reachable
* systemd user instance reachable over DBus
* can create transient unit and stop it (tiny noop test)
* filesystem permissions for `~/.local/share/devstack`
* reports results as JSON with remediation text

### `devstack gc`

* deletes run directories that are:

  * stopped
  * older than `--older-than` (default 7d)
* does not touch globals unless `--all`

---

## 16) Implementation plan (work breakdown in the right order)

### Phase 0 — Project skeleton

1. Cargo workspace with one binary `devstack`.
2. Subcommands:

   * user-facing: `install|up|status|ls|logs|down|kill|exec|doctor|gc`
   * internal: `daemon`, `__shim`

### Phase 1 — State + filesystem layout

1. Implement path resolver:

   * base dir: `~/.local/share/devstack`
2. Implement types:

   * `RunId`, `StackName`, `ServiceName`
   * `ServiceSpec` (from config)
   * `RunManifest` (persisted JSON)
3. Implement atomic writes:

   * write `manifest.json.tmp`, fsync, rename

### Phase 2 — Config parsing + templating

1. YAML schema parse into Rust structs via serde_yaml
2. Validate:

   * duplicate service names
   * missing deps
   * cycles (topological sort)
3. Template rendering via minijinja for env values

### Phase 3 — systemd DBus integration

1. Connect to session bus
2. Instantiate `ManagerProxy` from systemd-zbus ([Docs.rs][3])
3. Implement helpers:

   * `start_transient_service(unit_name, properties)`
   * `stop_unit(unit_name)`
   * `restart_unit(unit_name)`
   * `kill_unit(unit_name, signal)`
4. Encode ExecStart correctly as `a(sasb)` ([Gist][6])

### Phase 4 — Shim

1. `devstack __shim ...` args:

   * `--run-id`, `--service`, `--cmd`, `--cwd`, `--log-file`
2. Behavior:

   * open log file append
   * spawn `/bin/bash -lc <cmd>` with:

     * working dir set
     * env inherited (systemd provides injected env)
   * pipe stdout/stderr, line-buffer, prefix timestamps, write
   * handle SIGINT/SIGTERM:

     * forward to child
     * wait `grace_ms` (passed via env `DEV_GRACE_MS`)
     * then kill child process group (extra safety)
3. Exit code:

   * shim exits with child’s exit code so systemd sees failure/nonzero

### Phase 5 — Daemon HTTP API over Unix socket

1. Implement daemon as axum server bound to UDS ([GitHub][1])
2. Endpoints for up/down/kill/status/ls
3. Daemon main loop maintains:

   * in-memory map `runs: HashMap<RunId, RunState>`
   * persisted manifests on changes

### Phase 6 — `up` orchestration

1. Determine project dir and load config
2. If no run-id: generate `stack-<8hex>`
3. Allocate ports for all run services needing ports
4. Compute URL map
5. Render env templates
6. Start globals (ensure running, wait ready)
7. Start run services in topo order:

   * create transient unit with ExecStart pointing to shim
   * wait readiness
8. Persist final manifest and return JSON

### Phase 7 — readiness + status

1. Readiness probes:

   * tcp connect
   * http get
   * log regex
   * cmd
2. Status returns:

   * for each service:

     * desired state (running/stopped)
     * systemd state (active/substate) (queried via DBus unit props)
     * ready boolean
     * last failure reason (timeout, start-limit-hit, exit code, etc.)

### Phase 8 — down/kill

1. down:

   * stop all run units
   * mark run stopped
   * keep files unless purge
2. kill:

   * kill all run units SIGKILL
   * stop units
   * mark stopped

### Phase 9 — logs + exec

1. logs:

   * locate run dir from manifest
   * tail/follow log file directly
2. exec:

   * fetch manifest
   * set env vars from `manifest.env`
   * run command with inherited TTY

### Phase 10 — monitor health and restart-on-health-fail

1. For each ready service with a readiness check that can be probed:

   * spawn tokio task
   * on failure threshold, call restart_unit
   * implement limiter; mark degraded

---

## 17) 

 ensure **deterministic env var naming** is ironclad and universal:

* `DEV_RUN_ID`
* `DEV_URL_<SERVICE>`
* `DEV_PORT_<SERVICE>`

…and guarantee that:

* every service gets the full map
* every external consumer can read the same map from `manifest.json`

This becomes the single integration surface for Playwright, agents, scripts, etc.

---


[1]: https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs?utm_source=chatgpt.com "axum/examples/unix-domain-socket/src/main.rs at main"
[2]: https://github.com/softprops/hyperlocal?utm_source=chatgpt.com "softprops/hyperlocal: 🔌 ✨rustlang hyper bindings for local ..."
[3]: https://docs.rs/systemd-zbus/latest/systemd_zbus/struct.ManagerProxy.html "ManagerProxy in systemd_zbus - Rust"
[4]: https://docs.rs/sd-notify?utm_source=chatgpt.com "sd_notify - Rust"
[5]: https://crates.io/crates/minijinja?utm_source=chatgpt.com "minijinja - crates.io: Rust Package Registry"
[6]: https://gist.github.com/daharon/c088b3ede0d72fd20ac400b3060cca2d?utm_source=chatgpt.com "Calling systemd's StartTransientUnit via DBus"
[7]: https://man.archlinux.org/man/systemd-run.1.en?utm_source=chatgpt.com "systemd-run(1) - Arch manual pages"
[8]: https://docs.rs/systemd-zbus/latest/systemd_zbus/struct.ServiceProxy.html "ServiceProxy in systemd_zbus - Rust"

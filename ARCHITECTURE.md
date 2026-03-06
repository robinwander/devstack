# Devstack Architecture

## 1) System Overview

Devstack is split into three cooperating processes:

1. **CLI (`src/cli.rs`)**
   - Parses commands (`devstack up`, `status`, `logs`, etc.) and resolves project/config context.
   - Sends JSON HTTP requests over a **Unix domain socket** to the daemon (`call_daemon_inner`, `src/cli.rs:723-767`).

2. **Daemon (`src/daemon.rs`)**
   - Runs an Axum HTTP server on `devstackd.sock` (`run_daemon`, `src/daemon.rs:197-273`).
   - Owns run state, service orchestration, readiness, health, GC, and log index access.

3. **Shim (`src/shim.rs`)**
   - Spawned as each service’s `ExecStart` target (via `devstack __shim ...`, `src/daemon.rs:1919-1930`).
   - Launches real service commands (`/bin/bash -lc ...`) and captures stdout/stderr into JSONL.

### `devstack up` request flow

1. `devstack up` resolves stack/project/config in CLI (`src/cli.rs:307-347`, `1527-1597`).
2. CLI POSTs `/v1/runs/up` over Unix socket (`src/cli.rs:723-767`).
3. Daemon handler `up()` calls `orchestrate_up()` (`src/daemon.rs:406`, `1087+`).
4. Daemon loads config + stack plan, allocates ports, prepares env/templates, creates runtime records.
5. For each service in topo order:
   - Optional init tasks run (`run_init_tasks_blocking`, `src/daemon.rs:1062-1078`).
   - Transient unit started with `ExecStart=devstack __shim ...` (`start_prepared_service`, `src/daemon.rs:1888-1955`).
   - Readiness checks run (`handle_readiness`, `src/daemon.rs:2022-2074`).
6. Daemon persists manifest and returns `RunManifest` JSON.

---

## 2) Daemon Internals

### State model

- Global app container: `AppState` (`src/daemon.rs:52-58`)
  - `systemd`: platform service manager abstraction
  - `state: Arc<Mutex<DaemonState>>`: in-memory run/service state
  - `log_index`: shared Tantivy index handle
- In-memory state:
  - `DaemonState { runs: BTreeMap<String, RunState> }` (`src/daemon.rs:63-65`)
  - `RunState` + per-service `ServiceRuntime` (`src/daemon.rs:73-100`)

### Service lifecycle states

`ServiceState`: `Starting -> Ready -> Degraded/Failed -> Stopped` (`src/manifest.rs:10-17`)

Transitions are driven by:
- startup + readiness (`mark_service_ready`, `mark_service_failed`, `src/daemon.rs:2091-2135`)
- periodic health monitor (`start_health_monitor`, `src/daemon.rs:2168-2326`)
- explicit down/kill (`orchestrate_down`, `orchestrate_kill`, `src/daemon.rs:2423-2503`)

`RunLifecycle` is recomputed from service states (`recompute_run_state`, `src/daemon.rs:2138-2166`).

### Health monitoring loop

After service reaches `Ready`, daemon creates a per-service `HealthHandle` with counters and starts a 5s monitor loop (`src/daemon.rs:2099-2121`, `2168+`):

- Calls `check_ready_once()` on original readiness spec.
- Tracks `passes/failures/consecutive_failures/last_ok`.
- After 3 consecutive failures:
  - Marks service `Degraded`
  - Attempts up to 3 restarts with backoff (0s, 5s, 30s)
  - Marks `Failed` if restart limit exceeded.

### File watch + incremental restart model

Devstack does **hash-based change detection**, not a long-lived fs watcher for services:

- During service prep, daemon computes a `watch_hash` from:
  - rendered command/cwd/env/readiness
  - watch/ignore patterns
  - selected files via `watch::compute_watch_hash()`
  (`prepare_service`, `src/daemon.rs:1845-1871`, `watch.rs`).
- On `up` against an existing run, `orchestrate_refresh_run()` compares old/new `watch_hash` and restarts only changed/failed services (`src/daemon.rs:1440-1483`).

### Log ingestion in daemon

- Service log API (`/v1/runs/{id}/logs/{service}`) calls `read_service_logs()` (`src/daemon.rs:2644-2686`).
- `read_service_logs()` delegates to `LogIndex::search_service()`, which ingests new file bytes before querying (`src/log_index.rs:380-527`).
- Run-wide search/facets similarly ingest on demand (`search_run`, `facets_run`, `src/log_index.rs:529+`, `620+`).

---

## 3) Process Model

### Linux: transient systemd units over DBus

`RealSystemd` (`src/systemd.rs:75-187`) uses `systemd-zbus` to call `StartTransientUnit` and related methods.

Each service unit uses:
- `Type=exec`
- `KillMode=control-group`
- `KillSignal=SIGINT`
- restart/start-limit controls (`UnitProperties`, `src/systemd.rs:14-45`).

This gives process-tree lifecycle control via cgroups.

### macOS / non-Linux: LocalSystemd

`LocalSystemd` (`src/systemd.rs:205-427`) emulates minimal service-manager behavior:
- spawns child process groups (`setpgid`)
- stop/kill signals process group
- timeout-based reap
- in-memory unit map + reaper loop

### Shim model

`shim::run()` (`src/shim.rs:23-93`) does:
- spawn `/bin/bash -lc <cmd>` in new process group
- pump stdout/stderr lines
- strip ANSI
- write one JSON object per line to log file (`pump_lines`, `src/shim.rs:197-217`)
- on signal, terminate process group (SIGTERM then SIGKILL fallback)

---

## 4) Log Pipeline

### End-to-end flow

1. Service emits stdout/stderr.
2. Shim captures each line and writes JSONL with `time`, `stream`, and either merged app JSON fields or wrapped `msg` (`encode_log_line`, `src/shim.rs:173-195`).
3. Files are stored under run/global log paths (`paths.rs`).
4. `LogIndex` ingests incrementally using per-source cursor `{offset,next_seq}` (`src/log_index.rs:30-41`, `206-379`).
5. Tantivy index stores fields: `run_id/service/stream/level/ts_nanos/ts/seq/message/raw` (`src/log_index.rs:45-55`, `128-139`).
6. Query endpoints:
   - `/v1/runs/{id}/logs/{service}`
   - `/v1/runs/{id}/logs`
   - `/v1/runs/{id}/logs/facets`
   - source equivalents (`/v1/sources/...`)
7. CLI renders raw or structured output (`src/cli.rs` + `src/logs.rs`).

### External sources

- Sources are registered in `sources.json` via `SourcesLedger` (`src/sources.rs`).
- Source files/globs resolve to `LogSource` entries; each source maps to synthetic `run_id = source:<name>`.
- Daemon also runs periodic source ingestion every 5s (`spawn_periodic_source_ingest`, `src/daemon.rs:2933-2951`).

---

## 5) Config Resolution

### Discovery and parsing

- Nearest config walk: `ConfigFile::find_nearest_path()` (`src/config.rs:245-263`).
- Default file preference in a dir: `devstack.toml`, then `devstack.yml`, then `devstack.yaml` (`src/config.rs:230-243`).
- Parsing supports TOML/YAML and extensionless fallback (`src/config.rs:180-199`).

### Validation

`ConfigFile::validate()` enforces (`src/config.rs:266-298`):
- version compatibility
- valid service/task names
- valid `port` values
- readiness spec validity
- init-task references exist

Dependency order uses topo sort with cycle/missing-dep checks (`topo_sort`, `src/config.rs:341-390`).

### Templating

Daemon uses `minijinja` for service/global fields (`render_template`, `src/daemon.rs:1683-1688`) with context from `build_template_context()` (`src/daemon.rs:1645-1672`):
- `{{ run.id }}`
- `{{ project.dir }}`
- `{{ stack.name }}`
- `{{ services.<name>.port }}` / `.url`

Templating is applied to command, env values, cwd/env_file paths, and watch/ignore patterns.

---

## 6) Port Allocation

### Assignment

`allocate_ports()` (`src/port.rs:8-32`):
- `port = "none"` => no port
- fixed port => checked with bind probe (`ensure_available`)
- unset port => ephemeral bind on `127.0.0.1:0`

Refresh logic (`resolve_ports_for_refresh`, `src/daemon.rs:1556-1600`) reuses existing dynamic ports when possible for active runs.

### Injection and dependency wiring

Daemon builds base env (`build_base_env`, `src/daemon.rs:1602-1627`):
- `DEV_RUN_ID`, `DEV_STACK`, `DEV_PROJECT_DIR`
- `DEV_PORT_<SERVICE>`, `DEV_URL_<SERVICE>`

And per-dependency env (`inject_dep_env`, `src/daemon.rs:1629-1643`):
- `DEV_DEP_<DEP>_PORT`, `DEV_DEP_<DEP>_URL`

These values are also available through template context (`services.*.port/url`).

---

## 7) Key Design Decisions

1. **Transient units for service lifecycle**
   - Linux uses systemd transient units via DBus (`src/systemd.rs`) with control-group kill semantics.
   - This is aligned with the repository’s design docs emphasizing orphan cleanup and process-tree correctness (`implementation_plan.md`, README).

2. **Shim + JSONL logs**
   - Shim centralizes capture format and process-group signal handling (`src/shim.rs`).
   - JSONL allows both plain-text wrapping and structured app JSON passthrough.

3. **Tantivy index for logs**
   - Enables full-text query parsing, counts, tailing, and facets across services/runs/sources (`src/log_index.rs`).

4. **Unix socket HTTP API**
   - Local-only daemon transport avoids network exposure while keeping an easy HTTP+JSON contract (`src/cli.rs`, `src/daemon.rs`, `API.md`).

---

## Code Health Notes

1. **`resolve_env_vars` drops missing `${VAR}` placeholders instead of preserving them**
   - In `resolve_env_vars`, `${VAR}` missing case does not write original token back (comment says it should).
   - **Ref:** `src/config.rs:508-522`

2. **Source-log service identity is lost when querying through daemon API**
   - Daemon `/v1/sources/{name}/logs` flattens `LogEntry` to raw lines only.
   - CLI reconstructs entries with `structured_log_from_raw(source_name, line)`, forcing service name to source name.
   - Multi-file source service labels (`api`, `worker`, etc.) are lost.
   - **Refs:** `src/daemon.rs:944-946`, `src/cli.rs:979-989`

3. **Periodic source ingestion swallows all errors silently**
   - Background task ignores both join and ingestion errors (`let _ = ... .await`).
   - Failures in source loading/indexing have no logs/metrics and can silently stall freshness.
   - **Ref:** `src/daemon.rs:2940-2948`

4. **Globals env processing is inconsistent with normal services**
   - Normal services run `resolve_env_map()` after merge/render; globals do not.
   - `$VAR` / `${VAR}` interpolation behavior differs between stack services and globals.
   - **Refs:** service path `src/daemon.rs:1836-1838`, globals path `src/daemon.rs:3149-3156`

5. **LocalSystemd removes exited units from map, so status quickly becomes `None`**
   - Reaper removes exited units every second; `unit_status` also removes unit on exit and does not reinsert inactive metadata.
   - This can make exited state transient and harder to observe via status/readiness polling on non-Linux.
   - **Refs:** `src/systemd.rs:226-247`, `src/systemd.rs:396-415`

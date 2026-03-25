# Devstack End-to-End Behavior Harness

## Goal

Before doing the full refactor, define a **black-box test harness** at the outer edge of devstack so we can preserve behavior while changing internals freely.

The tests should exercise the system the same way a user does:

- through the **CLI**
- through the **daemon API**
- through the **filesystem side effects** devstack intentionally owns

The tests should **not** know about internal modules, stores, orchestration functions, or struct layouts.

If we do this well, the refactor can completely rewrite internals while the tests remain unchanged.

---

## Testing philosophy

### What we are trying to preserve

We want to preserve the **observable contract** of devstack:

- what commands exist
- what daemon endpoints exist
- what state transitions users can observe
- what files/logs/manifests appear on disk
- what events are emitted
- what lifecycle behaviors occur around start, refresh, restart, readiness, logs, watch, tasks, globals, and GC

### What we are explicitly *not* trying to preserve

We do **not** want tests that freeze incidental implementation details like:

- internal module boundaries
- exact function names
- lock structure
- specific internal state structs
- exact ordering of unrelated JSON fields
- exact terminal coloring/formatting except where that is a deliberate UX contract
- exact timestamp values
- exact random run id suffixes

### Contract-over-implementation rule

A good e2e test should be phrased like:

- “when I run `devstack up`, the service becomes ready and `status` reports it”
- “when a watched file changes, the service restarts”
- “when I query logs with a filter, I get matching entries”

not like:

- “`orchestrate_up` inserted `RunState.services[api].watch_hash` before readiness”

---

## Critical design constraint for the harness

The harness must be **hermetic**.

That means each test needs its own isolated:

- `HOME`
- `XDG_DATA_HOME`
- `XDG_CONFIG_HOME`
- `XDG_RUNTIME_DIR`
- project directory
- daemon socket
- logs index
- manifests
- ledgers

Current `src/paths.rs` already derives state from OS directories / `HOME`, so this isolation is feasible by environment setup.

### Necessary testability hooks

Two current behaviors will make true black-box e2e tests flaky unless we explicitly tame them:

#### 1. Process supervisor selection

On Linux, `run_daemon()` always uses `RealSystemd::connect()`.

That means tests depend on a live user systemd session DBus, which is not a good e2e harness foundation.

For the harness, devstack should support a **test-only runtime override**, e.g.:

```text
DEVSTACK_PROCESS_MANAGER=local
```

or

```text
DEVSTACK_TEST_MODE=1
```

which forces the daemon to use the local process manager even on Linux.

This is not a fake harness hidden inside tests. It is a runtime selection at the outer edge, and the rest of the stack still runs end-to-end.

#### 2. Dashboard spawn suppression

`run_daemon()` currently tries to spawn the dashboard process if installed.

That is noise in e2e tests and can create unrelated failures.

The harness should use an env flag like:

```text
DEVSTACK_DISABLE_DASHBOARD=1
```

so the daemon only exercises the core product surface under test.

Without these two controls, the harness will be much more environment-dependent than it needs to be.

---

## Behavior inventory: what devstack does today

This is the functionality the e2e suite should cover.

## 1. Daemon lifecycle

Observable surfaces:

- `devstack daemon`
- `GET /v1/ping`
- daemon socket presence
- daemon state persistence
- ability to reconnect after daemon restart

Behavior to preserve:

- daemon starts and binds its Unix socket
- second daemon instance does not silently steal the socket
- ping succeeds when daemon is healthy
- daemon reloads persisted state on restart

## 2. Stack lifecycle

Observable surfaces:

- `devstack up`
- `devstack status`
- `devstack down`
- `devstack kill`
- `GET /v1/runs/*`
- run manifests on disk

Behavior to preserve:

- `up` starts the selected stack
- default behavior refreshes an existing run instead of always creating a new one
- `--new-run` creates a new run
- `status` shows service readiness and run lifecycle
- `down` stops services gracefully and marks run stopped
- `kill` force-kills services and marks run stopped
- manifests reflect observable state

## 3. Refresh semantics

Observable surfaces:

- repeated `devstack up`
- `--force`
- status before/after refresh
- persisted run id

Behavior to preserve:

- up without `--new-run` refreshes the latest run for the same project + stack
- unchanged services can keep their port / identity as appropriate
- removed services disappear from the run
- changed services restart when necessary
- forced refresh restarts even when the watch hash did not change

## 4. Service readiness

Observable surfaces:

- `status`
- `up --no-wait`
- service logs
- failure messages

Behavior to preserve:

Supported readiness modes currently visible in config/runtime:

- tcp
- http
- log regex
- cmd
- delay
- exit
- none

For each mode, e2e should preserve:

- successful readiness transitions `starting -> ready`
- failed readiness transitions `starting -> failed/degraded`
- `--no-wait` returns early but background readiness eventually converges
- fast-exit success works for exit readiness

## 5. Init and post-init tasks

Observable surfaces:

- `devstack up`
- `devstack run --init`
- service/task log files
- task status / history
- marker files in test fixtures

Behavior to preserve:

- init tasks run before service start
- init tasks can be skipped when watch hash is unchanged
- post-init tasks run after readiness
- post-init runs again on restart paths
- post-init runs again even when init would be skipped
- failing init prevents service start
- failing post-init marks service failed/degraded appropriately

## 6. Restart service

Observable surfaces:

- `devstack restart-service` via CLI/API equivalent
- `status`
- logs
- marker files from post-init

Behavior to preserve:

- a single service can be restarted without recreating the whole run
- readiness is re-evaluated
- post-init runs again
- `--no-wait` returns early but converges correctly

## 7. Auto-restart watch

Observable surfaces:

- `devstack watch pause/resume`
- `GET /v1/runs/{run_id}/watch`
- service restarts after file changes
- watch-active / paused status in `status`

Behavior to preserve:

- services with `auto_restart: true` and watch patterns start watch monitoring
- watched file changes trigger service restart
- ignored files do not trigger restart
- pause disables automatic restart
- resume re-enables it
- watch status reports active/paused accurately

## 8. Logs and log search

Observable surfaces:

- `devstack logs`
- `GET /v1/runs/{run_id}/logs/{service}`
- `GET /v1/runs/{run_id}/logs`
- `GET /v1/events`
- log files on disk

Behavior to preserve:

- service logs are captured
- stdout/stderr are queryable
- `last`, `since`, `search`, `level`, `stream`, and `after` behave correctly
- combined log view across services works
- facet/filter metadata works
- follow mode returns incremental updates
- SSE log events are emitted for a run
- task logs are also discoverable where expected

## 9. External log sources

Observable surfaces:

- `devstack sources add/rm/list`
- `GET /v1/sources`
- `GET /v1/sources/{name}/logs`

Behavior to preserve:

- sources can be registered and removed
- sources are persisted in the ledger
- source logs can be indexed and queried
- deleting or refreshing sources updates what is searchable

## 10. Tasks as a first-class feature

Observable surfaces:

- `devstack run`
- `devstack run --detach`
- `GET /v1/tasks/run`
- `GET /v1/tasks/{execution_id}`
- `GET /v1/runs/{run_id}/tasks`
- task log files and history files

Behavior to preserve:

- named tasks can be listed and executed
- detached tasks return an execution id
- task status can be polled to completion
- run-scoped task history is visible
- adhoc task history is isolated by project
- verbose mode streams to the terminal instead of only log files

## 11. Globals

Observable surfaces:

- config `[globals]`
- `devstack up`
- `GET /v1/globals`
- global manifest/log files
- GC behavior for globals

Behavior to preserve:

- globals are ensured as part of stack startup
- globals reuse their existing instance when already active
- globals get ports/URLs as expected
- globals run readiness and post-init
- globals are listed independently
- stopped globals can be garbage-collected

## 12. Projects ledger

Observable surfaces:

- `devstack projects list/add/remove`
- `GET /v1/projects`
- project auto-touch during `up`

Behavior to preserve:

- projects are registered in the ledger
- existing runs can seed/touch project entries
- add/remove/list works
- current project filtering vs all-project listing behaves correctly

## 13. Navigation intent

Observable surfaces:

- `GET/POST/DELETE /v1/navigation/intent`
- `devstack show` flow

Behavior to preserve:

- navigation intent can be stored
- storing again replaces previous intent
- clearing removes it

## 14. Agent session integration

Observable surfaces:

- `devstack agent`
- `/v1/agent/sessions*`
- share / poll behavior

Behavior to preserve:

- sessions can be registered and unregistered
- queued messages are returned and then cleared
- latest session lookup by project works
- share routes to the latest matching session
- stale sessions are eventually cleaned up

## 15. Garbage collection

Observable surfaces:

- `devstack gc`
- `/v1/gc`
- run/global directories disappearing
- log index cleanup

Behavior to preserve:

- stopped runs older than threshold are removed
- `--all` also removes stopped globals
- active runs are not removed
- invalid durations fail clearly
- log index entries for removed runs are eventually gone

## 16. Doctor / lint / openapi / install-adjacent UX

These are lower priority than runtime lifecycle, but still part of the product surface.

Observable surfaces:

- `devstack doctor`
- `devstack lint`
- `devstack openapi`
- `devstack completions`

Behavior to preserve:

- lint validates config and exits appropriately
- doctor returns a health report
- openapi generation works
- shell completions generation works

For the refactor, these can be covered with smoke tests rather than deep scenario tests.

---

## What the e2e harness should look like

## Test structure

```text
tests/
├── e2e/
│   ├── mod.rs
│   ├── harness.rs               # TestEnv, Devstack, DaemonHandle, helpers
│   ├── fixtures.rs              # project fixture builders
│   ├── cli.rs                   # CLI wrapper
│   ├── api.rs                   # API client wrapper
│   ├── events.rs                # SSE client / recorder
│   ├── assertions.rs            # await helpers and semantic assertions
│   ├── stack_lifecycle.rs
│   ├── readiness.rs
│   ├── tasks.rs
│   ├── watch.rs
│   ├── logs.rs
│   ├── globals.rs
│   ├── projects_sources.rs
│   ├── agent.rs
│   └── gc.rs
```

Each scenario file should read like product behavior, not infrastructure setup.

---

## Core harness objects

## `TestEnv`

Owns all per-test isolated directories and environment.

```rust
pub struct TestEnv {
    pub root: TempDir,
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_runtime_dir: PathBuf,
    pub workspace: PathBuf,
}
```

Responsibilities:

- create temp dirs
- set env for all spawned child processes
- provide helpers like `project("simple")`
- expose devstack base dir for file assertions

## `Devstack`

A black-box wrapper around the compiled binary.

```rust
pub struct Devstack {
    pub bin: PathBuf,
    pub env: TestEnv,
}
```

Responsibilities:

- spawn daemon process
- run CLI commands
- create API client bound to the daemon socket
- expose high-level helpers:
  - `daemon()`
  - `cli([...])`
  - `api()`
  - `events()`

## `DaemonHandle`

Owns the daemon child process and its captured stderr/stdout.

```rust
pub struct DaemonHandle {
    child: std::process::Child,
    stderr_log: PathBuf,
}
```

Responsibilities:

- wait for `/v1/ping`
- kill on drop if still running
- allow restart tests
- surface daemon stderr when a test fails

## `ApiClient`

A black-box client against the real daemon endpoints.

Use the exact same Unix socket transport semantics the product uses, but the test client itself can be a minimal wrapper.

Responsibilities:

- call endpoints
- deserialize API responses
- expose semantically-named methods:
  - `up(req)`
  - `status(run_id)`
  - `down(run_id)`
  - `kill(run_id)`
  - `watch_pause(run_id, service)`
  - `watch_resume(run_id, service)`
  - `run_task(req)`
  - `task_status(id)`
  - `logs(run_id, service, query)`
  - `logs_view(run_id, query)`
  - `list_runs()`
  - `list_globals()`
  - `gc(req)`

## `EventRecorder`

Connects to `GET /v1/events` and records daemon events for assertions.

Responsibilities:

- subscribe before the action under test
- capture run/service/task/global/log events
- await specific events with timeouts
- filter by run id and service

This is extremely valuable because many behaviors are asynchronous and evented.

---

## DX-first harness rules

If the harness is awkward, people will immediately start bypassing it.

That means the harness must optimize for **test author experience**, not just raw capability.

The bar should be:

- most tests can be written without touching `std::process::Command`
- most tests can be written without manually constructing raw JSON requests
- most tests can be written without creating ad hoc fixture directories
- most tests can be written without hand-rolled polling loops
- when a test fails, the error message already includes the important diagnostics

A good e2e harness does not just make testing *possible*. It makes the obvious path the correct path.

## Harness design principles

### 1. Scenario code should read like product behavior

A good test should look like this:

```rust
#[tokio::test]
async fn restart_service_reruns_post_init() {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::init_post_init()).create().await?;
    let daemon = t.daemon().start().await?;

    let run = t.cli().up(&project).await?.into_run();
    run.assert_service_ready("api").await?;

    t.fs(&project).remove("state/post-init.txt")?;
    t.api().restart_service(run.id(), "api").await?;

    run.assert_service_ready("api").await?;
    t.fs(&project).assert_exists("state/post-init.txt")?;

    daemon.stop().await?;
}
```

The test should not need to know:

- where manifests live
- how to talk to the Unix socket
- how long to poll
- where daemon stderr is captured
- how to locate the run log path

That is harness responsibility.

### 2. Provide typed handles, not just helpers

A pile of free functions is better than nothing, but typed handles make tests much easier to read and harder to misuse.

Recommended handle types:

- `TestHarness`
- `ProjectHandle`
- `DaemonHandle`
- `CliHandle`
- `ApiHandle`
- `RunHandle`
- `TaskHandle`
- `EventRecorder`
- `FsHandle`

These let the API become contextual:

```rust
let run = t.cli().up(&project).await?.into_run();
run.assert_ready().await?;
run.service("api").assert_log_contains("ready").await?;
run.watch().pause("api").await?;
```

That is much better DX than repeatedly passing `run_id` strings everywhere.

### 3. Centralize all waiting behavior

No test should write its own polling loop.

The harness should own:

- retry interval
- timeout defaults
- timeout diagnostics
- eventual consistency semantics

If tests need custom waiting, add a new harness helper instead of letting the test roll its own.

### 4. Centralize fixture creation

No test should manually create its own project tree with raw `std::fs::write` unless it is explicitly testing config parsing edge cases.

Instead, tests should use fixture builders and overlays.

### 5. Fail loudly on harness escape hatches

If a test reaches for raw process spawning or raw tempdir layout, that is usually a harness design failure.

We should treat repeated custom utilities inside tests as a signal to move that functionality back into the shared harness.

---

## The harness should provide a small internal DSL

Not a huge framework. Just enough shape so tests stay declarative.

Example:

```rust
let t = TestHarness::new().await?;
let project = t.fixture(fixtures::simple_http()).create().await?;
let daemon = t.daemon().start().await?;
let events = t.events().subscribe().await?;

let run = t.cli().up(&project).await?.into_run();
run.assert_service_ready("api").await?;
events.assert_service_state(run.id(), "api", ServiceState::Ready).await?;
```

Key idea:

- `fixture(...)` builds a project
- `daemon()` manages the real daemon process
- `cli()` and `api()` are the outer-edge interfaces
- `into_run()` turns responses into a richer typed handle

The DSL should stay thin. It should just remove ceremony.

---

## Recommended harness API surface

## `TestHarness`

```rust
impl TestHarness {
    pub async fn new() -> Result<Self>;
    pub fn fixture(&self, fixture: impl FixtureSpec) -> FixtureBuilder;
    pub fn daemon(&self) -> DaemonController;
    pub fn cli(&self) -> CliHandle;
    pub fn api(&self) -> ApiHandle;
    pub fn events(&self) -> EventsHandle;
    pub fn fs(&self, project: &ProjectHandle) -> FsHandle;
}
```

### Why this shape is good DX

- one obvious entrypoint
- no test has to wire the pieces together manually
- project-aware filesystem helpers become easy to discover

## `ProjectHandle`

A fixture should return a typed project handle, not just a `PathBuf`.

```rust
pub struct ProjectHandle {
    root: PathBuf,
    name: String,
}

impl ProjectHandle {
    pub fn path(&self) -> &Path;
    pub fn config_path(&self) -> PathBuf;
    pub fn stack(&self, name: &str) -> StackRef;
}
```

This gives us a stable object to hang project-scoped helpers from.

## `CliHandle`

```rust
impl CliHandle {
    pub async fn run(&self, args: &[&str]) -> Result<CmdResult>;
    pub async fn up(&self, project: &ProjectHandle) -> Result<UpResult>;
    pub async fn status_json(&self, run_id: &str) -> Result<RunStatusResponse>;
    pub async fn logs_json(&self, run_id: &str, service: &str, query: LogsQuery) -> Result<LogsResponse>;
}
```

Important: include both:

- generic `run(args)` for edge cases
- semantic helpers like `up()` / `status_json()` for the common path

If we only provide generic command execution, every test will rebuild its own command wrappers.

## `ApiHandle`

Similar rule: both generic and semantic.

```rust
impl ApiHandle {
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T>;
    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T>;

    pub async fn up(&self, req: UpRequest) -> Result<RunManifest>;
    pub async fn status(&self, run_id: &str) -> Result<RunStatusResponse>;
    pub async fn restart_service(&self, run_id: &str, service: &str) -> Result<RunManifest>;
    pub async fn task_status(&self, execution_id: &str) -> Result<TaskStatusResponse>;
}
```

## `RunHandle`

This is one of the highest-value DX objects.

```rust
pub struct RunHandle {
    run_id: String,
    project: ProjectHandle,
    harness: TestHarness,
}

impl RunHandle {
    pub fn id(&self) -> &str;
    pub fn service(&self, name: &str) -> ServiceHandle;
    pub async fn status(&self) -> Result<RunStatusResponse>;
    pub async fn assert_ready(&self) -> Result<()>;
    pub async fn assert_service_ready(&self, service: &str) -> Result<()>;
    pub async fn down(&self) -> Result<RunManifest>;
    pub async fn kill(&self) -> Result<RunManifest>;
}
```

Tests should spend most of their time on `ProjectHandle`, `RunHandle`, `ServiceHandle`, and `TaskHandle`, not on raw ids.

## `ServiceHandle`

```rust
impl ServiceHandle {
    pub async fn assert_ready(&self) -> Result<()>;
    pub async fn assert_failed(&self) -> Result<()>;
    pub async fn assert_log_contains(&self, needle: &str) -> Result<()>;
    pub async fn restart(&self) -> Result<()>;
    pub async fn wait_for_restart_count(&self, count: u64) -> Result<()>;
    pub async fn pause_watch(&self) -> Result<()>;
    pub async fn resume_watch(&self) -> Result<()>;
}
```

This is much nicer than spreading service-oriented helpers across unrelated modules.

---

## Fixture strategy

We need fixture projects that are intentionally tiny and deterministic.

These should not be copied from real projects. They should be synthetic behavior fixtures.

## Fixture design principles

- every service command should be a small script committed in `tests/fixtures/bin/`
- use marker files and deterministic log output
- use tiny HTTP servers or shell scripts rather than heavy real apps
- one fixture should isolate one behavior whenever possible
- fixtures should be **composable**, not just static folders
- fixture authors should be able to customize a base fixture without copying it

## Fixture API should be builder-based

The worst fixture DX is:

- copy an entire project folder
- tweak one file in the test
- repeat forever

Instead, the harness should expose a small builder model:

```rust
let project = t
    .fixture(fixtures::simple_http())
    .with_file(".env.local", "GREETING=hello\n")
    .with_config_patch(|cfg| {
        cfg.service("api").auto_restart(true);
    })
    .create()
    .await?;
```

That gives test authors three paths:

1. use a stock fixture as-is
2. overlay a few files
3. patch the fixture config structurally

All three are far better than forking fixture directories.

## Recommended fixture abstractions

### `FixtureSpec`

```rust
pub trait FixtureSpec {
    fn name(&self) -> &'static str;
    fn render(&self) -> Result<RenderedFixture>;
}
```

### `RenderedFixture`

```rust
pub struct RenderedFixture {
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}
```

### `FixtureBuilder`

```rust
impl FixtureBuilder {
    pub fn with_file(self, path: impl AsRef<Path>, contents: impl Into<Vec<u8>>) -> Self;
    pub fn with_text(self, path: impl AsRef<Path>, contents: impl Into<String>) -> Self;
    pub fn with_config_patch(self, patch: impl FnOnce(&mut FixtureConfig)) -> Self;
    pub async fn create(self) -> Result<ProjectHandle>;
}
```

This keeps the common path ergonomic while still allowing targeted customization.

## Prefer reusable fixture packs over bespoke per-test fixtures

Each stock fixture should be a **fixture pack**:

- a config template
- any helper scripts it needs
- seed files / marker directories
- small helper methods when the fixture has common actions

For example, `watch_restart()` can expose a helper object:

```rust
let fixture = fixtures::watch_restart();
let project = t.fixture(fixture.clone()).create().await?;
fixture.touch_watched_file(&project)?;
fixture.touch_ignored_file(&project)?;
```

That is much better than every watch test deciding on its own file layout.

## Shared helper scripts are part of the harness

Do not let each fixture invent its own little shell ecosystem.

Create a standard toolbox under `tests/fixtures/bin/`, for example:

- `serve-http` — tiny HTTP server with configurable port/path/body
- `emit-log` — print deterministic stdout/stderr lines
- `wait-for-file` — block until a file exists
- `write-marker` — write a marker file
- `append-marker` — append to a marker file for restart counting
- `fail` — exit nonzero with a known message
- `sleep-then` — delay then execute another command
- `env-dump` — dump selected env vars to a file
- `touch-loop` — long-running process that logs heartbeats

If we provide these once, fixtures become config compositions instead of one-off shell scripts.

## Recommended fixture set

### `simple_http`

One service that:

- starts an HTTP server on `$DEV_PORT_API`
- logs a startup line
- exposes `/health`

Used for:

- `up`
- status
- logs
- down
- kill
- restart

### `readiness_matrix`

Several services, each using a different readiness kind:

- tcp
- http
- log regex
- cmd
- delay
- exit
- none

Used for:

- readiness behavior tests

### `init_post_init`

Service with:

- init task writing `init.txt`
- post-init task writing `post-init.txt`
- watchable input file to change init hash behavior

Used for:

- init skip semantics
- post-init rerun semantics
- restart behavior

### `watch_restart`

Service with:

- `auto_restart: true`
- clear watched and ignored paths
- log line on each start including a counter or timestamp file

Used for:

- watch pause/resume
- restart-on-change
- ignore behavior

### `globals_fixture`

Project with one stack service plus one global service.

Used for:

- ensure globals
- listing globals
- global port/url reuse
- global GC

### `tasks_fixture`

Several named tasks:

- success
- failure
- long-running detached
- env-printing
- watched init task

Used for:

- run/list/detach/status/history

### `sources_fixture`

External source files populated by the test itself.

Used for:

- source registration
- source log search
- source refresh/removal behavior

### `agent_fixture`

No real app needed; just enough project context for agent-session API tests.

---

## Assertions should be semantic, not raw

The harness should provide waiting/assertion helpers so tests do not hand-roll polling everywhere.

Examples:

```rust
devstack.assert_run_exists(&run_id).await?;
devstack.assert_service_ready(&run_id, "api").await?;
devstack.assert_service_failed(&run_id, "worker").await?;
devstack.assert_watch_active(&run_id, "api", true).await?;
devstack.assert_log_contains(&run_id, "api", "ready").await?;
devstack.assert_task_completed(&execution_id).await?;
devstack.assert_global_present("moto").await?;
```

These helpers should:

- poll with a short interval
- use daemon API as the source of truth
- print relevant diagnostics on timeout:
  - current status JSON
  - recent logs
  - daemon stderr
  - manifest contents

This is the difference between a harness that helps refactoring and one that becomes miserable to debug.

## Assertions should return rich errors

A timeout like this is bad DX:

```text
timed out waiting for readiness
```

A timeout like this is good DX:

```text
timed out waiting for service api in run dev-1234 to become ready after 10s

last observed state: starting
current run status: { ... }
recent log tail:
  [stdout] booting
  [stderr] port busy
manifest path: /tmp/.../manifest.json
manifest contents: { ... }
daemon stderr tail:
  ...
recent events:
  service state_changed starting
  log stderr port busy
```

The harness should make the second form the default.

## Standard assertion families

To keep tests uniform, provide a small standard vocabulary:

### lifecycle assertions

- `assert_ready()`
- `assert_stopped()`
- `assert_failed()`
- `assert_state(ServiceState::Ready)`

### logs assertions

- `assert_log_contains()`
- `assert_log_not_contains()`
- `assert_recent_error_contains()`
- `assert_logs_match(query, predicate)`

### filesystem assertions

- `assert_exists()`
- `assert_missing()`
- `assert_file_contains()`
- `assert_json_file()`

### event assertions

- `assert_run_created()`
- `assert_service_state()`
- `assert_task_completed()`
- `assert_global_state()`
- `assert_log_event_contains()`

### daemon / process assertions

- `assert_ping()`
- `assert_restart_survives()`
- `assert_socket_exists()`

If a test needs a concept outside this vocabulary more than once, add it to the harness.

## Anti-patterns for test authors

The harness should explicitly discourage these patterns:

- hand-written `loop { sleep(...); ... }` polling in test files
- raw `std::process::Command` except inside the harness
- custom tempdir project layouts inside scenario tests
- direct parsing of manifest files in tests that could use API assertions
- giant snapshot tests of full CLI output
- copying fixture directories just to tweak one config field

If we see these patterns, we should treat them as missing harness features.

---

## CLI testing strategy

We want the harness to preserve CLI behavior, but CLI output is more brittle than API output.

So use two tiers of CLI assertions.

## Tier 1: contract-level CLI assertions

Assert:

- exit status
- machine-readable JSON when available
- presence of key phrases in human output

Examples:

- `devstack status --json` returns a parseable `RunStatusResponse`
- `devstack ls` contains the new run id
- `devstack lint` exits nonzero on invalid config

## Tier 2: human UX smoke tests

Only for a few commands where output shape is itself a user-facing feature:

- `status`
- `logs --facets`
- maybe `doctor`

For these, assert a few stable textual markers, not a giant snapshot of the whole terminal.

Do **not** snapshot the exact entire ANSI-colored output of every command.

---

## API testing strategy

The API suite should be the most complete behavioral surface because it is structured and less brittle.

The harness should test:

- all run lifecycle endpoints
- tasks endpoints
- logs endpoints
- projects/sources endpoints
- navigation intent endpoints
- agent session endpoints
- GC
- SSE events

The API is also the best place to assert asynchronous convergence.

---

## Filesystem assertions

The filesystem is part of devstack’s contract because it creates durable manifests, logs, ledgers, and task history.

The harness should verify these intentionally, but at the correct level.

Good assertions:

- run manifest exists after `up`
- run log file exists and contains expected service output
- task history file records task execution
- run directory disappears after GC/purge

Bad assertions:

- exact JSON formatting whitespace
- exact ordering of unrelated keys

---

## The scenario suite

Below is the suite I would want before the full refactor.

## A. Core lifecycle

### `up_starts_simple_stack_and_status_reports_ready`

- start daemon
- run `devstack up`
- assert one run created
- assert service becomes ready
- assert status shows `running`
- assert manifest exists

### `up_without_new_run_refreshes_existing_run`

- run `up`
- capture run id
- run `up` again on same project/stack
- assert run id is unchanged

### `up_with_new_run_creates_distinct_run`

- run `up`
- run `up --new-run`
- assert second run id differs

### `down_stops_run_and_marks_manifest_stopped`

### `kill_force_stops_run_and_marks_manifest_stopped`

## B. Readiness

One test per readiness kind, each asserting success/failure semantics.

### `no_wait_returns_early_and_background_readiness_converges`

## C. Init / post-init

### `init_runs_before_service_start`

### `init_skips_when_watch_hash_unchanged`

### `post_init_runs_after_readiness`

### `post_init_runs_again_on_refresh`

### `restart_service_runs_post_init_again`

### `failing_init_marks_service_failed_without_starting_process`

### `failing_post_init_marks_service_failed`

## D. Refresh / restart behavior

### `refresh_removes_deleted_services`

### `force_refresh_restarts_even_without_hash_change`

### `restart_service_no_wait_eventually_returns_to_ready`

## E. Watch

### `watched_file_change_triggers_restart`

### `ignored_file_change_does_not_trigger_restart`

### `watch_pause_prevents_restart`

### `watch_resume_restores_restart_behavior`

## F. Logs

### `service_logs_are_queryable_by_service`

### `combined_logs_view_can_filter_by_service_level_stream`

### `logs_since_filters_older_entries`

### `logs_search_returns_matching_entries`

### `logs_follow_returns_incremental_updates`

### `logs_facets_returns_filter_metadata`

### `sse_emits_run_service_task_and_log_events`

## G. Tasks

### `run_lists_available_tasks`

### `run_executes_named_task`

### `run_detach_returns_execution_id_and_task_status_converges`

### `run_init_executes_stack_init_tasks_without_starting_services`

### `run_verbose_streams_output`

## H. Globals

### `up_ensures_globals_and_list_globals_reports_them`

### `globals_reuse_existing_instance_when_already_active`

### `globals_run_post_init`

### `gc_all_removes_stopped_globals`

## I. Projects and sources

### `up_touches_project_in_ledger`

### `projects_add_list_remove_round_trip`

### `sources_add_list_remove_round_trip`

### `source_logs_can_be_queried`

## J. Navigation / agent

### `navigation_intent_round_trip`

### `agent_session_register_poll_share_unregister_round_trip`

### `share_targets_latest_session_for_project`

## K. GC / persistence

### `gc_removes_old_stopped_runs`

### `gc_does_not_remove_running_runs`

### `daemon_restart_preserves_visible_run_state`

This last test is especially important because the refactor is likely to change internal persistence and reload paths.

---

## Harness implementation details

## Binary invocation

Use the compiled test binary, not `cargo run`.

That means:

- integration tests should use `CARGO_BIN_EXE_devstack` or equivalent
- every command invocation is a real process
- the daemon is a real child process

## Timeouts

Every await helper should use bounded timeouts.

Suggested defaults:

- daemon start/ping: 5s
- service readiness in fixtures: 10s
- detached task completion: 10s
- watch-triggered restart: 10s
- SSE event arrival after action: 5s

Make them overridable per test.

## Polling vs events

Use both.

- use polling for authoritative state convergence (`status`, `task_status`)
- use SSE for event assertions and ordering-sensitive checks

Do not try to make the entire suite event-only.

## Diagnostics on failure

Every failed semantic assertion should dump enough context to debug quickly:

- latest daemon stderr
- current `status` JSON
- current manifest file
- recent service log tail
- captured event stream tail

This is mandatory.

---

## A note on the API/CLI boundary

Some behaviors should be tested primarily through CLI, some primarily through API.

### Best tested through CLI

- command UX
- argument parsing
- config discovery from cwd
- output mode behavior
- detached task command shape

### Best tested through API

- structured state
- async transitions
- logs/filter correctness
- navigation intent
- agent session mechanics
- SSE events

The same scenario can use both:

1. invoke action via CLI
2. assert convergence via API

That is probably the best default pattern.

---

## What should remain stable through the refactor

The harness should treat these as stable contracts unless we explicitly choose to change product behavior:

- CLI command names and major flags
- daemon endpoint paths and response semantics
- run/global/task manifest presence and broad meaning
- event categories (`run`, `service`, `task`, `global`, `log`)
- lifecycle semantics for start/refresh/restart/down/kill
- init/post-init behavior
- watch pause/resume semantics
- logs query/filter behavior
- project/source ledger behavior
- navigation intent behavior
- agent session share/poll behavior
- GC semantics

---

## Recommended next move

Before touching the refactor itself, implement the harness foundation and a small set of high-value scenarios:

1. daemon start/ping
2. simple `up -> status -> down`
3. `up --no-wait` readiness convergence
4. restart-service reruns post-init
5. watch pause/resume
6. logs view + facets
7. detached task execution
8. globals listing
9. agent session share/poll
10. daemon restart preserves visible state

That set will give us a meaningful safety net over the most structurally risky areas of the refactor.

But the important mindset shift is this:

> the purpose of the harness is not to test the refactor in pieces
>
> it is to lock down the product-level behavior of devstack so the refactor can be done as one clean break

# Devstack Refactor Plan

## Why this plan changed

After a fresh pass over the current code, the original refactor plan still pointed in the right direction, but it was too **file-centric** and not quite **workflow-centric** enough.

The code is not just suffering from large files. It is suffering from a few deeper structural problems:

- the daemon owns both **transport** and **application logic**
- one `Mutex<DaemonState>` protects unrelated concerns
- "globals" reuse run mechanics without being modeled as a first-class concept
- the same launch lifecycle is duplicated across `orchestrate_up`, `orchestrate_refresh_run`, and `ensure_globals`
- read paths like `build_status` also mutate state and persist manifests
- `RunManifest` currently serves as persistence model, transport model, and quasi-domain model
- several traits in the earlier proposal were abstractions without a strong payoff

So this version shifts the refactor from "split big files" to "make the runtime model explicit."

## Design goals

- make transport code obviously separate from application logic
- make state mutation explicit and localized
- make service launch a single reusable pipeline
- model globals directly instead of treating them as fake runs
- separate disk format, API format, and in-memory runtime state
- keep abstractions minimal and justified
- make the code easy to test without turning it into an abstraction festival

## Architecture

```text
transport → app commands / app queries → stores + launch pipeline → infra
                                  ↓
                           model / config / persistence
```

### Layer meanings

- **transport**: axum handlers, CLI command dispatch, agent entrypoints, OpenAPI wiring
- **app commands**: operations that change state (`up`, `refresh`, `restart`, `down`, `gc`, `ensure_globals`)
- **app queries**: read-side operations (`status`, `list_runs`, `logs_view`, `list_globals`)
- **stores**: in-memory runtime state wrappers
- **launch pipeline**: shared service preparation / start / readiness / post-init flow
- **infra**: systemd, Tantivy, filesystem, Unix socket client, watchers
- **model / config / persistence**: pure types, config parsing, manifest file models

## The main architectural changes

### 1. Move application logic out of `daemon/`

The previous plan put the main logic under `daemon/orchestration/`. That still couples the core behavior to one transport.

The current code already shows this is the wrong ownership boundary:

- HTTP handlers call orchestration functions
- background readiness tasks call orchestration functions
- health monitors and auto-restart watchers call orchestration functions
- status requests perform reconciliation and persistence

That is application logic, not daemon logic.

So the core behavior should move under an `app/` layer, and `daemon/` should become mostly transport/bootstrap.

### 2. Replace one `DaemonState` lock with focused stores

Today `AppState` holds one `Arc<Mutex<DaemonState>>` containing:

- `runs`
- `detached_tasks`
- `agent_sessions`
- `navigation_intent`

These are not one coherent state aggregate.

The refactor should split them into focused stores:

- `RunStore`
- `TaskStore`
- `AgentSessionStore`
- `NavigationStore`

This is not about performance theater. It is about boundary clarity.

Agent session polling should not conceptually share the same mutation surface as run orchestration. Navigation intent should not live under the same lock as service lifecycle transitions.

### 3. Model globals explicitly

`ensure_globals()` currently duplicates much of the service launch flow while inventing fake run ids like `global-{key}` and persisting global manifests separately from in-memory daemon state.

That is a strong sign the model is missing a concept.

The refactor should introduce an explicit instance scope:

```rust
pub enum InstanceScope {
    Run {
        run_id: RunId,
        stack: String,
    },
    Global {
        key: String,
        project_dir: PathBuf,
        name: String,
    },
}
```

The launch pipeline can then operate on an `InstanceScope` instead of assuming everything is a run.

This avoids both extremes:

- globals are not shoehorned into fake runs
- globals do not require a totally separate launch implementation

### 4. Extract a shared launch pipeline early

The previous plan treated lifecycle deduplication as a late cleanup step. That is too late.

The code currently duplicates the same conceptual workflow across:

- `orchestrate_up`
- `orchestrate_refresh_run`
- `ensure_globals`

That shared workflow is the heart of the system:

1. resolve ports
2. build template context
3. resolve cwd and env file
4. render env
5. compute watch fingerprint/hash
6. build unit definition
7. start service
8. wait for readiness
9. run post-init
10. update runtime state
11. persist manifest/snapshot
12. sync background watchers

This should become a first-class pipeline, not three hand-rolled flows.

### 5. Separate commands from queries

The current `build_status()` function does too much. It:

- reads run state
- queries systemd
- derives service state
- mutates daemon state
- emits events
- persists the manifest

That means a read operation is also a write operation.

The refactor should separate:

- **commands**: mutate state or reconcile runtime state
- **queries**: return a snapshot / response only

That can be implemented in either of two acceptable ways:

- a dedicated `reconcile_run` command invoked before status reads
- a background reconciler that keeps runtime snapshots current

Either way, the `status` query itself should be side-effect free.

### 6. Split persistence models from API models

`RunManifest` currently does too many jobs:

- on-disk persistence model
- API response type
- shared representation used by CLI and daemon internals

This is already showing strain. For example, `watch_hash` exists on `ServiceManifest` but is skipped in serialization, which means the type is being pulled in incompatible directions.

The refactor should split this into separate model families:

- `persistence::PersistedRun`
- `persistence::PersistedService`
- `protocol::RunResponse`
- `protocol::ServiceResponse`
- runtime-only in-memory records in `model/`

Mapping code is cheaper than leaking storage concerns everywhere.

### 7. Keep traits minimal

The earlier proposal still had too many traits, and it also had an inconsistency around readiness: it both introduced a readiness port and argued against extracting one yet.

The right rule is simple:

> add a trait only when it creates a meaningful test seam or isolates an external boundary we genuinely want to swap

That leads to a smaller set.

## Proposed module tree

```text
src/
├── main.rs
├── lib.rs
│
├── model/
│   ├── ids.rs
│   ├── instance_scope.rs           # Run vs Global scope
│   ├── run.rs                      # RunRecord, RunLifecycleState
│   ├── service.rs                  # ServiceSpec, ServiceLaunchPlan, ServiceRecord,
│   │                               # ServiceRuntimeState, ServiceHandles
│   └── agent_session.rs
│
├── protocol/
│   ├── api.rs                      # request/response DTOs only
│   ├── events.rs                   # SSE event payloads only
│   └── openapi.rs
│
├── persistence/
│   ├── manifest.rs                 # PersistedRun, PersistedService
│   ├── daemon_state.rs             # persisted daemon metadata if still needed
│   └── codecs.rs                   # load/save helpers
│
├── config/
│   ├── model.rs
│   ├── load.rs
│   ├── validate.rs
│   ├── plan.rs                     # stack_plan, topo sort, globals map
│   └── env.rs
│
├── stores/
│   ├── runs.rs                     # RunStore
│   ├── tasks.rs                    # TaskStore
│   ├── agent_sessions.rs           # AgentSessionStore
│   └── navigation.rs               # NavigationStore
│
├── app/
│   ├── context.rs                  # RuntimeDeps + shared app wiring
│   ├── launch/
│   │   ├── context.rs              # LaunchContext, template/env context
│   │   ├── prepare.rs              # prepare_service
│   │   ├── start.rs                # start service/global
│   │   ├── readiness.rs            # readiness + post-init flow
│   │   ├── pipeline.rs             # launch_service / relaunch_service
│   │   └── watch.rs                # watch setup/sync helpers
│   ├── commands/
│   │   ├── up.rs
│   │   ├── refresh.rs
│   │   ├── restart.rs
│   │   ├── down.rs
│   │   ├── kill.rs
│   │   ├── ensure_globals.rs
│   │   ├── gc.rs
│   │   ├── reconcile.rs
│   │   ├── tasks.rs
│   │   └── navigation.rs
│   └── queries/
│       ├── status.rs
│       ├── runs.rs
│       ├── globals.rs
│       ├── logs.rs
│       └── watch.rs
│
├── infra/
│   ├── fs/
│   │   ├── paths.rs
│   │   ├── util.rs
│   │   ├── projects_ledger.rs
│   │   └── sources_ledger.rs
│   ├── ipc/
│   │   └── unix_daemon_client.rs   # shared concrete client for CLI + agent
│   ├── logs/
│   │   ├── parser.rs
│   │   ├── streamer.rs
│   │   └── index/
│   │       ├── mod.rs
│   │       ├── schema.rs
│   │       ├── ingest.rs
│   │       ├── query.rs
│   │       ├── facets.rs
│   │       └── compaction.rs
│   ├── runtime/
│   │   ├── systemd.rs
│   │   ├── local_supervisor.rs
│   │   ├── shim.rs
│   │   ├── port_allocator.rs
│   │   └── file_watch.rs
│   └── time/
│       └── clock.rs
│
├── services/
│   ├── readiness/
│   │   ├── model.rs
│   │   ├── coordinator.rs
│   │   ├── probes.rs
│   │   └── port_ownership.rs
│   ├── tasks/
│   │   ├── model.rs
│   │   ├── executor.rs
│   │   ├── orchestration.rs
│   │   └── history.rs
│   └── diagnose.rs
│
├── daemon/
│   ├── bootstrap.rs
│   ├── router.rs
│   ├── error.rs
│   ├── event_bus.rs
│   ├── log_tailing.rs
│   └── handlers/
│       ├── ping.rs
│       ├── runs.rs
│       ├── tasks.rs
│       ├── watch.rs
│       ├── logs.rs
│       ├── events.rs
│       ├── projects.rs
│       ├── sources.rs
│       ├── navigation.rs
│       ├── globals.rs
│       ├── gc.rs
│       └── agent.rs
│
├── cli/
│   ├── args.rs
│   ├── context.rs
│   ├── output.rs
│   └── commands/
│       ├── up.rs
│       ├── status.rs
│       ├── down.rs
│       ├── logs.rs
│       ├── tasks.rs
│       ├── watch.rs
│       ├── diagnose.rs
│       ├── projects.rs
│       ├── sources.rs
│       ├── exec.rs
│       ├── gc.rs
│       ├── install.rs
│       ├── init.rs
│       ├── doctor.rs
│       ├── completions.rs
│       ├── openapi.rs
│       └── ui.rs
│
└── agent/
    ├── command.rs
    ├── pty_proxy.rs
    └── auto_share.rs
```

## Core runtime types

### `RunStore` and friends

The most important store is the run store, but it should not become a generic mutable bag.

Prefer intention-revealing methods over a generic `with_run_mut` API.

```rust
pub struct RunStore {
    inner: Mutex<BTreeMap<String, RunRecord>>,
}

impl RunStore {
    pub async fn create_run(&self, run: RunRecord) -> Result<()>;
    pub async fn get_run(&self, run_id: &str) -> Option<RunRecord>;
    pub async fn list_runs(&self) -> Vec<RunRecord>;
    pub async fn remove_run(&self, run_id: &str) -> Option<RunRecord>;

    pub async fn insert_service(&self, run_id: &str, service: String, record: ServiceRecord) -> Result<()>;
    pub async fn mark_service_starting(&self, run_id: &str, service: &str) -> Result<Vec<DaemonEvent>>;
    pub async fn mark_service_ready(&self, run_id: &str, service: &str) -> Result<Vec<DaemonEvent>>;
    pub async fn mark_service_failed(&self, run_id: &str, service: &str, reason: String) -> Result<Vec<DaemonEvent>>;
    pub async fn mark_run_stopped(&self, run_id: &str) -> Result<Vec<DaemonEvent>>;
}
```

The point is not to hide state. The point is to stop every callsite from having to remember all the bookkeeping rules.

### `ServiceRecord`

The earlier `ServiceRuntime` split was right and should stay, but it should align to the launch pipeline.

```rust
pub struct ServiceSpec {
    pub name: String,
    pub deps: Vec<String>,
    pub readiness: ReadinessSpec,
    pub auto_restart: bool,
    pub watch_patterns: Vec<String>,
    pub ignore_patterns: Vec<String>,
}

pub struct ServiceLaunchPlan {
    pub unit_name: String,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cmd: String,
    pub log_path: PathBuf,
    pub port: Option<u16>,
    pub scheme: String,
    pub url: Option<String>,
    pub watch_hash: String,
    pub watch_fingerprint: Vec<u8>,
    pub watch_extra_files: Vec<PathBuf>,
}

pub struct ServiceRuntimeState {
    pub state: ServiceState,
    pub last_failure: Option<String>,
    pub last_started_at: Option<String>,
    pub watch_paused: bool,
}

pub struct ServiceHandles {
    pub health: Option<HealthHandle>,
    pub watch: Option<ServiceWatchHandle>,
}

pub struct ServiceRecord {
    pub spec: ServiceSpec,
    pub launch: ServiceLaunchPlan,
    pub runtime: ServiceRuntimeState,
    pub handles: ServiceHandles,
}
```

This keeps immutable launch data separate from live task handles.

## Minimal trait strategy

Use traits only where they buy us something real.

### Keep as traits

- `SystemSupervisor`
- `Clock`
- `IdGenerator` if deterministic ids matter in tests
- optionally `TaskExecutor` if app command tests need to fake task execution

### Keep concrete for now

- shared Unix socket daemon client
- manifest persistence helpers
- project/source ledgers
- readiness coordinator
- Tantivy log index unless tests prove a fake is really valuable

A shared concrete Unix daemon client already solves the obvious duplication between `src/cli.rs` and `src/agent.rs` without paying the cost of a port that only has one implementation.

## Specific improvements over the previous version

### 1. `daemon/orchestration/` becomes `app/commands` and `app/queries`

This is the most important naming and ownership correction.

### 2. `RunRegistry` becomes `RunStore`, and `DaemonState` is no longer the central aggregate

The original plan improved the lock access pattern but still treated runtime state as one giant thing. Splitting stores is cleaner.

### 3. `ensure_globals` is no longer a side lane

Globals must share the launch pipeline through `InstanceScope`.

### 4. Service lifecycle deduplication moves from late cleanup to early design

Do not first split three duplicated launch flows into different files and only later try to reunify them.

### 5. `build_status` stops mutating state

Status should read a reconciled snapshot, not perform reconciliation itself.

### 6. Persistence and API models are split

`RunManifest` should stop doing three jobs.

## Fresh execution order

## Phase 1: pure extractions

These are still worth doing first because they are low-risk and make the later structural changes easier.

1. split `config.rs` into `config/{model,load,validate,plan,env}.rs`
2. split `readiness.rs` into `services/readiness/{model,coordinator,probes,port_ownership}.rs`
3. split `tasks.rs` into `services/tasks/{model,executor,orchestration,history}.rs`
4. split `log_index.rs` into `infra/logs/index/{mod,schema,ingest,query,facets,compaction}.rs`
5. move low-level helpers into `infra/fs/`, `infra/runtime/`, and `infra/logs/`
6. split `agent.rs` into `agent/{command,pty_proxy,auto_share}.rs`
7. split `cli.rs` into `cli/{args,context,output,commands/*}.rs`

## Phase 2: model and persistence cleanup

1. introduce `model::instance_scope`
2. introduce `model::run` and `model::service`
3. introduce `persistence::manifest`
4. map between runtime models, persisted models, and protocol responses
5. replace direct `RunManifest` leakage in internal code paths

## Phase 3: split state into focused stores

1. extract `RunStore`
2. extract `TaskStore`
3. extract `AgentSessionStore`
4. extract `NavigationStore`
5. remove raw `Mutex<DaemonState>` access from app logic

## Phase 4: build the shared launch pipeline

1. extract env/template preparation into `app/launch/context.rs`
2. extract service preparation into `app/launch/prepare.rs`
3. extract service start into `app/launch/start.rs`
4. extract readiness + post-init flow into `app/launch/readiness.rs`
5. extract watcher sync into `app/launch/watch.rs`
6. make `orchestrate_up`, `orchestrate_refresh_run`, and `ensure_globals` all delegate to the pipeline

## Phase 5: create the app layer

1. move mutating flows into `app/commands/*`
2. move reads into `app/queries/*`
3. extract reconciliation into `app/commands/reconcile.rs`
4. make daemon handlers thin wrappers around app commands/queries

## Phase 6: shrink daemon into transport/bootstrap

1. extract `daemon/error.rs`
2. extract `daemon/event_bus.rs`
3. extract `daemon/log_tailing.rs`
4. extract handlers into `daemon/handlers/*`
5. extract router/bootstrap

## Phase 7: share the Unix daemon client

1. move duplicated socket HTTP client code from CLI and agent into `infra/ipc/unix_daemon_client.rs`
2. make both CLI and agent use it directly

## Hard parts to watch carefully

### `orchestrate_refresh_run` is still the hardest flow

It mixes:

- config reload
- service diffing
- removal of deleted services
- port reuse rules
- relaunch decisions
- state transitions
- readiness flow
- watcher sync

That function should be decomposed around decisions, not around arbitrary chunks of lines.

### `ensure_globals` is the design smell that should guide the refactor

It currently duplicates launch behavior because the runtime model does not know what a global instance is. If the new design still leaves globals as a weird exception, the refactor did not go deep enough.

### background tasks should call app commands, not private daemon helpers

Health monitors, readiness tasks, and auto-restart watchers should depend on the app layer, not on transport-local modules.

### `status` should not be the reconciliation engine forever

The current code uses status reads as an opportunity to repair runtime state. The new architecture should make that explicit and relocatable.

## Success criteria

- `daemon/` is mostly handlers, router, bootstrap, and event plumbing
- launch logic exists once and is reused by runs and globals
- globals are explicitly modeled
- no single state lock protects unrelated concerns
- query paths are side-effect free
- persistence structs are distinct from API structs
- CLI and agent share one concrete Unix daemon client
- most files are under 400 LOC and almost none exceed 800 LOC
- tests can exercise app commands by faking only a small number of real boundaries

## Non-goals

- introducing a DI framework
- abstracting every helper behind a trait
- preserving the current accidental type boundaries just because they exist today
- modeling every background task as a framework unto itself

## Bottom line

The original plan correctly identified the oversized files and the need for better seams. The improved plan goes one level deeper:

- the core split is not just `daemon.rs` into many files
- it is **transport vs app vs stores vs launch pipeline vs persistence**

If we do that well, the file split becomes a consequence of a clearer design instead of the refactor's primary goal.

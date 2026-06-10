# Design: Service Tasks & Global Env

## Problem

Tasks and services have completely separate env pipelines. A task defined in `[tasks]` builds its own environment from its `TaskDefinition.env` and `TaskDefinition.env_file`. When a service references a task via `init` or `post_init`, that task runs with zero knowledge of the service's environment — no `DEV_*` vars, no rendered templates, no service env overrides.

This means:
- `{{ services.moto.url }}` in a task env value is passed as a raw literal string
- A service's `DB_URL` override doesn't reach its own init task
- Tasks can't reference dep URLs that the service's env already has

Additionally, there's no way to share env across services. Every service that needs `ENVIRONMENT = "local"` or `env_file = ".env"` must declare it independently.

## Design

Two changes:

### 1. Global env on ConfigFile

A top-level `env` map and optional `env_file` on the config. Every service inherits these as a base layer, before its own `env_file` and `env` overrides.

```toml
version = 1
env_file = ".env"

[env]
ENVIRONMENT = "local"
AWS_ACCESS_KEY_ID = "test"

[stacks.voice.services.voice-server]
cmd = "poetry run start-server --port $PORT"
# inherits ENVIRONMENT=local, AWS_ACCESS_KEY_ID=test, and .env file contents
# no need to repeat env_file = ".env" or ENVIRONMENT = "local"

[stacks.voice.services.call-analyzer]
cmd = "python -m call_analyzer"
env = { DB_URL = "mysql://custom-url" }
# inherits global env, DB_URL overrides any DB_URL from global env or env_file
```

### 2. Service-scoped tasks

Tasks defined directly on a service. These are resolved before global `[tasks]` (service-local shadows global). When executed as init/post_init, they receive the service's full resolved env as their base environment.

```toml
[stacks.voice.services.call-analyzer]
cmd = "python -m call_analyzer"
deps = ["voice-server"]
env = { DB_URL = "mysql://localhost:3311/analyzer" }
init = ["migrate"]
post_init = ["seed-data"]

[stacks.voice.services.call-analyzer.tasks.migrate]
cmd = "alembic upgrade head"
watch = ["alembic/"]

[stacks.voice.services.call-analyzer.tasks.seed-data]
cmd = "python scripts/seed.py"
```

When `migrate` runs, it gets the full service env: `DEV_*` vars + global env_file + global env + service env_file + dep env + service env (rendered). The task's own `env`/`env_file` (if any) layer on top as overrides.

Global tasks still work for shared/reusable tasks (like `poetry-install-voice` used by multiple services) and for standalone `devstack run <task>`.

## Env Cascade

The full resolution order for a service's env (and thus its tasks' base env):

```
1. base_env          — DEV_RUN_ID, DEV_STACK, DEV_PORT_*, DEV_URL_* (from devstack internals)
2. global env_file   — loaded from ConfigFile.env_file, resolved relative to config dir
3. global env        — ConfigFile.env, rendered with template context
4. service env_file  — loaded from ServiceConfig.env_file (or .env in service cwd if not set)
5. dep env           — DEV_DEP_*_PORT, DEV_DEP_*_URL for declared deps
6. port env          — PORT (or custom port_env)
7. service env       — ServiceConfig.env, rendered with template context
8. $VAR resolution   — resolve_env_map (expand $VAR / ${VAR} references)
```

When a task runs in service context, it gets this full resolved env as `base_env`. The task's own `env_file` and `env` are additional overrides on top (steps 9 and 10).

When a task runs standalone (`devstack run`), it gets the run's `base_env` (step 1 only — the `DEV_*` vars) plus the global env (steps 2-3). Not the full service env, since there's no service context.

## Task Name Resolution

When resolving init/post_init task names for a service:

1. Look in `service.tasks` first
2. Fall back to `config.tasks` (global)

Service task names shadow global task names. This lets a service override a global task with a service-specific version if needed.

## Implementation

### Step 1: Config model

**`src/config/model.rs`**

Add to `ConfigFile`:
```rust
#[serde(default)]
pub env: BTreeMap<String, String>,
#[serde(default)]
pub env_file: Option<PathBuf>,
```

Add to `ServiceConfig`:
```rust
#[serde(default)]
pub tasks: Option<UniqueMap<String, TaskConfig>>,
```

### Step 2: Validation

**`src/config/validate.rs`**

Update `validate_service_init_tasks` and `validate_service_post_init_tasks`:
- Accept `service_tasks: Option<&UniqueMap<String, TaskConfig>>` in addition to global tasks
- A task name is valid if it exists in either service tasks or global tasks
- Remove the requirement that `[tasks]` must exist if init/post_init are used (service tasks suffice)
- Validate service task names with `validate_name_for_path_component`

### Step 3: Global env in prepare_service

**`src/app/launch/prepare.rs`**

Change `prepare_service` to accept global env context. Rather than adding more individual parameters to an already 8-param function, add them to a context struct or just add the two fields. 

Add `global_env: &BTreeMap<String, String>` and `global_env_file: Option<&Path>` params.

In the env build section, insert global env between base_env and service env:

```rust
let mut env = base_env.clone();

// Global env file
if let Some(path) = global_env_file {
    let file_env = load_env_file(path)?;
    merge_env_file(&mut env, file_env);
}

// Global env (rendered with templates)
let rendered_global_env = render_env(global_env, &template_context)?;
env.extend(rendered_global_env);

// Service env file (existing behavior)
let file_env = load_env_file(&env_file_path)?;
merge_env_file(&mut env, file_env);

// ... rest unchanged (inject_dep_env, port env, service env, resolve_env_map)
```

**Callers to update (4 sites):**
1. `src/app/commands/runs.rs:launch_service` — has config access via call chain
2. `src/app/commands/ensure_globals.rs` — globals: pass empty global env (globals are standalone)
3. `src/persistence/daemon_state.rs` (2 sites) — has config access, thread it through

### Step 4: Task execution with base env

**`src/services/tasks/executor.rs`**

Add `base_env: &BTreeMap<String, String>` to `run_task`. Apply it before the task's own env_file and env:

```rust
for (k, v) in base_env {
    command.env(k, v);
}
// then task env_file
// then task env
```

Update `run_service_tasks` to accept and pass through `base_env`.
Update `run_init_tasks` and `run_post_init_tasks` to accept and pass through `base_env`.

### Step 5: Wire init/post_init in launch_service

**`src/app/commands/runs.rs`**

In `launch_service`, after `prepare_service`:

```rust
// Build resolved task map: service tasks shadow global tasks
let resolved_tasks = resolve_service_tasks(service, tasks_map);

// Pass prepared.env as base_env for init tasks
run_init_tasks_blocking(
    resolved_tasks.clone(),
    init_tasks.clone(),
    project_dir.to_path_buf(),
    run_id.clone(),
    prepared.env.clone(),
).await
```

Similarly for post_init via `PostInitContext`.

Helper:
```rust
fn resolve_service_tasks(
    service: &ServiceConfig,
    global_tasks: &BTreeMap<String, TaskConfig>,
) -> BTreeMap<String, TaskConfig> {
    let mut tasks = global_tasks.clone();
    if let Some(service_tasks) = &service.tasks {
        tasks.extend(service_tasks.as_map().clone());
    }
    tasks
}
```

**`src/app/commands/tasks.rs`**

Update `run_init_tasks_blocking` and `run_post_init_tasks_blocking` to accept and thread `base_env`.

**`src/app/launch/readiness.rs`** and **`src/app/launch/pipeline.rs`**

Update `PostInitContext` to carry `base_env`. Thread it through to `run_post_init_tasks_blocking`.

### Step 6: Standalone task execution

**`src/cli/commands/tasks.rs`**

For `devstack run <task>` (inline, not detached):
- If there's an active run, load the manifest and use `manifest.env` as base
- Also load global env from config, merge on top
- Pass as base_env to `run_task`

**`src/app/commands/tasks.rs`**

For `execute_detached_task` (detached via daemon):
- Similar: get run's base_env if available, merge global env
- Pass as base_env to `run_task`

### Step 7: Global env_file path resolution

**`src/app/launch/prepare.rs`** or a new helper

The global `env_file` from ConfigFile is relative to the config dir (where `devstack.toml` lives). Resolve it before passing to `prepare_service`:

```rust
let global_env_file_path = config.env_file.as_ref().map(|p| {
    if p.is_absolute() { p.clone() } else { config_dir.join(p) }
});
```

This resolution happens in `build_new_launch_resources` and `build_refresh_launch_resources`, then stored in `LaunchResources`.

### Step 8: LaunchResources changes

**`src/app/commands/runs.rs`**

Add to `LaunchResources`:
```rust
global_env: BTreeMap<String, String>,
global_env_file_path: Option<PathBuf>,
```

Populate from config in `build_new_launch_resources` and `build_refresh_launch_resources`.

Thread through `launch_services` → `launch_service` → `prepare_service`.

## Files Changed

| File | Change |
|------|--------|
| `src/config/model.rs` | Add `env`, `env_file` to ConfigFile; add `tasks` to ServiceConfig |
| `src/config/validate.rs` | Update init/post_init validation to check service tasks |
| `src/app/launch/prepare.rs` | Add global env to prepare_service pipeline |
| `src/app/launch/context.rs` | No changes needed |
| `src/services/tasks/executor.rs` | Add base_env to run_task, run_service_tasks |
| `src/services/tasks/mod.rs` | Update re-exports if needed |
| `src/app/commands/runs.rs` | Thread global env through launch, resolve service tasks |
| `src/app/commands/tasks.rs` | Add base_env to blocking wrappers and detached execution |
| `src/app/launch/readiness.rs` | Add base_env to PostInitContext |
| `src/app/launch/pipeline.rs` | Thread base_env to post_init execution |
| `src/persistence/daemon_state.rs` | Thread global env to prepare_service calls |
| `src/app/commands/ensure_globals.rs` | Pass empty global env for globals |
| `src/cli/commands/tasks.rs` | Build base_env for standalone task runs |

## What This Doesn't Change

- Template rendering in task env values. Not needed — service tasks inherit the service's already-rendered env. The friction report issue (#1) is solved by inheritance, not by adding rendering to tasks.
- Dashed service name template fix. Separate concern, separate PR.
- Parallel service startup. Separate concern.
- `devstack exec --service`. Separate concern (but trivially implementable once per-service env is built).

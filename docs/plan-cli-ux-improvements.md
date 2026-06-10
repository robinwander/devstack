# Plan: CLI UX Improvements

## Changes

### 1. Dashed template names

`{{ services.voice-server.url }}` silently evaluates to undefined because Minijinja parses `voice-server` as subtraction. 

**Fix:** In `build_template_context`, insert an underscore-normalized alias for each service name that contains a dash. `voice-server` → also available as `voice_server`.

**File:** `src/app/launch/context.rs`

### 2. `devstack up [stack] [services...]`

Currently `devstack up` takes one optional positional arg (stack name). Change it to accept multiple positional args: the first is the stack name, the rest are service names to include (+ their transitive deps). If only a stack is given, behavior is unchanged.

**Parsing:** CLI positional `targets: Vec<String>`. First element = stack, rest = services. If empty, use default_stack.

**Filtering:** Add `services: Vec<String>` to `UpRequest`. In the daemon, if non-empty, compute transitive deps and filter the `StackPlan` to only those services before launching.

**New method:** `StackPlan::filter_to(targets)` — BFS from targets following deps, keep only reachable services, preserve topo order.

**Files:** `src/cli/args.rs`, `src/cli/commands/lifecycle.rs`, `src/cli/commands/mod.rs`, `src/api.rs`, `src/app/commands/runs.rs`, `src/config/plan.rs`

### 3. `devstack logs <target>` positional

Currently `devstack logs` requires `--service api` or `--source foo`. Add a positional arg that resolves to the right one automatically:

- If the target matches a service in the active run → use as service
- Else if it matches a registered source → use as source  
- Else → use as service (will fail with normal error)

Flags `--service` and `--source` still work and take priority over the positional.

**Files:** `src/cli/args.rs`, `src/cli/commands/logs.rs`, `src/cli/commands/mod.rs`

### 4. Init task visibility check

Verify whether our env-inheritance changes improved init task visibility. The answer is no — the service is still only registered in the run store after init completes. This is a separate concern (not in scope for this PR) but good to confirm.

## Test Plan

### Dashed template names
- Service `my-api` with `env.SELF_URL = "{{ services.my_api.url }}"` → service sees the rendered URL, not the literal template string

### Up with service filter  
- Multi-service fixture (api + worker), `devstack up dev api` → only `api` starts, `worker` does not
- Verify the dep case: service with deps, request only the leaf → deps also start

### Logs positional
- `devstack logs api` (positional, no `--service`) → returns logs for the `api` service
- `--service` flag still works alongside positional (flag wins / same behavior)

## Implementation Order

1. Write e2e tests (should fail)
2. Implement dashed template names (one-liner)
3. Implement `StackPlan::filter_to` + wire through `up`
4. Implement logs positional target
5. Verify all tests pass
6. Run full suite for regressions

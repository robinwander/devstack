# Devstack Friction Report — augment-voice monorepo

**Date:** 2026-04-02
**Context:** Setting up the full augment-voice stack (7 services: moto, voice-server, voice-worker, call-analyzer, test-worker, sim-api, sim-ui) with moto for local AWS mocking. Involved adding a new service (call-analyzer) from scratch.

---

## Devstack Issues

### 1. Templates don't work in init/post_init task env — and nothing tells you

The single biggest time sink. I put `{{ services.moto.url }}` in a task's env, `devstack lint` passed, `devstack up` started — and the task silently received the raw literal `{{ services.moto.url }}` string instead of the resolved URL. No warning, no error, just broken behavior at runtime.

I had to read the Rust source (`run_task` → `task_cmd_parts()` → `def.env.clone()`) to discover that template rendering only applies to service configs, not task configs.

**Impact:** Hours of debugging. Had to rewrite `moto-init.py` to self-discover the moto port by reading devstack's manifest JSON from disk — a hacky workaround.

**Suggestion:** Either render templates in task env values too, or reject `{{` syntax in task env values at lint time with a clear error.

---

### 2. Templates can't handle service names with dashes

`{{ services.voice-server.url }}` fails with `"undefined value"` because the template engine interprets `voice-server` as the expression `voice minus server`. This is a silent gotcha since devstack encourages dashed service names everywhere else in the config.

**Impact:** Had to remove the template reference entirely and use `DEV_URL_VOICE_SERVER` env var instead.

**Suggestion:** Either support bracket notation (`services["voice-server"].url`) or auto-normalize dashed names into the template context (e.g., expose as `services.voice_server.url` alongside the original).

---

### 3. `devstack up` gives zero visibility into what's blocking

When `devstack up` hung for 5+ minutes, I had no idea why. `devstack status` only showed the 2 services that had already reached "ready" state. The other 5 services weren't listed at all — they simply didn't exist in the run yet. There was no indication that:

- An init task (`alembic upgrade head`) was stuck in an exponential-backoff retry loop
- Which specific task was running
- What the task's stdout/stderr was producing

I had to `ps aux | grep alembic` to find the stuck process and diagnose the root cause (wrong DB port).

**Impact:** Multiple wasted `devstack up` cycles at 5+ minutes each while trying to figure out what was stuck.

**Suggestion:**
- `devstack status` should show services in `initializing` state with the name of the currently-running init task
- Init task stdout/stderr should be captured to a log file accessible via something like `devstack logs --task migrate-analyzer`
- Consider a progress line during `devstack up` showing which service/task is currently being processed

---

### 4. Init tasks don't inherit service env overrides

The call-analyzer service has `DB_URL` overridden in `[services.call-analyzer.env]` to point at port 3311 (shared MySQL). But the `migrate-analyzer` init task ran with the `.env` file's `DB_URL` (port 3306), because init tasks only receive the task's own env + env_file, not the service's env overrides.

This is deeply counterintuitive. The init tasks are declared on the service (`init = ["migrate-analyzer"]`) — they exist *for* that service. Why would they use a different environment?

**Impact:** The alembic migration retried for 10 minutes against a non-existent DB on port 3306, blocking the entire stack startup. The fix was editing the `.env` file, which shouldn't have been necessary since the devstack config already had the correct override.

**Suggestion:** Init tasks should inherit the service's merged env (env_file + service env overrides). If there's a reason not to, the docs should call this out explicitly.

---

### 5. Post-init tasks also don't get the service's rendered env

Same fundamental issue as #4 but for `post_init`. The `moto-init` script needed the moto URL to create AWS resources, but `post_init` tasks don't receive the service's rendered env vars (where `{{ services.moto.url }}` has been resolved).

This is even more surprising for `post_init` than `init`, because post_init runs *after* the service is ready — all URLs are known and resolved at that point.

**Impact:** Had to write `moto-init.py` with a manual discovery mechanism that reads devstack's manifest JSON from `~/.local/share/devstack/runs/*/manifest.json` to find the moto port at runtime.

**Suggestion:** Post-init tasks should receive the fully-rendered service env. They run after readiness — there's no reason to withhold resolved values.

---

### 6. `devstack logs --service X` returns 404 for services stuck in init

If a service is stuck in its init phase, `devstack logs --service call-analyzer` returns `404 Not Found: service call-analyzer not found`. The service doesn't exist in the run yet because it hasn't started, but its init task *is* running and producing output somewhere inaccessible.

**Impact:** No way to see why a service failed to start without reading devstack internals or using `ps aux`.

**Suggestion:** Either register services in the run before their init tasks start (in an `initializing` state), or provide a way to view init task output.

---

### 7. Service start is fully sequential with no parallelism for independent services

`launch_services` iterates `stack_plan.order` sequentially, calling `launch_service` (which blocks on readiness) for each one. Services with no dependency relationship still wait for each other.

In the augment-voice stack, `call-analyzer` and `voice-worker` both depend only on `voice-server`. They could start in parallel, but instead `call-analyzer` waits for `voice-worker` to be fully ready (or vice versa, depending on topo-sort order).

**Impact:** Adds unnecessary wall-clock time to stack startup, especially when init tasks (poetry install) are slow.

**Suggestion:** Start services in parallel when their deps are satisfied, rather than strictly sequential.

---

## Call Simulation / Voice Stack Issues

### 8. The `TestCallConfig` contract isn't documented

After a rebase that changed `TestCallConfig` from a nested shape to flat fields, there was no migration guide or documentation of what fields are required vs optional. I had to trace through `from_test_metadata()`, the voice-worker consumption path, and the agent initialization code to reconstruct the contract.

### 9. Env var naming is inconsistent across packages for the same infrastructure

- `CALL_QUEUE_URL` (voice_api_v2) = the call dispatch queue
- `VOICE_API_SQS_URL` (call_analyzer and voice_api_v2) = the call completion queue sent to the analyzer
- `CALL_ANALYZER_OUTPUT_SQS_URL` (both) = the analysis results queue

The names don't indicate direction, purpose, or which service reads vs writes. `VOICE_API_SQS_URL` in particular is confusing — it sounds like "the SQS URL for the voice API" but it's actually "the queue where voice API sends call completions for the analyzer to read."

### 10. The S3 bucket name for local dev was already defined but not surfaced

`augment-commons` already had `voice-service-v2-local` registered in `LIVEKIT_BUCKETS`, but nothing in the project pointed to this being the correct local bucket name. The devstack config used `augment-voice-local` (wrong), causing a runtime `ValueError: Unknown bucket` in the call analyzer. The only way to discover the right name was to hit the error, then inspect the installed package in the venv.

### 11. The call-analyzer's voice API client hardcodes staging/prod URLs with no override

`VoiceApiClient._get_voice_api_base_url()` returns staging or prod URLs based on `ENVIRONMENT`, with no env var override for local dev. I had to patch the source to add `VOICE_API_BASE_URL` / `DEV_URL_VOICE_SERVER` support. Any service-to-service URL should be overridable via env var for local development.

### 12. Call sim doesn't report *why* a scenario timed out

When scenarios hit the 180s max-duration timeout, the result just says "timeout" with no indication of what phase was stuck — still talking, waiting for a tool response, stuck in silence, etc. The transcript is there but there's no annotation of the timeout point or what the agent was doing when time ran out.

---

## The Recurring Theme

Devstack and the voice stack assume either cloud deployment or a very specific blessed local setup. They have implicit assumptions that break silently when you deviate from the happy path:

- Templates silently don't render in certain contexts
- Env vars silently come from the wrong source
- Services silently don't start and don't appear in status
- Bucket names silently don't match and only fail at runtime

The fix for almost every issue was "read the Rust source" or "read the Python source and add an env var override." Developer tools should not require reading their own implementation to use correctly.

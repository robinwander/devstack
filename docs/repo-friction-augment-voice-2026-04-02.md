# Repo-Level Friction — augment-voice monorepo

**Date:** 2026-04-02  
**Context:** Working end-to-end across voice_api_v2, call_analyzer, call_simulation, and augment-commons. Adding call-analyzer to devstack, fixing post-rebase breakage, verifying the full call pipeline.

This is separate from devstack tool friction — these are issues with the codebase, its conventions, and the developer experience of the repo itself.

---

## 1. `augment-commons` is `develop = true` in voice_api_v2 but `develop = false` in call_analyzer

This is the single most disorienting thing about the monorepo.

- `voice_api_v2/pyproject.toml`: `augment-commons = { path = "../augment-commons", develop = true }` — reads from source, changes are immediately visible.
- `call_analyzer/pyproject.toml`: `augment-commons = { path = "../augment-commons", develop = false }` — installs a built copy, changes require `poetry install` or `pip install --force-reinstall`.

When I edited `transcript_types.py` in augment-commons to add a bucket name, voice_api_v2 picked it up immediately but call_analyzer kept using the old version. I ran `poetry install` but it didn't update because the lockfile didn't detect a change. I had to `pip install --force-reinstall ../augment-commons` to make it stick.

This creates a world where the same source change is simultaneously live in one package and stale in another. When debugging cross-package issues, you can't trust that packages are seeing the same version of shared code.

**Suggestion:** Use `develop = true` everywhere, or have a documented workflow for "I changed commons, now what?"

---

## 2. No documentation of the SQS queue topology

There are 4+ SQS queues connecting 3 services, and nowhere in the repo documents which service reads from which queue:

```
CALL_QUEUE_URL          — voice-server writes, voice-server's CallConsumer reads (dispatch queue)
VOICE_API_SQS_URL       — voice-worker writes (on call completion), call-analyzer reads
CALL_ANALYZER_OUTPUT_SQS_URL — call-analyzer writes (analysis results), voice-server reads
VOICE_API_DLQ_SQS_URL   — DLQ for VOICE_API_SQS_URL
```

I had to trace through 4 source files across 2 packages to reconstruct this. The CLAUDE.md mentions "SQS → CallAnalysisConsumer processes transcripts" in one line but doesn't name the queues or explain the full flow.

**Suggestion:** A simple diagram in CLAUDE.md showing the queue topology, with env var names annotated.

---

## 3. The env var names don't describe what they do

- `VOICE_API_SQS_URL` — sounds like "the main SQS URL for the voice API." Actually: "the queue where voice-worker sends call completion events for the call analyzer to consume."
- `CALL_QUEUE_URL` — sounds generic. Actually: "the queue where the API server enqueues outbound call dispatch requests for the CallConsumer to process."
- `CALL_ANALYZER_OUTPUT_SQS_URL` — this one is actually clear.

When setting up moto-init, I had to create 5 SQS queues and map them to env vars. Getting the mapping wrong means messages flow into the void with no error. Clearer naming (e.g., `CALL_COMPLETION_SQS_URL` instead of `VOICE_API_SQS_URL`) would make this self-documenting.

---

## 4. 48 env vars in voice_api_v2 with no manifest of what's required vs optional

`voice_api_v2` calls `env.get()` or `env.get_optional()` for 48 distinct env vars. Some are required for the server to start, some are only used by specific agent workflows, some are only used in production. There's no single list that says "you need these 12 to run locally, these 8 are optional for dev, these 28 are production-only."

The `.env.template` exists but is sparse and doesn't distinguish required from optional. When something fails with `KeyError` on startup, you're guessing which env var to add.

**Suggestion:** The `.env.template` (or CLAUDE.md) should categorize env vars: required for local dev, optional with defaults, production-only.

---

## 5. Hardcoded environment-based URL resolution with no override escape hatch

Multiple places in the codebase determine service URLs based on `ENVIRONMENT`:

```python
# call_analyzer/voice_api_client.py
def _get_voice_api_base_url(self):
    if is_prod():
        return "https://voice-v2.prod.goaugment.com"
    elif is_dev_or_staging():
        return "https://voice-v2.staging.goaugment.com"
```

There's no env var to override this. When running locally with devstack, `ENVIRONMENT=dev` or `ENVIRONMENT=staging` both point at the remote staging server, not the local one. I had to patch the source code to add `VOICE_API_BASE_URL` / `DEV_URL_VOICE_SERVER` support.

This pattern appears elsewhere too — any service-to-service URL should be overridable via env var for local development, with the environment-based default as fallback.

---

## 6. `LIVEKIT_BUCKETS` is a hardcoded constant, not config

```python
# augment-commons/transcript_types.py
LIVEKIT_BUCKETS: Final[set[str]] = {"voice-api-v2-prod", "voice-api-v2-staging", "voice-service-v2-local"}
```

The S3 bucket name for local dev is embedded in a `Final` constant in augment-commons. If your moto bucket name doesn't match one of these, the call analyzer crashes with `ValueError: Unknown bucket`. You have to either:

1. Name your moto bucket to match the hardcoded value (what I ended up doing)
2. Edit the source code to add your bucket name

This should either read from `VOICE_SERVICE_V2_S3_BUCKET` env var as an additional recognized bucket, or the bucket-to-transcript-type mapping should be configurable.

---

## 7. The test-worker and call_simulation use voice_api_v2's .env

```toml
# devstack.toml
[stacks.voice.services.test-worker]
env_file = "../voice_api_v2/.env"

[stacks.voice.services.api]
env_file = "../voice_api_v2/.env"
```

The call simulation packages don't have their own complete `.env` — they reference `../voice_api_v2/.env` for LiveKit credentials, API keys, etc. This creates an implicit dependency: call_simulation only works if voice_api_v2's `.env` is set up correctly with keys that call_simulation also needs.

More importantly, there's no documentation of which keys in voice_api_v2's `.env` are consumed by call_simulation. When something fails in the test-worker, you don't know whether the missing env var should be in voice_api_v2's `.env` or somewhere else.

---

## 8. No integration test for the dispatch metadata contract

The `TestCallConfig` shape changed during a rebase (from nested to flat), breaking call simulation. There's no test that validates the round-trip:

```
call_simulation creates dispatch metadata 
  → voice_api_v2 parses it via TestCallConfig.from_test_metadata() 
  → agent initializes correctly
```

I wrote a smoke test inline during debugging, but there's no permanent test in CI that would catch this kind of contract break. The unit tests in each package test their own side of the contract, but nothing tests the interface between them.

---

## 9. The `config_overrides` field silently disappeared during refactoring

`TestCallConfig` had a `config_overrides: dict` field that call simulation used to pass runtime configuration to the voice agent. When the model was refactored to the flat shape, this field was dropped. No test caught it because:

- call_simulation tests mock the voice-worker side
- voice_api_v2 tests don't exercise the simulation dispatch path
- There's no cross-package integration test

The error only surfaced at runtime during a live call simulation: `AttributeError: 'TestCallConfig' object has no attribute 'config_overrides'`.

---

## 10. Git stashes as feature storage

The moto-init script for local AWS mocking was in `stash@{7}` out of 46 stashes. Not a branch, not a draft PR, not a docs reference — a stash with a one-line description. Finding it required `git stash list` and manually inspecting candidates.

Experimental features, partial implementations, and utility scripts shouldn't live in stashes. They're invisible, unsearchable, and one `git stash drop` from gone.

---

## 11. CLAUDE.md is good but missing the call-analyzer integration

The CLAUDE.md has solid architecture docs for voice_api_v2 — call flow, directory structure, common workflows. But it doesn't cover:

- The call-analyzer service or how it fits in the pipeline
- The SQS queue topology (which service writes/reads which queue)
- How to add call-analyzer to local dev
- The moto AWS mock setup

After this work, the full local stack is 7 services, but CLAUDE.md only documents the voice_api_v2 + call_simulation portion. Someone trying to work on the call-analyzer pipeline locally would hit the same walls I did.

---

## The pattern

Most of these issues share a root cause: **the repo evolved as separate packages that happen to live in the same git repo, not as an integrated system.** Each package has its own .env, its own dependency on augment-commons with different install modes, its own env var naming conventions, and its own assumptions about what's running.

The devstack config is the first thing that forces them to be treated as a single system — and that's where all the seams show.

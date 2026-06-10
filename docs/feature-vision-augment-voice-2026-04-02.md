# Devstack Feature Vision — What Would Make Me Fully Autonomous

**Date:** 2026-04-02
**Context:** After setting up the full augment-voice stack (7 services, moto AWS mock, MySQL, 3 Python packages), reflecting on what features would have the biggest compounding impact on daily work — not just fixes, but capabilities that change how I use the tool.

---

## 1. `devstack exec --service <name> -- <cmd>` — Run commands in a service's resolved env

**The problem I kept hitting:**
Every time I needed to debug something — run a migration manually, test a Python import, verify an API key — I had to reconstruct the service's environment by hand. The call-analyzer needed `AWS_ENDPOINT_URL`, `DB_URL`, `VOICE_API_SQS_URL`, etc., all with moto URLs that change every run. I'd end up doing `export AWS_ENDPOINT_URL=http://localhost:42011` by hand after reading `devstack status`.

`devstack exec` exists but only injects the base `DEV_*` vars. It doesn't give you a service's full merged env (env_file + service env overrides + rendered templates).

**What I want:**
```bash
# Run alembic in call-analyzer's full env (DB_URL, AWS_ENDPOINT_URL, etc. all resolved)
devstack exec --service call-analyzer -- poetry run alembic upgrade head

# Drop into a shell with voice-worker's env
devstack exec --service voice-worker -- bash

# Quick test: does this import work with the right env?
devstack exec --service call-analyzer -- python -c "from augment.call_analyzer.server import app"
```

This single feature would have saved me hours. Every debugging session started with "what env vars does this service actually see?" and ended with me manually piecing together values from `devstack status` output.

**Why it compounds:** Once this exists, init/post_init task env inheritance becomes less critical — you can always `devstack exec --service X -- <task-cmd>` as a workaround. It also makes ad-hoc debugging, migration runs, and one-off scripts trivial.

---

## 2. `devstack up --only <service>` — Start a subset of the stack

**The problem:**
When debugging the call-analyzer integration, I didn't need the sim UI, the sim API, or the test-worker. But `devstack up` is all-or-nothing — it starts every service in the stack. Starting 7 services takes 2-3 minutes. When I only needed moto + voice-server + call-analyzer, I was wasting a minute per cycle on services I didn't care about.

**What I want:**
```bash
# Start only these services (+ their transitive deps)
devstack up --only call-analyzer

# This would auto-start: moto → voice-server → call-analyzer
# And skip: voice-worker, test-worker, api, ui

# Start two specific subtrees
devstack up --only call-analyzer,voice-worker
```

**Why it compounds:** Faster iteration cycles. In a 7-service stack, you rarely need all 7 running. When adding a new service, you want to iterate on just that service + its deps until it works, then bring up the full stack.

---

## 3. Init/post_init tasks inherit service env — The foundation fix

**The problem (from friction report):**
Init tasks run with env from the task definition + env_file, but NOT the service's env overrides. This means if you override `DB_URL` in `[services.call-analyzer.env]`, the `migrate-analyzer` init task doesn't see it. This caused the alembic migration to retry for 10 minutes against a wrong DB.

Post_init tasks have the same gap — they can't access rendered template values like `{{ services.moto.url }}`, even though they run after the service (and all its deps) are ready.

**What I want:**
Init and post_init tasks inherit the service's fully merged env. The merge order should be:

1. Base `DEV_*` vars
2. `env_file` values
3. Service `env` overrides (with templates rendered for post_init; raw for init since deps may not be ready)
4. Task-specific `env` (if tasks ever get their own env section)

This is the single most impactful architectural fix. It eliminates the entire class of "my task sees different env than my service" bugs.

---

## 4. Startup progress streaming — See what's happening during `devstack up`

**The problem:**
`devstack up` blocks for minutes with zero output. When it hangs, you have no idea which service is stuck, which init task is running, or whether anything is happening at all. I had to `ps aux | grep alembic` to find a stuck migration.

**What I want:**
```
$ devstack up
▸ moto          init: (skipped, cached)
▸ moto          starting... ready (1.2s)
▸ voice-server  init: poetry-install-voice (skipped, cached)
▸ voice-server  init: migrate (running...)
▸ voice-server  init: migrate ✓ (3.1s)
▸ voice-server  starting... ready (4.8s)
▸ voice-server  post_init: moto-init (running...)
▸ voice-server  post_init: moto-init ✓ (1.5s)
▸ call-analyzer init: poetry-install-analyzer (skipped, cached)
▸ call-analyzer init: init-analyzer-db ✓ (0.3s)
▸ call-analyzer init: migrate-analyzer (running...)
```

Real-time, streaming to the terminal. If a task is stuck, I see it immediately. If a readiness check is failing, I see the retries.

**Why it compounds:** This makes `devstack up` self-diagnosing. You never need to context-switch to `ps aux` or `devstack logs` to figure out why startup is slow. For an AI agent, this is especially valuable — the agent can immediately identify and act on startup issues instead of timing out and guessing.

---

## 5. Template support for dashed service names

**The problem:**
`{{ services.voice-server.url }}` fails silently because the template engine parses `voice-server` as `voice minus server`. Devstack uses Minijinja, which doesn't support dashes in dotted access.

**What I want:**
Either normalize names in the template context (expose `voice_server` alongside `voice-server`) or support bracket notation. But honestly, the simplest fix that'd work: when building the Minijinja context, replace dashes with underscores in the keys, and document that `{{ services.voice_server.url }}` is the template form of a service named `voice-server`.

This is small but it bit me hard. The `{{ services.moto.url }}` pattern worked great for `moto`, but the moment I needed to reference `voice-server` from `call-analyzer`, I hit a wall.

---

## 6. `devstack env <service>` — Show resolved env for a service

**The problem:**
Debugging env var issues required reading the config, the .env file, understanding the merge order, and mentally resolving templates. There was no way to just ask "what does this service actually see?"

**What I want:**
```bash
$ devstack env call-analyzer
AWS_ENDPOINT_URL=http://localhost:42011
AWS_ACCESS_KEY_ID=test
AWS_SECRET_ACCESS_KEY=test
DB_URL=mysql+pymysql://call-analyzer-user:call-analyzer-password@localhost:3311/call-analyzer-db
VOICE_API_SQS_URL=http://localhost:42011/123456789012/augment-voice-api-sqs
DEV_URL_VOICE_SERVER=http://localhost:40051
ENVIRONMENT=dev
# ... all vars, fully resolved, in merge order

$ devstack env call-analyzer --diff
# Shows only vars that differ from the env_file defaults
# (i.e., the overrides from devstack.toml)
```

**Why it compounds:** Env var misconfiguration is the #1 class of startup failure in multi-service stacks. Being able to inspect the resolved env before starting means you catch issues before they become 5-minute retry loops.

---

## 7. Parallel service startup for independent services

**The problem:**
`launch_services` iterates the topo-sorted order sequentially. Services with no dependency relationship still wait for each other. In the augment-voice stack:

```
moto (3s) → voice-server (8s) → [voice-worker (5s), call-analyzer (10s)] → test-worker → api → ui
```

`voice-worker` and `call-analyzer` both depend only on `voice-server`, but one waits for the other. `test-worker` depends on `voice-worker`, not `call-analyzer`, but it waits for both.

**What I want:**
Start services in parallel when their deps are all ready. The topo sort gives you the levels — services at the same level in the DAG can start concurrently.

**Why it compounds:** In a 7-service stack with sequential startup, wall-clock time is the sum of all services. With parallel startup, it's the critical path. For augment-voice, that's roughly 90s → 45s. When you're iterating on config changes and restarting frequently, that halving matters.

---

## 8. First-class support for "seed data" services (moto, fixtures, etc.)

**The problem:**
Setting up moto required a service (moto_server), a post_init task (moto-init.py), and a script that does runtime discovery of the moto port because post_init tasks don't get rendered env. This pattern — "start a mock service, then seed it with data" — is universal but there's no first-class support for it.

**What I want:**
A `seed` or `setup` hook on services that runs after the service is ready AND receives the fully resolved env:

```toml
[stacks.voice.services.moto]
cmd = "moto_server -H 127.0.0.1 -p $PORT"
readiness = { http = { path = "/moto-api/" } }
setup = ["moto-init"]  # runs after ready, gets full env including {{ services.moto.url }}
```

Alternatively, if post_init just got the service's rendered env (feature #3), this would be solved already. But conceptually, "seed data after service ready" is common enough to deserve clear documentation and examples.

---

## 9. `devstack logs` with cross-service correlation

**The problem:**
When debugging the call-analyzer pipeline, I needed to follow a request across services: voice-worker enqueues a message → call-analyzer polls it → call-analyzer fetches from voice-server → call-analyzer runs LLM analysis. Each service has its own log stream. Correlating them required opening multiple `devstack logs --service X` windows and matching timestamps by eye.

**What I want:**
```bash
# Interleaved logs from all services, color-coded by service
devstack logs --all --follow

# Filter across all services by a shared identifier
devstack logs --all --search "01KN5Y13FDSWZT1C1NP797YJS4"

# This already works for indexed logs, but --follow + --all would be the game-changer
```

The log indexing is already excellent for after-the-fact investigation. What's missing is real-time cross-service tailing during active debugging.

---

## 10. Config validation beyond lint — `devstack check`

**The problem:**
`devstack lint` validates TOML syntax and basic structure. But it doesn't catch:
- Templates referencing services not in deps (will fail at runtime)
- `env_file` paths that don't exist
- Circular dependencies
- Port conflicts with hardcoded ports
- Init tasks referencing undefined task names (actually this might be caught)
- SQS URL env vars pointing at queues that moto-init doesn't create

**What I want:**
A deeper validation pass that catches config-level issues before you wait 3 minutes for `devstack up` to fail:

```bash
$ devstack check
⚠ call-analyzer: env references {{ services.voice-server.url }} but voice-server 
  is not in deps (will it be available?)
⚠ call-analyzer: env_file ".env" exists but DB_URL will be overridden by service env
✓ All init tasks resolve to defined [tasks]
✓ Dependency graph is acyclic
✓ No port conflicts
```

---

## Priority ranking

If I had to pick the 3 that would most change my daily experience:

1. **#4 — Startup progress streaming.** This is the difference between "devstack is a black box" and "I can see exactly what's happening." It makes every other issue faster to diagnose.

2. **#1 — `devstack exec --service`.** This eliminates the entire category of "let me reconstruct this service's env by hand." It's the debugging Swiss Army knife.

3. **#3 — Init/post_init inherit service env.** This is the foundation fix that eliminates the most common class of config bugs. It makes the env story coherent instead of surprising.

Everything else is valuable but these three would make the difference between "I need to understand devstack internals to use it" and "devstack just works."

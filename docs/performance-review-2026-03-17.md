# Devstack Performance Review

Date: 2026-03-17

## Scope

This review covered:

- `AGENTS.md`
- `README.md`
- `devstack-dash/README.md`
- `ARCHITECTURE.md`
- Core backend paths in `src/cli.rs`, `src/daemon.rs`, `src/log_index.rs`, `src/watch.rs`, `src/logs.rs`, `src/tasks.rs`, and related modules
- Dashboard query paths in `devstack-dash/src/lib/api.ts` and `devstack-dash/src/components/*.tsx`

I also ran the existing test suites:

```bash
/usr/bin/time -v cargo test
cd devstack-dash && /usr/bin/time -v pnpm test
```

Results:

- `cargo test`: `156/156` passed, `1.33s` wall, `250064 kB` max RSS
- `pnpm test`: `48/48` passed, `4.01s` wall, `204616 kB` max RSS

## System Model

`devstack` is a 3-process system:

1. CLI: parses commands, resolves config/project context, talks JSON/HTTP over Unix socket.
2. Daemon: owns run state, orchestration, health, periodic ingest, status building, and the Tantivy log index.
3. Shim: is the service `ExecStart` target and writes stdout/stderr as JSONL.

Important runtime paths:

- Startup/refresh: `cli -> orchestrate_up/orchestrate_refresh_run -> prepare_service -> compute_watch_hash -> start -> readiness`
- Service log path: `read_service_logs -> search_service -> ingest_sources -> Tantivy query`
- Run/source search path: `logs_search/source_logs -> search_run/facets_run`

## Representative Workloads And Commands

I used a synthetic JSONL corpus to exercise the real indexing/query code paths:

- 3 source files
- 150k total log lines
- 32 MB total

Commands:

```bash
/usr/bin/time -v env XDG_DATA_HOME=/tmp/devstack-perf/data \
  target/release/devstack sources add bench /tmp/devstack-perf/logs/*.jsonl

hyperfine --warmup 3 --runs 30 \
  --export-json /tmp/devstack-perf/query-hyperfine.json \
  'env XDG_DATA_HOME=/tmp/devstack-perf/data /home/dana/tools/devstack/target/release/devstack logs --source bench --search error --last 200 >/dev/null'

hyperfine --warmup 1 --runs 10 \
  --export-json /tmp/devstack-perf/ingest-hyperfine.json \
  "bash -lc 'tmp=\$(mktemp -d /tmp/devstack-ingest.XXXXXX); \
  XDG_DATA_HOME=\$tmp /home/dana/tools/devstack/target/release/devstack \
  sources add bench /tmp/devstack-perf/logs/*.jsonl >/dev/null; rm -rf \$tmp'"

strace -f -c bash -lc 'tmp=$(mktemp -d /tmp/devstack-strace.XXXXXX); \
  XDG_DATA_HOME=$tmp /home/dana/tools/devstack/target/release/devstack \
  sources add bench /tmp/devstack-prof-small/logs/*.jsonl >/dev/null'

bash -lc 'ulimit -n 1024; \
  env XDG_DATA_HOME=/tmp/devstack-massif-data.7bxJ7r \
  valgrind --tool=massif \
  --massif-out-file=/tmp/devstack-prof-small/ingest.massif \
  /home/dana/tools/devstack/target/release/devstack \
  sources add bench \
  /tmp/devstack-prof-small/logs/api.jsonl \
  /tmp/devstack-prof-small/logs/web.jsonl \
  /tmp/devstack-prof-small/logs/worker.jsonl >/dev/null'
```

Notes:

- `perf` was unavailable on this host because `perf_event_paranoid=4`.
- I used `strace`, `massif`, and static/callgrind-style inspection instead.

## Baseline Numbers

Warm source query:

- p50: `12.26 ms`
- p95: `13.77 ms`
- p99: `13.78 ms`
- mean: `12.28 ms`
- throughput: about `81 qps`
- peak memory for one warm query process: `18404 kB` max RSS

Cold source ingest/index build:

- p50: `3.495 s`
- p95: `3.704 s`
- p99: `3.704 s`
- mean: `3.509 s`
- throughput: about `42.7k lines/s`
- peak memory: `408088 kB` max RSS

## Profile Summary

### CPU / Query

Warm query cost is mostly inside Tantivy postings traversal and decompression, not obvious app-side glue. That matches the already-low warm query latency.

### Allocation / Heap

`massif` showed peak heap around `69.5 MB` on the reduced ingest profile. The largest allocators were inside `LogIndex::ingest_sources()`:

- `RawVec` growth for staging
- `Vec` materialization of pending docs
- `read_to_end`
- string cloning / parsed-line staging
- Tantivy commit-side allocations

### I/O / Syscalls

`strace -c` for ingest showed syscall time dominated by synchronization (`futex`) rather than storage latency. Actual file-related activity was mainly:

- `read`
- `openat`
- `newfstatat`
- `write`
- `fdatasync`

This points more toward CPU/allocation overhead than raw disk latency on the tested workload.

## Main Findings

### 1. `status` scales poorly with service count

The clearest backend inefficiency is in run status construction:

- [`recent_stderr_lines()`](/home/dana/tools/devstack/src/daemon.rs#L3733) does a full `search_service()` for one service.
- [`build_status()`](/home/dana/tools/devstack/src/daemon.rs#L3775) loops that across services.

This is an N-per-service query pattern. It is the best current candidate for a meaningfully better p95 on active dashboards.

Why it matters:

- every status poll can fan out into many log queries
- each per-service query does more work than the caller needs
- dashboard status polling happens every 2s

Isomorphic optimization shape:

- batch recent stderr retrieval once per run
- group top 3 stderr lines per service
- preserve the exact current per-service filter and ordering

Proof sketch:

- same `stream=stderr` filter
- same per-service cap of `3`
- same final sort order as current per-service tail path
- only retrieval strategy changes

### 2. `search_service()` and `search_run()` do repeated full query passes

Both [`search_service()`](/home/dana/tools/devstack/src/log_index.rs#L625) and [`search_run()`](/home/dana/tools/devstack/src/log_index.rs#L781) do several separate Tantivy searches:

- total scope count
- error count
- warn count
- matched total
- top docs fetch

That is acceptable for full UX responses, but it is wasteful for callers that only need lines.

Best low-risk split:

- keep the current full-stats path for existing external semantics
- add an internal tail-only path for status/recent-error callers

Proof sketch:

- preserve the same scope query
- preserve the same `ts_nanos` then `seq` ordering
- preserve the same `after` semantics for follow mode
- only skip counters for internal callers that never observe them

### 3. Run search/facets do useless source discovery work

Daemon run-level search/facets build and sort `sources`:

- [`logs_search()`](/home/dana/tools/devstack/src/daemon.rs#L954)
- [`logs_facets()`](/home/dana/tools/devstack/src/daemon.rs#L1021)

But the downstream functions ignore that argument:

- [`search_run()`](/home/dana/tools/devstack/src/log_index.rs#L781)
- [`facets_run()`](/home/dana/tools/devstack/src/log_index.rs#L942)

This is dead work:

- state access
- task-log path discovery
- allocation
- sorting

This is a clean zero-behavior-change removal candidate.

### 4. Refresh latency is strongly tied to `compute_watch_hash()`

[`prepare_service()`](/home/dana/tools/devstack/src/daemon.rs#L2706) calls [`compute_watch_hash()`](/home/dana/tools/devstack/src/watch.rs#L12) every time it prepares a service.

The gross cost is not the hash function itself. It is:

- whole-tree walk
- path materialization
- ordered set insertion
- path comparisons
- full file-content reads

The current implementation uses `BTreeSet<PathBuf>`, which adds unnecessary ordered-insert overhead while collecting files.

Isomorphic optimization shape:

- collect into `Vec<PathBuf>`
- `sort_unstable()`
- `dedup()`
- hash in the same final order

Proof sketch:

- if the final ordered unique path sequence is identical
- and `hash_path_metadata()` sees the same paths in the same order
- the final BLAKE3 output is identical

### 5. Ingest serialization is a real risk, but not the first “safe” lever

[`ingest_sources()`](/home/dana/tools/devstack/src/log_index.rs#L403) is serialized by a global ingest gate while it performs:

- file IO
- parsing
- dynamic field handling
- Tantivy writes
- commit
- reader reload
- cursor persistence

This can create tail-latency contention. But I would not make this the first change if the requirement is strict output isomorphism with minimal risk, because the simpler wins above are easier to reason about.

## Dashboard Query-Path Audit

This second pass found an important correction to the earlier mental model.

### Correction: the dashboard log viewer does not use `search_service()`

The dashboard log viewer polls:

- run view: [`api.searchRunLogs()`](/home/dana/tools/devstack/devstack-dash/src/lib/api.ts#L214)
- source view: [`api.searchSourceLogs()`](/home/dana/tools/devstack/devstack-dash/src/lib/api.ts#L267)

Those are driven from [`LogViewer`](/home/dana/tools/devstack/devstack-dash/src/components/log-viewer.tsx#L521), not from the service-log endpoint.

That means:

- the main dashboard log viewer is not paying per-request on-demand ingest
- instead, it depends on the daemon’s periodic ingest loops:
  - [`spawn_periodic_run_ingest()`](/home/dana/tools/devstack/src/daemon.rs#L4058)
  - [`spawn_periodic_source_ingest()`](/home/dana/tools/devstack/src/daemon.rs#L4105)

Practical consequence:

- dashboard log search polls every `1.5s`
- facets poll every `5s`
- but run/source search freshness is bounded by daemon ingest every `5s`

So the dashboard is currently over-polling the search endpoints relative to freshness. That is a responsiveness mismatch and a wasted-query issue.

This means my earlier concern about “dashboard polling mainly hammering `search_service()` and the ingest gate” was overstated. That concern applies more to status building and explicit service-log consumers, not to the main dashboard log-viewer search path.

### Source-view service filter bug

This looks like a real end-to-end bug.

Frontend:

- source search includes `service` in query params from [`LogViewer`](/home/dana/tools/devstack/devstack-dash/src/components/log-viewer.tsx#L501)
- the request goes through [`api.searchSourceLogs()`](/home/dana/tools/devstack/devstack-dash/src/lib/api.ts#L267)

Backend:

- [`source_logs()`](/home/dana/tools/devstack/src/daemon.rs#L1407) constructs a `LogSearchQuery`
- but it hardcodes `service: None` at [`src/daemon.rs:1441`](/home/dana/tools/devstack/src/daemon.rs#L1441)

Result:

- in source view, selecting a service tab or service facet does not actually constrain log rows on the daemon search path
- source facets do receive the service filter
- so rows and facets can diverge in source view

That was an omission in the first report and is likely the most obvious dashboard-path correctness issue I found on the second pass.

### Run/source search freshness mismatch

Dashboard poll intervals:

- navigation intent: `1s`
- run status: `2s`
- latest agent session: `2s`
- run/source logs search: `1.5s`
- facets: `5s`

Relevant frontend code:

- [`queries.navigationIntent`](/home/dana/tools/devstack/devstack-dash/src/lib/api.ts#L378)
- [`queries.runStatus`](/home/dana/tools/devstack/devstack-dash/src/lib/api.ts#L394)
- [`LogViewer` facets query](/home/dana/tools/devstack/devstack-dash/src/components/log-viewer.tsx#L473)
- [`LogViewer` logs query](/home/dana/tools/devstack/devstack-dash/src/components/log-viewer.tsx#L521)

Relevant backend freshness:

- run/source search indexes are updated periodically every `5s`

So:

- many `logs` polls cannot return fresher data than the last poll
- this wastes CPU on repeated search/count/doc-fetch work
- but reducing the poll interval would be a behavior change, not a pure isomorphic optimization

I would treat this as an architectural responsiveness mismatch, not as the first no-risk optimization.

## Revised Opportunity Matrix

Scored as `(Impact x Confidence) / Effort`.

1. Batch `status` recent-error lookup
   - Impact: high
   - Confidence: high
   - Effort: medium
   - Best “safe” backend lever

2. Remove dead source-discovery/setup work from run search/facets
   - Impact: medium
   - Confidence: very high
   - Effort: low
   - Pure cleanup with zero semantic change

3. Add an internal tail-only log path for status/recent-error callers
   - Impact: high
   - Confidence: high
   - Effort: medium
   - Very strong isomorphic candidate

4. Replace `BTreeSet` collection in watch hashing with sort/dedup vector
   - Impact: medium
   - Confidence: medium-high
   - Effort: low-medium
   - Strong unchanged-refresh win

5. Fix source-view service filtering
   - Impact: correctness high, performance low-medium
   - Confidence: high
   - Effort: low
   - Not isomorphic, because it corrects wrong current behavior

6. Revisit dashboard poll cadence versus 5s ingest cadence
   - Impact: medium
   - Confidence: high
   - Effort: low-medium
   - Not isomorphic, because it changes freshness/UX

## Regression Oracles / Guardrails

For the isomorphic candidates above, the oracle should assert:

- same returned lines / entries
- same ordering
- same counters
- same facet values and sort order
- same watch hash for identical trees
- same restart/no-restart decision on unchanged refresh

Suggested guardrails:

- benchmark `status` on `1`, `10`, and `20` services
- benchmark unchanged `up` on a fixed watched tree
- benchmark warm run/source search and facets on a fixed corpus
- keep golden fixtures for:
  - source logs with multiple service identities
  - task-prefixed services
  - ties on `ts_nanos` resolved by `seq`

## Bottom Line

After the second pass, the most important corrections are:

1. The dashboard log viewer’s main cost center is not the on-demand service-log path. Its bigger issue is polling search endpoints every `1.5s` while the underlying index only refreshes every `5s`.
2. Source-view service filtering is currently broken on the daemon path.
3. The best strictly isomorphic backend performance work still looks like:
   - batching status recent-error retrieval
   - removing dead source setup in run search/facets
   - splitting tail-only internal log retrieval from full-count query paths
   - reducing watch-hash collection overhead without changing final hash order

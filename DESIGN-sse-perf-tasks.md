# Design: SSE Event Stream, Log Performance, Task Args

Three parallel improvements to devstack's UI and CLI. Each is independent and touches different parts of the codebase.

## Context

The devstack dashboard currently polls the daemon's REST API for all state. This causes:

1. **Stale status after CLI commands** — When `devstack down` then `devstack up` runs, the dashboard doesn't pick up the state change for seconds. The root cause is both a missing push mechanism AND a bug in the dashboard's auto-select logic (see §1.1).
2. **Slow log loading** — The combined entries+facets query runs every 1.5s, forcing server-side ingestion + search + facet aggregation on every poll. Most polls return the same 500 entries.
3. **No way to pass flags to tasks** — `devstack run migrate` runs a fixed command string. Users must create separate task entries for each flag combination.

## 1. SSE Event Stream

Replace polling with Server-Sent Events as the primary data channel between daemon and dashboard. No polling fallback — SSE is the transport.

### 1.1 Auto-Select Bug (fix independently of SSE)

In `devstack-dash/src/components/dashboard.tsx` ~line 73, the auto-select effect checks:

```ts
const selectionStillValid =
  selectedRunId !== null && runs.some((r) => r.run_id === selectedRunId)
```

This checks if the run **exists**, not whether it's **active**. A stopped run still exists in the runs list, so the dashboard stays pinned to it even when a new active run appears. The user has to manually switch.

**Fix:** Treat a stopped run the same as a missing one for auto-select purposes:

```ts
const selectionStillValid =
  selectedRunId !== null &&
  runs.some((r) => r.run_id === selectedRunId && r.state !== 'stopped')
```

When the current run stops and an active run exists, auto-switch to it.

### 1.2 Daemon: Broadcast Channel

Add a `broadcast::Sender<DaemonEvent>` to `AppState`. Events are lightweight — just enough for the dashboard to know what to refetch.

Event types (sent as SSE named events):

| SSE event name | Payload | Fired when |
|---|---|---|
| `run` | `{ kind: "created" \| "state_changed" \| "removed", run_id, state?, stack?, project_dir? }` | `orchestrate_up`, `orchestrate_down`, `orchestrate_kill`, `recompute_run_state`, GC |
| `service` | `{ kind: "state_changed", run_id, service, state }` | `mark_service_ready`, `mark_service_failed`, health monitor transitions, restart |
| `global` | `{ kind: "state_changed", key, state }` | Global lifecycle changes |
| `log` | `{ run_id, service, ts, stream, level, message, raw, attributes? }` | New log line written (see §1.4) |

Channel capacity ~1024 with `broadcast::channel`. Slow subscribers that fall behind get `RecvError::Lagged` — they should do a full refresh on reconnect anyway.

### 1.3 Daemon: SSE Endpoint

`GET /v1/events` — returns `text/event-stream`.

Accepts optional query param `?run_id=X` to subscribe to log streaming for that run. Without it, the client gets only state events (run/service/global lifecycle).

The handler subscribes to the broadcast channel and streams events. When the client disconnects, cleanup happens naturally (the subscriber drops).

Vite dev proxy (`devstack-dash/vite.config.ts`) needs to pass through SSE responses without buffering — the current `unix-socket-proxy` plugin reads the full response body before sending it. It needs to stream for the `/v1/events` path.

### 1.4 Daemon: Log Tailing

When at least one SSE subscriber requests logs for a run (via `?run_id=X`), the daemon tails that run's log files:

- Use `notify` (already a dependency, used for auto-restart file watching) to watch the run's logs directory
- On file change, read new bytes from a tracked offset — same incremental pattern as `LogIndex` ingestion in `src/log_index.rs`
- Parse JSONL lines, broadcast as `log` events on the channel
- Stop tailing when the last subscriber for that run disconnects

This means log entries flow: shim writes JSONL → `notify` detects change → daemon reads new bytes → parses → broadcasts → SSE streams to dashboard. No polling involved.

### 1.5 Dashboard: `useEventStream` Hook

Single hook that manages the `EventSource` lifecycle. Used by the Dashboard component.

```ts
function useEventStream(activeRunId: string | null): {
  connected: boolean
  logs: LogEntry[]
  clearLogs: () => void
}
```

- Opens `EventSource` to `/api/v1/events?run_id=<activeRunId>`
- When `activeRunId` changes, closes old connection, opens new one
- On state events → `queryClient.invalidateQueries()` for relevant keys
- On log events → appends to local `LogEntry[]` buffer (capped at ~5000, drop oldest)
- On disconnect → show reconnecting banner. `EventSource` auto-reconnects. On reconnect, invalidate all queries to catch up.

### 1.6 Dashboard: Remove All Polling

Drop every `refetchInterval` from `devstack-dash/src/lib/api.ts`. Queries fetch on mount + on SSE-triggered invalidation only.

The `queries` object in `api.ts` currently has polling intervals on every query (runs: 3s, status: 2s, logs: 1.5s, etc). All of these go away. The SSE connection is the sole trigger for data freshness.

Exception: facets query keeps a 10s `refetchInterval` (see §2.1).

### 1.7 Event → Query Key Mapping

| SSE event | Invalidated query keys |
|---|---|
| `run:created`, `run:state_changed`, `run:removed` | `queryKeys.runs`, `queryKeys.runStatus(run_id)` |
| `service:state_changed` | `queryKeys.runStatus(run_id)` |
| `global:state_changed` | `queryKeys.globals` |
| `log` | (not invalidated — appended directly to local buffer) |

## 2. Log Performance

### 2.1 Split Entries and Facets

Currently one combined `runLogView` query fetches `include_entries=true, include_facets=true` every 1.5s. This forces server-side ingestion + Tantivy search + facet aggregation on every poll.

**After:** Two separate queries:

- **Facets query**: Uses the existing `LogViewResponse` endpoint with `include_entries=false, include_facets=true`. Polls every 10s (the one remaining polling interval). Always fetched — not lazy loaded.
- **Entries**: Come from SSE in live mode, or from a one-shot HTTP fetch in search mode (see §2.2).

### 2.2 Live Mode vs Search Mode

The log viewer already has a `timeRange` state: `live`, `5m`, `15m`, `1h`, `custom`.

**Live mode** (`timeRange === 'live'`, no search query): Log entries come directly from the SSE `log` events. Client-side filtering by service/level/stream tab. No HTTP polling for entries.

**Search mode** (user typed a search query, or selected a non-live time range): One-shot HTTP fetch to `LogViewResponse` endpoint with `include_entries=true, include_facets=false`. Server does Tantivy search. Re-fetch when filter params change, not on a timer. SSE log events still flow into the buffer but the displayed view shows search results.

This maps cleanly to existing UI state — the `timeRange` toggle already exists in the log viewer toolbar.

### 2.3 Log Entry Buffer Management

The `useEventStream` hook manages a `LogEntry[]` buffer:

- Capped at ~5000 entries (configurable). Drop oldest when exceeded.
- Cleared when switching runs or reconnecting.
- In live mode, the LogViewer reads directly from this buffer.
- In search mode, the LogViewer reads from the HTTP query result instead.

## 3. Task Trailing Args

### 3.1 CLI Change

Add trailing args to the `Run` command variant in `src/cli.rs`:

```rust
Run {
    name: Option<String>,
    // ... existing fields ...
    #[arg(last = true, allow_hyphen_values = true)]
    args: Vec<String>,
}
```

`last = true` means everything after `--` is captured as positional args.

### 3.2 Task Runner Change

In `src/tasks.rs`, `run_task()` receives the trailing args and appends them to the command string (shell-escaped). The args also need to flow through `run_task_command_cli()` in `src/cli.rs` and `run_init_tasks()` (init tasks don't get trailing args — they run automatically).

### 3.3 Usage

```bash
devstack run migrate -- --skip-seed
devstack run test -- --watch --filter="auth"
devstack run build -- --release --target x86_64
```

No config changes needed. No daemon changes.

## Key Files

### Daemon (Rust)
- `src/daemon.rs` — AppState, orchestration functions, SSE endpoint, all mutation sites that need to fire events
- `src/api.rs` — API types (DaemonEvent type goes here or in a new events module)
- `src/log_index.rs` — Incremental JSONL ingestion pattern to reuse for log tailing
- `src/shim.rs` — No changes needed (shim writes JSONL to files, daemon tails files)
- `src/tasks.rs` — Task runner, needs trailing args parameter
- `src/cli.rs` — CLI arg parsing, `run_task_command_cli`, Run command variant
- `src/manifest.rs` — ServiceState, RunLifecycle enums (referenced by event types)

### Dashboard (TypeScript/React)
- `devstack-dash/src/lib/api.ts` — Query definitions, remove all refetchInterval, add facets-only query
- `devstack-dash/src/components/dashboard.tsx` — Auto-select bug fix, useEventStream integration
- `devstack-dash/src/components/log-viewer.tsx` — Live mode vs search mode split, read from SSE buffer vs HTTP
- `devstack-dash/vite.config.ts` — SSE proxy streaming support

### Architecture docs
- `ARCHITECTURE.md` — Update to reflect SSE event stream
- `API.md` — Document `/v1/events` endpoint
- `README.md` — Update if needed

## Design Decisions

1. **SSE over WebSocket** — Unidirectional (server → client) is all we need. Simpler implementation, automatic reconnection built into the EventSource API, works through reverse proxies.

2. **No polling fallback** — This is a personal tool. If SSE disconnects, show a reconnecting banner and let EventSource auto-reconnect. No parallel polling system.

3. **Log entries via SSE, not just signals** — Direct streaming avoids an extra HTTP round-trip per update. The daemon already has JSONL parsing machinery. The marginal complexity of tailing log files is small compared to the latency improvement.

4. **Facets still poll at 10s** — Facet counts change gradually as logs accumulate. There's no per-entry event that maps to "facets changed." A 10s refresh is fine and avoids coupling facets to the SSE stream.

5. **Client-side filtering in live mode** — SSE streams all entries for the subscribed run. Service/level/stream filtering happens client-side. This avoids needing per-filter SSE subscriptions. For search queries, the server does the filtering via Tantivy.

6. **Auto-select fix is independent** — The dashboard bug where stopped runs stay selected is a one-line fix that should ship immediately, regardless of SSE work.

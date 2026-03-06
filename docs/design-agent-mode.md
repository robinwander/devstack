# Design: Agent Mode (`devstack agent`)

## Overview

A transparent PTY proxy that lets devstack mediate between a coding agent and the developer, enabling collaborative debugging without copy-paste. The agent gets the same terminal TUI experience as running directly. Devstack sits invisibly in the middle, enabling two-way context sharing.

## UX

### User → Agent (Share)

User clicks "Share" on a log view in the dashboard. Devstack reconstructs the **query** that produced the view and injects it into the agent's terminal input:

```
Can you take a look at this? Run `devstack logs --service api --search "connection refused" --last 10`
```

Note: CLI flags, API params, and dashboard URL params all use identical names (`--search`, `--last`, `--level`, etc.). See `docs/cli-redesign.md` for the full vocabulary.

The agent runs the query, gets live results, and can modify it — widen the time range, change the search term, pivot to a different service. The query is the message, not a static log dump.

### Agent → User (Show)

Agent calls `devstack show` to navigate the dashboard:

```bash
devstack show --service api --search "connection refused"
devstack show --service api --tail
```

Daemon stores a navigation intent. Dashboard picks it up on next poll, navigates to that view, clears the intent.

## CLI Surface

```bash
devstack agent [flags] -- <command> [args...]
```

Examples:

```bash
devstack agent -- claude "Fix the crash in the API"
devstack agent -- pi "Debug the worker service"
devstack agent --auto-share warn -- claude "Keep this running"
```

### Flags

| Flag | Description |
|------|-------------|
| `--auto-share <level>` | Auto-inject when services log at this level or above (default: `error`) |
| `--watch <services>` | Auto-share when specific services have issues |
| `--run <id>` | Attach to a specific run (default: resolve from cwd) |

## Architecture

```
┌─────────────┐     ┌─────────────────────────┐     ┌─────────────┐
│ Your terminal│◄───►│ devstack agent           │◄───►│ agent CLI   │
│              │     │  - PTY proxy (invisible) │     │ (in PTY)    │
│              │     │  - injection listener    │     └─────────────┘
│              │     │  - crash watcher         │
└─────────────┘     └────────────┬────────────┘
                                 │ register/inject
                    ┌────────────▼────────────┐     ┌─────────────┐
                    │ devstack daemon          │◄───►│  dashboard  │
                    │  - agent session registry│     │  - share btn│
                    │  - navigation intents    │     │  (sends qry)│
                    └─────────────────────────┘     └─────────────┘
```

### PTY Proxy

The `devstack agent` CLI process:

1. Allocates a PTY via `pty-process` crate
2. Spawns the agent command attached to the PTY
3. Puts the user's terminal in raw mode
4. Forwards all I/O bidirectionally (user terminal ↔ PTY)
5. Handles SIGWINCH → PTY resize
6. Registers with daemon as an agent session
7. Listens on a Unix socket for injection messages from daemon
8. On exit: restores terminal, unregisters with daemon, exits with agent's exit code

The user sees the exact same TUI as running the agent directly. Devstack is invisible.

### Injection Mechanism

When the daemon needs to inject a message (share button, auto-share-errors):

1. Dashboard sends share request to daemon API
2. Daemon looks up agent session → finds Unix socket path
3. Daemon sends formatted message to the socket
4. `devstack agent` CLI reads from socket, writes to PTY stdin
5. Message is sent as **bracketed paste** (`\x1b[200~...\x1b[201~`) so the agent CLI treats it as pasted text, not keystrokes

Bracketed paste means the message appears in the agent's input buffer without auto-executing. The user can review, edit, and submit.

### Agent Session Registry

On startup, `devstack agent` registers with the daemon:

```
POST /v1/agent/sessions
{
  "agent_id": "a7f3b2c1-...",       // generated UUID
  "project_dir": "/home/dana/myapp",
  "stack": "default",
  "command": "claude",
  "inject_socket": "/tmp/devstack-agent-a7f3b2c1.sock",
  "pid": 12345
}
```

On shutdown (or crash cleanup): session is removed. Daemon periodically prunes stale sessions (check PID liveness, socket reachability).

Multiple concurrent sessions are supported in the data model. Share routing defaults to **most recent session** — no picker UI needed initially.

### Environment Variables

Set in the PTY environment (inherited by agent and all subprocesses):

| Variable | Purpose |
|----------|---------|
| `DEVSTACK_AGENT_ID` | Unique session ID for message routing |

Run resolution continues to use the existing directory-based lookup (`find_nearest_path` walks up to `devstack.toml`). The agent ID is purely for the injection channel.

### Navigation Intents

The dashboard should support URL query params for view state:

```
http://localhost:3000/?run=abc123&service=api&search=connection+refused&level=error
```

This makes dashboard views shareable/bookmarkable independently of agent mode, and simplifies navigation intents — the agent just constructs a URL.

Agent calls `devstack show` CLI → hits daemon API:

```
POST /v1/runs/{run_id}/navigate
{
  "url": "/?run=abc123&service=api&search=connection+refused&level=error"
}
```

Daemon stores as a pending navigation on the run. Dashboard polls, sees intent, applies the URL params to React state (selected run, active service tab, search/filter state), clears via:

```
DELETE /v1/runs/{run_id}/navigate
```

#### Dashboard URL Params

| Param | Maps to | Example |
|-------|---------|---------|
| `run` | `selectedRunId` | `run=abc123` |
| `service` | `activeTab` (service name) | `service=api` |
| `search` | `searchInput` | `search=connection+refused` |
| `level` | `levelFilter` | `level=error` |
| `stream` | `streamFilter` | `stream=stderr` |
| `since` | `timeRange` | `since=5m` |
| `last` | line count | `last=100` |

Same names everywhere — CLI flags, API params, URL params. See `docs/cli-redesign.md` for the full specification.

### Share Button (Dashboard → Daemon)

Dashboard reconstructs the current view state as a devstack CLI query. The filter state maps 1:1 to CLI flags because both derive from the same API params.

```
POST /v1/agent/share
{
  "agent_id": null,              // null = most recent session
  "command": "devstack logs --service api --search \"connection refused\" --level error --last 10",
  "message": "Can you look at this?"   // optional user-added context
}
```

Daemon sends to agent via injection socket. The agent receives a runnable query, not a static log dump — it can execute, modify, widen the search, pivot to a different service.

### Auto-Share

When `--auto-share <level>` is active (default: `error`), the `devstack agent` process monitors the run's log stream. When a service emits a log at or above the specified level:

```
[devstack] Service 'api' logged an error.
Run `devstack logs --service api --level error --last 20` to see what happened.
```

Injected via the same bracketed paste mechanism. The level threshold controls sensitivity:
- `error` (default) — only crashes and errors
- `warn` — warnings and above
- Omit the flag entirely to disable auto-sharing

## Signal Handling

| Signal | Behavior |
|--------|----------|
| SIGINT | In raw mode, ctrl+C is byte `0x03` — passes through to agent naturally |
| SIGWINCH | Caught by wrapper, forwarded as PTY resize |
| SIGTERM | Graceful shutdown: restore terminal, kill agent, unregister, exit |
| Panic | Panic hook restores terminal state before unwinding |

## Single Source of Truth for Filters

Log filter parameters currently exist in 4 places: Rust API struct, CLI arg building, hand-maintained TypeScript interface, hardcoded dashboard toggle buttons. Adding a filter means touching all of them. This must be unified.

### Approach

**The API describes the filters. Everything else derives from that.**

#### 1. Data-driven facets response

The facets endpoint returns filter metadata alongside values. The dashboard renders filters programmatically — no hardcoded `type LevelFilter = "all" | "warn" | "error"`.

```json
{
  "total": 473,
  "filters": [
    {
      "field": "service",
      "kind": "select",
      "values": [
        { "value": "api", "count": 300 },
        { "value": "worker", "count": 173 }
      ]
    },
    {
      "field": "level",
      "kind": "toggle",
      "values": [
        { "value": "info", "count": 400 },
        { "value": "warn", "count": 50 },
        { "value": "error", "count": 23 }
      ]
    },
    {
      "field": "stream",
      "kind": "toggle",
      "values": [
        { "value": "stdout", "count": 450 },
        { "value": "stderr", "count": 23 }
      ]
    }
  ]
}
```

Dashboard iterates `filters`, renders a toggle group per entry. Styling uses heuristics for well-known values (error=red, warn=amber) but renders unknown values with neutral styling. Adding a new indexed field to the log schema → it appears in facets automatically → dashboard renders a new filter group with zero TS changes.

#### 2. Rust struct as API source of truth

`LogSearchQuery` drives API deserialization (serde) and the OpenAPI spec (utoipa). The CLI builds the same struct from args and calls `.to_query_string()` — eliminating all manual `params.push()` chains in `cli.rs`.

#### 3. TypeScript codegen

Generate dashboard types from the OpenAPI spec (`/v1/openapi.json`) using `openapi-typescript`. No hand-maintained `LogFilterParams` interface.

#### 4. Dashboard URL params

Read/write URL search params using the same field names as API params. React state initializes from URL on load, syncs back on change. Navigation intents and share queries are just URL param sets — same field names, same types, same source.

#### Adding a new filter

1. Add field to Rust log indexing + `LogSearchQuery` struct
2. Done — OpenAPI updates, CLI picks it up, facets endpoint returns it, dashboard renders it, URL params work, share/navigation use it

## Dependencies

- `pty-process` (with `async` feature) — PTY allocation and process spawning
- `crossterm` or `termion` — raw mode, terminal size detection
- Existing tokio runtime for async I/O forwarding

## Implementation Phases

### Phase 1: Transparent Proxy
- PTY spawn + raw mode + I/O forwarding + resize
- Agent session registration/cleanup with daemon
- Injection socket listener
- `devstack agent -- <command>` CLI
- Exit code passthrough, terminal restoration, signal handling

### Phase 2: Dashboard URL Params + Navigation
- Dashboard URL query param support (run, service, q, level, stream, since)
- `devstack show` CLI + navigation intent API
- Dashboard polls for and applies navigation intents

### Phase 3: Share Button
- "Share with agent" button on log lines / service panels in dashboard
- Reconstructs current view as a devstack CLI query
- Daemon routes to agent session, injects via bracketed paste

### Phase 4: Auto-Share
- `--auto-share <level>` log monitoring
- `--watch <services>` selective monitoring
- Formatted context injection with query commands

# Devstack Daemon HTTP API (local Unix socket)

This daemon exposes a small JSON API over HTTP/1.1 on a Unix domain socket.
The CLI (`devstack`) talks to the daemon using these endpoints.

## Socket location

The socket lives under the devstack base directory:

- macOS: `$HOME/Library/Application Support/devstack/daemon/devstackd.sock`
- Linux: `$HOME/.local/share/devstack/daemon/devstackd.sock`

If you are unsure, run `devstack doctor` or inspect `paths::daemon_socket_path()`.

## Quick test

```sh
SOCKET="$HOME/Library/Application Support/devstack/daemon/devstackd.sock"
# Linux example:
# SOCKET="$HOME/.local/share/devstack/daemon/devstackd.sock"

curl --unix-socket "$SOCKET" http://localhost/v1/ping
```

## Conventions

- Content type: `application/json`
- Errors are JSON objects: `{ "error": "message" }`
- Status codes: `400` for bad request, `404` for not found, `500` for internal errors
- Timestamps are RFC3339 strings

---

# Endpoints

## GET /v1/ping

Health check.

Response: `PingResponse`

Example response:

```json
{ "ok": true }
```

---

## POST /v1/runs/up

Create or refresh a run.

Request: `UpRequest`

Response: `RunManifest`

Notes:
- If `new_run` is `false` and `run_id` is omitted, the daemon reuses the latest run for the same `(project_dir, stack)` and refreshes it.
- `force=true` forces restarts even if no config/watch changes are detected.
- `no_wait=true` skips readiness waits.

---

## POST /v1/runs/down

Stop a run (optionally purge artifacts).

Request: `DownRequest`

Response: `RunManifest`

---

## POST /v1/runs/kill

Hard-stop a run.

Request: `KillRequest`

Response: `RunManifest`

---

## POST /v1/runs/{run_id}/restart-service

Restart a single service inside a run.

Request: `RestartServiceRequest`

Response: `RunManifest`

---

## GET /v1/runs/{run_id}/status

Return live status for a run.

Response: `RunStatusResponse`

---

## GET /v1/runs

List known runs.

Response: `RunListResponse`

---

## GET /v1/globals

List global services (one-per-project globals). These are read from disk manifests.

Response: `GlobalsResponse`

---

## POST /v1/gc

Garbage-collect stopped runs (and optionally globals).

Request: `GcRequest`

Response: `GcResponse`

Notes:
- `older_than` is a `humantime` duration string (e.g. `"72h"`, `"7d"`).
- Default cutoff is 7 days.
- `all=true` also removes stopped globals past the cutoff.

---

# Data types

## Requests

### UpRequest

```json
{
  "stack": "string",
  "project_dir": "string",
  "run_id": "string | null",
  "file": "string | null",
  "no_wait": false,
  "new_run": false,
  "force": false
}
```

### DownRequest

```json
{ "run_id": "string", "purge": false }
```

### KillRequest

```json
{ "run_id": "string" }
```

### RestartServiceRequest

```json
{ "service": "string", "no_wait": false }
```

### GcRequest

```json
{ "older_than": "string | null", "all": false }
```

---

## Responses

### PingResponse

```json
{ "ok": true }
```

### RunListResponse

```json
{
  "runs": [RunSummary]
}
```

### RunSummary

```json
{
  "run_id": "string",
  "stack": "string",
  "project_dir": "string",
  "state": "starting | running | degraded | stopped",
  "created_at": "rfc3339",
  "stopped_at": "rfc3339 | null"
}
```

### RunManifest

```json
{
  "run_id": "string",
  "project_dir": "string",
  "stack": "string",
  "manifest_path": "string",
  "services": {
    "<service>": ServiceManifest
  },
  "env": { "KEY": "value" },
  "state": "starting | running | degraded | stopped",
  "created_at": "rfc3339",
  "stopped_at": "rfc3339 | null"
}
```

### ServiceManifest

```json
{
  "port": 5432,
  "url": "http://... | null",
  "state": "starting | ready | degraded | stopped | failed",
  "watch_hash": "string | null"
}
```

### RunStatusResponse

```json
{
  "run_id": "string",
  "stack": "string",
  "project_dir": "string",
  "state": "starting | running | degraded | stopped",
  "services": {
    "<service>": ServiceStatus
  }
}
```

### ServiceStatus

```json
{
  "desired": "running | stopped",
  "systemd": SystemdStatus | null,
  "ready": true,
  "state": "starting | ready | degraded | stopped | failed",
  "last_failure": "string | null",
  "url": "http://... | null"
}
```

### SystemdStatus

```json
{
  "active_state": "string",
  "sub_state": "string",
  "result": "string | null"
}
```

### GlobalsResponse

```json
{
  "globals": [GlobalSummary]
}
```

### GlobalSummary

```json
{
  "key": "string",
  "name": "string",
  "project_dir": "string",
  "state": "starting | running | degraded | stopped",
  "port": 5432,
  "url": "http://... | null"
}
```

### GcResponse

```json
{
  "removed_runs": ["run-id"],
  "removed_globals": ["global-key"]
}
```

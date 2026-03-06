# CLI Redesign

## `devstack show`

The `show` command mirrors the `logs` command's filter flags (same names, same values) but instead of querying logs, it navigates the dashboard to that view.

### Flags

| Flag | Type | Description |
|------|------|-------------|
| `--service` | `String` | Filter to a specific service |
| `--search` | `String` | Search query string |
| `--level` | `String` | Log level filter (e.g. `warn`, `error`) |
| `--stream` | `String` | Stream filter (`stdout` or `stderr`) |
| `--since` | `String` | Time-based filter (e.g. `5m`, `1h`) |
| `--last` | `usize` | Show last N log lines |
| `--run` | `String` | Target a specific run |

### Behavior

1. CLI sends a navigation intent to the daemon via `POST /v1/navigation/intent`
2. The daemon stores the intent (replacing any existing one)
3. The dashboard polls `GET /v1/navigation/intent` for pending intents
4. When an intent is found, the dashboard applies the filters to the URL params and log viewer state
5. After consuming, the dashboard calls `DELETE /v1/navigation/intent` to clear it
6. The `show` command opens the dashboard UI if not already open

### Example

```bash
devstack show --service api --level error --since 5m
```

This navigates the dashboard to show only error-level logs from the `api` service in the last 5 minutes.

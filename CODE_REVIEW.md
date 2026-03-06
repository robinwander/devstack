# Devstack code quality review (src/)

I reviewed all files in `src/` and ran:
- `cargo test -q` (110 tests passed)
- `cargo clippy -q --all-targets --all-features` (no output)

Below are issues that could make coworkers hesitate to adopt/contribute.

## 🔴 Must fix

1) **Run ID path traversal / unsafe filesystem writes**  
- `RunId::new` accepts arbitrary strings with no validation (`src/ids.rs:10-13`).
- Paths are formed by direct join (`src/paths.rs:37-38`).
- User input flows directly to run dir creation (`src/daemon.rs:1125-1134`).
- Stack names are also unvalidated and feed run-id generation (`src/config.rs:272-278`, `src/daemon.rs:2077-2083`).

**Why this matters:** `--run-id ../...` (or malicious stack names) can escape `runs/` and write outside expected boundaries.

2) **Daemon restart loses critical service runtime metadata; restart behavior becomes incorrect**  
- Persisted service manifest stores only `{port,url,state,watch_hash}`; `watch_hash` is even skipped on serialize (`src/manifest.rs:30-35`).
- On daemon boot, runtime is reconstructed with placeholder defaults (`scheme="http"`, `deps=[]`, `readiness=None`) (`src/daemon.rs:1003-1039`).
- `restart-service` then uses this reconstructed readiness/scheme (`src/daemon.rs:2517-2581`).

**Why this matters:** after daemon restart, readiness checks can be silently wrong (often effectively skipped), making status/restart semantics unreliable.

3) **Source names are not validated for URL/path safety, but are used as route segments**  
- Only empty-name check exists (`src/sources.rs:66-69`).
- CLI builds paths with raw source names (no URL encoding) (`src/cli.rs:976`, `src/cli.rs:2582-2585`).
- Server routes use `/v1/sources/{name}` and `/v1/sources/{name}/logs` (`src/daemon.rs:884`, `src/daemon.rs:902-905`).

**Why this matters:** names containing `/`, `?`, `%`, spaces, etc. can be persisted but not reliably addressed/removed/queried.

4) **`watch_hash` persistence bug causes unnecessary service restarts after daemon restart**  
- `watch_hash` is not serialized (`src/manifest.rs:34`).
- Refresh logic restarts when hash mismatches (`src/daemon.rs:1430-1433`).

**Why this matters:** first `devstack up` after daemon restart can restart unchanged services unnecessarily.

## 🟡 Should fix

1) **Periodic source ingestion silently swallows failures**  
- Errors from spawned ingest task are ignored (`src/daemon.rs:2939-2949`).

2) **Daemon liveness check is socket-exists only (stale socket false-positive)**  
- `daemon_is_running` checks only file existence (`src/cli.rs:2526-2529`).

3) **Inconsistent source log semantics with/without daemon**  
- Daemon source logs flatten entries to raw lines (service lost) (`src/daemon.rs:944-946`).
- CLI daemon path reconstructs entries with `service=source_name` (`src/cli.rs:984-988`).
- Local fallback (`search_run`) preserves per-file service identities.

4) **Task watch hash stored from pre-run state**  
- Hash computed before running task and then persisted (`src/tasks.rs:215`, `src/tasks.rs:231`).

**Impact:** tasks that mutate watched files can trigger extra reruns.

5) **Watch hashing uses metadata only (size+mtime), not file contents**  
- `hash_path_metadata` includes path, len, mtime only (`src/watch.rs:109-118`).

**Impact:** false negatives are possible on coarse timestamp filesystems / same-size rewrites.

6) **Unix socket path length edge-case is not proactively handled**  
- Socket path is derived from `BaseDirs` (`src/paths.rs:9-23`), then bound directly (`src/daemon.rs:231-234`).

**Impact:** on macOS/Linux long home/data paths can hit AF_UNIX path limits with opaque bind errors.

7) **Linux behavior assumes systemd user bus always available**  
- Daemon requires `RealSystemd::connect()` on Linux startup (`src/daemon.rs:200-202`, `src/systemd.rs:82-84`).
- Installer hard-requires `systemctl --user` (`src/cli.rs:2225-2241`).

**Impact:** poor UX on non-systemd Linux setups (WSL/minimal containers/disabled user bus).

8) **Unsafe pre-exec in shim ignores `setpgid` failure**  
- `setpgid` return code is discarded (`src/shim.rs:33-37`).

9) **Blocking filesystem work under async state lock during GC**  
- `remove_dir_all` runs while holding `state.state.lock()` (`src/daemon.rs:2967-2989`, `src/daemon.rs:2995-3019`).

10) **Dead/unused API surface (`LogsQuery.regex`)**  
- Field exists in API (`src/api.rs:172`) but is not consumed in log query execution.

## 🟢 Nice to have

1) **Refactor giant modules for contributor ergonomics**  
- `cli.rs` central dispatch + business logic in one file (`src/cli.rs:299+`).
- `daemon.rs` mixes routing, orchestration, persistence, health, GC, diagnostics (`src/daemon.rs:197+`, `src/daemon.rs:1003+`, `src/daemon.rs:2933+`).

2) **Improve user-facing message consistency**  
- Example: dashboard hint references dev-repo-specific script path (`src/cli.rs:2491-2493`).

3) **Test coverage gaps on critical behavior**  
- No tests in `src/systemd.rs`, `src/projects.rs`, `src/diagnose.rs` (especially around process-group signaling, project ledger edge cases, diagnose heuristics).
- Add regression tests for: run-id validation, source-name validation/encoding, daemon restart state reconstruction.

4) **Dependency hygiene**  
- `hyperlocal` appears unused in `src/` (`Cargo.toml:19`; no source references found).

---

## Overall
The codebase is functional and test suite is green, but there are a few correctness/security/runtime-state issues (notably run-id/path handling and restart state reconstruction) that should be resolved before sharing widely. The biggest adoption blocker is reliability around daemon restarts and edge-case input validation.
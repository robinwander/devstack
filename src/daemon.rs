use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::Infallible;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant as StdInstant, SystemTime};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{delete, get, post},
};
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use notify::{EventKind, RecursiveMode, Watcher};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::net::UnixStream;
use tokio::sync::{Mutex, broadcast};
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::{
    AddSourceRequest, AddSourceResponse, AgentSession, AgentSessionMessageRequest,
    AgentSessionMessageResponse, AgentSessionPollResponse, AgentSessionRegisterRequest,
    DaemonEvent, DaemonGlobalEvent, DaemonGlobalEventKind, DaemonLogEvent, DaemonRunEvent,
    DaemonRunEventKind, DaemonServiceEvent, DaemonServiceEventKind, DaemonTaskEvent,
    DaemonTaskEventKind, DoctorCheck, DoctorResponse, DownRequest, GcRequest, GcResponse,
    GlobalSummary, GlobalsResponse, HealthCheckStats, HealthStatus, KillRequest,
    LatestAgentSessionQuery, LatestAgentSessionResponse, LogViewQuery, LogViewResponse, LogsQuery,
    LogsResponse, NavigationIntent, NavigationIntentResponse, PingResponse, ProjectsResponse,
    RecentErrorLine, RegisterProjectRequest, RegisterProjectResponse, RestartServiceRequest,
    RunListResponse, RunStatusResponse, RunSummary, RunWatchResponse, ServiceStatus,
    SetNavigationIntentRequest, ShareAgentMessageRequest, ShareAgentMessageResponse, SourceSummary,
    SourcesResponse, StartTaskRequest, StartTaskResponse, SystemdStatus, TaskExecutionState,
    TaskExecutionSummary, TaskStatusResponse, TasksResponse, UpRequest, WatchControlRequest,
    WatchServiceStatus,
};
use crate::config::{ConfigFile, ServiceConfig, StackPlan, TaskConfig};
use crate::ids::{RunId, ServiceName};
use crate::log_index::{LogIndex, LogSource};
use crate::logfmt::{classify_line_level, extract_log_content, extract_timestamp_str};
use crate::manifest::{RunLifecycle, RunManifest, ServiceManifest, ServiceState};
use crate::paths;
use crate::port::allocate_ports;
use crate::projects::ProjectsLedger;
use crate::readiness::{ReadinessContext, ReadinessKind, ReadinessSpec, readiness_url};
use crate::sources::{SourcesLedger, source_run_id};
use crate::systemd::{ExecStart, SystemdManager, UnitProperties};
use crate::util::{atomic_write, expand_home, now_rfc3339, sanitize_env_key};
use crate::watch::compute_watch_hash;

#[cfg(not(target_os = "linux"))]
use crate::systemd::LocalSystemd;
#[cfg(target_os = "linux")]
use crate::systemd::RealSystemd;
#[cfg(target_os = "linux")]
use sd_notify::notify;

#[derive(Clone)]
pub(crate) struct AppState {
    systemd: Arc<dyn SystemdManager>,
    state: Arc<Mutex<DaemonState>>,
    binary_path: PathBuf,
    log_index: Arc<LogIndex>,
    event_tx: broadcast::Sender<DaemonEvent>,
    log_tails: Arc<Mutex<RunLogTailRegistry>>,
    _lock: Arc<std::fs::File>,
}

type AppResult<T> = Result<T, AppError>;

#[derive(Default)]
struct DaemonState {
    runs: BTreeMap<String, RunState>,
    detached_tasks: BTreeMap<String, DetachedTaskExecution>,
    agent_sessions: BTreeMap<String, AgentSessionState>,
    navigation_intent: Option<NavigationIntent>,
}

#[derive(Default)]
struct RunLogTailRegistry {
    runs: HashMap<String, RunLogTailHandle>,
}

struct RunLogTailHandle {
    subscribers: usize,
    task: tokio::task::JoinHandle<()>,
}

struct LogTailCursor {
    offset: u64,
}

struct LogTailSubscription {
    state: AppState,
    run_id: Option<String>,
}

impl Drop for LogTailSubscription {
    fn drop(&mut self) {
        let Some(run_id) = self.run_id.take() else {
            return;
        };
        let state = self.state.clone();
        tokio::spawn(async move {
            release_run_log_tail(&state, &run_id).await;
        });
    }
}

struct AgentSessionState {
    agent_id: String,
    project_dir: String,
    stack: Option<String>,
    command: String,
    pid: u32,
    created_at: String,
    pending_messages: VecDeque<String>,
}

#[derive(Serialize)]
struct DaemonStateFile {
    runs: Vec<String>,
    updated_at: String,
}

struct RunState {
    run_id: String,
    stack: String,
    project_dir: PathBuf,
    base_env: BTreeMap<String, String>,
    services: BTreeMap<String, ServiceRuntime>,
    state: RunLifecycle,
    created_at: String,
    stopped_at: Option<String>,
}

#[derive(Clone)]
struct DetachedTaskExecution {
    execution_id: String,
    task: String,
    project_dir: PathBuf,
    run_id: Option<String>,
    state: TaskExecutionState,
    started_at: String,
    started_at_instant: StdInstant,
    finished_at: Option<String>,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
}

struct ServiceRuntime {
    name: String,
    unit_name: String,
    port: Option<u16>,
    scheme: String,
    url: Option<String>,
    deps: Vec<String>,
    readiness: ReadinessSpec,
    log_path: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    state: ServiceState,
    last_failure: Option<String>,
    health: Option<HealthHandle>,
    last_started_at: Option<String>,
    watch_hash: Option<String>,
    watch_patterns: Vec<String>,
    ignore_patterns: Vec<String>,
    watch_extra_files: Vec<PathBuf>,
    watch_fingerprint: Vec<u8>,
    auto_restart: bool,
    watch_paused: bool,
    watch_handle: Option<ServiceWatchHandle>,
}

struct PreparedService {
    name: String,
    unit_name: String,
    port: Option<u16>,
    scheme: String,
    url: Option<String>,
    deps: Vec<String>,
    readiness: ReadinessSpec,
    log_path: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    cmd: String,
    watch_hash: String,
    watch_patterns: Vec<String>,
    ignore_patterns: Vec<String>,
    watch_extra_files: Vec<PathBuf>,
    watch_fingerprint: Vec<u8>,
    auto_restart: bool,
}

#[derive(Clone)]
struct ServiceWatchHandle {
    stop_flag: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

type WatchStartArgs = (
    PathBuf,
    Vec<String>,
    Vec<String>,
    Vec<PathBuf>,
    Vec<u8>,
    bool,
);

/// Per-service health monitor handle.  Lives in an `Arc` so the background
/// monitor task and the status endpoint can both reach the stats without
/// going through the global `DaemonState` mutex.
#[derive(Clone)]
struct HealthHandle {
    stop_flag: Arc<AtomicBool>,
    stats: Arc<std::sync::Mutex<HealthSnapshot>>,
}

/// Counters updated by the health-monitor task.  Protected by its own
/// lightweight `std::sync::Mutex` (held for nanoseconds) — never behind
/// the global async `DaemonState` mutex.
#[derive(Clone, Default)]
struct HealthSnapshot {
    passes: u64,
    failures: u64,
    consecutive_failures: u32,
    last_check_at: Option<String>,
    last_ok: Option<bool>,
}

fn emit_event(state: &AppState, event: DaemonEvent) {
    let _ = state.event_tx.send(event);
}

fn emit_events(state: &AppState, events: Vec<DaemonEvent>) {
    for event in events {
        emit_event(state, event);
    }
}

fn run_created_event(run: &RunState) -> DaemonEvent {
    DaemonEvent::Run(DaemonRunEvent {
        kind: DaemonRunEventKind::Created,
        run_id: run.run_id.clone(),
        state: Some(run.state.clone()),
        stack: Some(run.stack.clone()),
        project_dir: Some(run.project_dir.to_string_lossy().to_string()),
    })
}

fn run_state_changed_event(run: &RunState) -> DaemonEvent {
    DaemonEvent::Run(DaemonRunEvent {
        kind: DaemonRunEventKind::StateChanged,
        run_id: run.run_id.clone(),
        state: Some(run.state.clone()),
        stack: None,
        project_dir: None,
    })
}

fn run_removed_event(run_id: impl Into<String>) -> DaemonEvent {
    DaemonEvent::Run(DaemonRunEvent {
        kind: DaemonRunEventKind::Removed,
        run_id: run_id.into(),
        state: None,
        stack: None,
        project_dir: None,
    })
}

fn service_state_changed_event(run_id: &str, service: &str, state: ServiceState) -> DaemonEvent {
    DaemonEvent::Service(DaemonServiceEvent {
        kind: DaemonServiceEventKind::StateChanged,
        run_id: run_id.to_string(),
        service: service.to_string(),
        state,
    })
}

fn global_state_changed_event(key: &str, state: RunLifecycle) -> DaemonEvent {
    DaemonEvent::Global(DaemonGlobalEvent {
        kind: DaemonGlobalEventKind::StateChanged,
        key: key.to_string(),
        state,
    })
}

fn task_event(task: &DetachedTaskExecution, kind: DaemonTaskEventKind) -> DaemonEvent {
    DaemonEvent::Task(DaemonTaskEvent {
        kind,
        execution_id: task.execution_id.clone(),
        task: task.task.clone(),
        run_id: task.run_id.clone(),
        state: task.state.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
        exit_code: task.exit_code,
        duration_ms: task.duration_ms,
    })
}

fn task_summary_from_history(execution: &crate::tasks::TaskExecution) -> TaskExecutionSummary {
    TaskExecutionSummary {
        task: execution.task.clone(),
        execution_id: None,
        state: if execution.exit_code == 0 {
            TaskExecutionState::Completed
        } else {
            TaskExecutionState::Failed
        },
        started_at: execution.started_at.clone(),
        finished_at: Some(execution.finished_at.clone()),
        exit_code: Some(execution.exit_code),
        duration_ms: Some(execution.duration_ms),
    }
}

fn task_summary_from_detached(task: &DetachedTaskExecution) -> TaskExecutionSummary {
    TaskExecutionSummary {
        task: task.task.clone(),
        execution_id: Some(task.execution_id.clone()),
        state: task.state.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
        exit_code: task.exit_code,
        duration_ms: Some(task_duration_ms(task)),
    }
}

fn task_status_response(task: &DetachedTaskExecution) -> TaskStatusResponse {
    TaskStatusResponse {
        execution_id: task.execution_id.clone(),
        task: task.task.clone(),
        state: task.state.clone(),
        project_dir: task.project_dir.to_string_lossy().to_string(),
        run_id: task.run_id.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
        exit_code: task.exit_code,
        duration_ms: task_duration_ms(task),
    }
}

fn task_duration_ms(task: &DetachedTaskExecution) -> u64 {
    task.duration_ms.unwrap_or_else(|| {
        task.started_at_instant
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    })
}

fn detached_task_is_newer(
    candidate: &TaskExecutionSummary,
    current: &TaskExecutionSummary,
) -> bool {
    match candidate.started_at.cmp(&current.started_at) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match candidate.finished_at.cmp(&current.finished_at) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => {
                candidate.execution_id.is_some() && current.execution_id.is_none()
            }
        },
    }
}

fn merge_task_summary(
    tasks: &mut BTreeMap<String, TaskExecutionSummary>,
    summary: TaskExecutionSummary,
) {
    match tasks.entry(summary.task.clone()) {
        std::collections::btree_map::Entry::Vacant(slot) => {
            slot.insert(summary);
        }
        std::collections::btree_map::Entry::Occupied(mut slot) => {
            if detached_task_is_newer(&summary, slot.get()) {
                slot.insert(summary);
            }
        }
    }
}

fn set_service_state(
    run_id: &str,
    service: &str,
    svc: &mut ServiceRuntime,
    state: ServiceState,
) -> Option<DaemonEvent> {
    if svc.state == state {
        return None;
    }
    svc.state = state.clone();
    Some(service_state_changed_event(run_id, service, state))
}

fn recompute_run_state(run: &mut RunState) -> Option<DaemonEvent> {
    if matches!(run.state, RunLifecycle::Stopped) {
        return None;
    }
    let previous = run.state.clone();
    let mut all_ready = true;
    let mut any_degraded = false;
    for svc in run.services.values() {
        match svc.state {
            ServiceState::Ready => {}
            ServiceState::Starting => {
                all_ready = false;
            }
            ServiceState::Degraded | ServiceState::Failed => {
                any_degraded = true;
                all_ready = false;
            }
            ServiceState::Stopped => {
                all_ready = false;
            }
        }
    }
    run.state = if any_degraded {
        RunLifecycle::Degraded
    } else if all_ready {
        RunLifecycle::Running
    } else {
        RunLifecycle::Starting
    };
    (run.state != previous).then(|| run_state_changed_event(run))
}

async fn retain_run_log_tail(state: &AppState, run_id: &str) -> Result<()> {
    let mut registry = state.log_tails.lock().await;
    if let Some(handle) = registry.runs.get_mut(run_id) {
        handle.subscribers += 1;
        return Ok(());
    }

    let run_id_owned = run_id.to_string();
    let task_state = state.clone();
    let task_run_id = run_id_owned.clone();
    let task = tokio::spawn(async move {
        if let Err(err) = tail_run_logs(task_state, task_run_id.clone()).await {
            eprintln!("devstack: log tail failed for {task_run_id}: {err}");
        }
    });

    registry.runs.insert(
        run_id_owned,
        RunLogTailHandle {
            subscribers: 1,
            task,
        },
    );
    Ok(())
}

async fn release_run_log_tail(state: &AppState, run_id: &str) {
    let handle = {
        let mut registry = state.log_tails.lock().await;
        let Some(entry) = registry.runs.get_mut(run_id) else {
            return;
        };
        if entry.subscribers > 1 {
            entry.subscribers -= 1;
            return;
        }
        registry.runs.remove(run_id)
    };

    if let Some(handle) = handle {
        handle.task.abort();
    }
}

async fn tail_run_logs(state: AppState, run_id: String) -> Result<()> {
    let logs_dir = paths::run_logs_dir(&RunId::new(run_id.clone()))?;
    std::fs::create_dir_all(&logs_dir)?;
    tail_run_logs_in_dir(state, run_id, logs_dir).await
}

async fn tail_run_logs_in_dir(state: AppState, run_id: String, logs_dir: PathBuf) -> Result<()> {
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = notify_tx.send(event);
    })
    .context("create log tail watcher")?;
    watcher
        .watch(&logs_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watch log directory {}", logs_dir.to_string_lossy()))?;

    let mut cursors = initial_log_tail_cursors(&logs_dir)?;
    let _watcher = watcher;

    while let Some(event) = notify_rx.recv().await {
        match event {
            Ok(event) => match event.kind {
                EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) => {
                    for path in event.paths {
                        if !is_tailed_log_path(&path) {
                            continue;
                        }
                        if !path.exists() {
                            cursors.remove(&path);
                            continue;
                        }
                        let cursor = cursors
                            .entry(path.clone())
                            .or_insert(LogTailCursor { offset: 0 });
                        if let Ok(events) = read_new_log_events(&run_id, &path, cursor) {
                            for event in events {
                                emit_event(&state, DaemonEvent::Log(event));
                            }
                        }
                    }
                }
                EventKind::Remove(_) => {
                    for path in event.paths {
                        cursors.remove(&path);
                    }
                }
                _ => {}
            },
            Err(err) => eprintln!("devstack: log tail watcher error for {run_id}: {err}"),
        }
    }

    Ok(())
}

fn initial_log_tail_cursors(logs_dir: &Path) -> Result<HashMap<PathBuf, LogTailCursor>> {
    let mut cursors = HashMap::new();
    if !logs_dir.exists() {
        return Ok(cursors);
    }

    for entry in std::fs::read_dir(logs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !is_tailed_log_path(&path) {
            continue;
        }
        let offset = std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        cursors.insert(path, LogTailCursor { offset });
    }

    Ok(cursors)
}

fn is_tailed_log_path(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("log")
}

fn read_new_log_events(
    run_id: &str,
    path: &Path,
    cursor: &mut LogTailCursor,
) -> Result<Vec<DaemonLogEvent>> {
    let file_len = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if file_len < cursor.offset {
        cursor.offset = 0;
    }

    let mut file = File::open(path).with_context(|| format!("open log {}", path.display()))?;
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    if buf.is_empty() {
        return Ok(Vec::new());
    }

    let Some(last_newline) = buf.iter().rposition(|byte| *byte == b'\n') else {
        return Ok(Vec::new());
    };

    let complete_len = last_newline + 1;
    let complete = &buf[..complete_len];
    cursor.offset = cursor.offset.saturating_add(complete_len as u64);

    let service = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("invalid log path {}", path.display()))?;

    let mut events = Vec::new();
    for raw_line in String::from_utf8_lossy(complete).lines() {
        if let Some(event) = parse_log_tail_event(run_id, &service, raw_line) {
            events.push(event);
        }
    }

    Ok(events)
}

fn parse_log_tail_event(run_id: &str, service: &str, raw_line: &str) -> Option<DaemonLogEvent> {
    let raw = crate::util::strip_ansi_if_needed(raw_line.trim_end_matches(['\r', '\n']));
    if raw.is_empty() {
        return None;
    }

    let ts = extract_timestamp_str(&raw).unwrap_or_default();
    let (stream, message) = extract_log_content(&raw);
    Some(DaemonLogEvent {
        run_id: run_id.to_string(),
        service: service.to_string(),
        ts,
        stream,
        level: classify_line_level(&raw),
        message,
        raw: raw.clone(),
        attributes: extract_log_attributes(&raw),
    })
}

fn extract_log_attributes(line: &str) -> BTreeMap<String, String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return BTreeMap::new();
    }

    let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(trimmed) else {
        return BTreeMap::new();
    };

    let mut attributes = BTreeMap::new();
    for (name, value) in map {
        let Some(name) = normalize_log_attribute_name(&name) else {
            continue;
        };
        if is_reserved_log_attribute(&name) {
            continue;
        }
        let Some(value) = log_attribute_value_to_string(&value) else {
            continue;
        };
        attributes.entry(name).or_insert(value);
    }
    attributes
}

fn normalize_log_attribute_name(field_name: &str) -> Option<String> {
    let mut normalized = String::with_capacity(field_name.len());
    let mut last_was_underscore = false;

    for ch in field_name.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_underscore = false;
        } else if !last_was_underscore {
            normalized.push('_');
            last_was_underscore = true;
        }
    }

    let normalized = normalized.trim_matches('_');
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn is_reserved_log_attribute(field_name: &str) -> bool {
    matches!(
        field_name,
        "time"
            | "ts"
            | "timestamp"
            | "msg"
            | "message"
            | "level"
            | "severity"
            | "stream"
            | "run_id"
            | "service"
            | "ts_nanos"
            | "seq"
            | "raw"
    )
}

fn log_attribute_value_to_string(value: &JsonValue) -> Option<String> {
    let value = match value {
        JsonValue::String(value) => value.clone(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) | JsonValue::Null => return None,
    };

    if value.is_empty() || value.chars().count() > 256 {
        None
    } else {
        Some(value)
    }
}

fn acquire_daemon_lock() -> Result<Arc<std::fs::File>> {
    let lock_path = paths::daemon_lock_path()?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open daemon lock at {lock_path:?}"))?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == ErrorKind::WouldBlock {
            return Err(anyhow!("daemon already running (lock held)"));
        }
        return Err(err).context("lock daemon file");
    }
    Ok(Arc::new(file))
}

async fn ping_existing_daemon(stream: UnixStream) -> Result<bool> {
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("handshake with existing daemon")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("GET")
        .uri("http://localhost/v1/ping")
        .body(Full::new(hyper::body::Bytes::new()))?;
    let response = sender
        .send_request(req)
        .await
        .context("send ping to existing daemon")?;
    Ok(response.status().is_success())
}

async fn clear_stale_socket(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }
    match UnixStream::connect(socket_path).await {
        Ok(stream) => {
            if ping_existing_daemon(stream).await? {
                return Err(anyhow!("daemon already running (socket active)"));
            }
            Err(anyhow!(
                "daemon socket present but unresponsive; stop the existing daemon and try again"
            ))
        }
        Err(err) => match err.kind() {
            ErrorKind::ConnectionRefused | ErrorKind::NotFound => {
                let _ = std::fs::remove_file(socket_path);
                Ok(())
            }
            _ => Err(err).context("connect to existing daemon socket"),
        },
    }
}

pub async fn run_daemon() -> Result<()> {
    paths::ensure_base_layout()?;
    let lock = acquire_daemon_lock()?;
    #[cfg(target_os = "linux")]
    let systemd = Arc::new(RealSystemd::connect().await?);
    #[cfg(not(target_os = "linux"))]
    let systemd = Arc::new(LocalSystemd::new());
    let binary_path = std::env::current_exe().context("current_exe")?;
    let state = Arc::new(Mutex::new(load_state_from_disk()?));
    let log_index = Arc::new(LogIndex::open_or_create()?);
    let (event_tx, _) = broadcast::channel(1024);

    let app_state = AppState {
        systemd,
        state,
        binary_path,
        log_index,
        event_tx,
        log_tails: Arc::new(Mutex::new(RunLogTailRegistry::default())),
        _lock: lock,
    };

    // Seed projects ledger from existing runs
    if let Ok(runs_dir) = paths::runs_dir()
        && let Ok(mut ledger) = ProjectsLedger::load()
        && let Ok(count) = ledger.seed_from_runs(&runs_dir)
        && count > 0
    {
        eprintln!("[projects] seeded {} projects from existing runs", count);
    }

    // Spawn periodic GC to evict stopped runs from memory and the log index,
    // preventing unbounded memory growth over long daemon lifetimes.
    spawn_periodic_gc(app_state.clone());
    spawn_periodic_ingest(app_state.clone());
    spawn_periodic_compaction(app_state.clone());
    spawn_periodic_agent_session_cleanup(app_state.clone());

    write_daemon_state(&app_state).await.ok();

    let socket_path = paths::daemon_socket_path()?;
    clear_stale_socket(&socket_path).await?;
    let listener = tokio::net::UnixListener::bind(&socket_path)
        .with_context(|| format!("bind socket {socket_path:?}"))?;

    let app = Router::new()
        .route("/v1/ping", get(ping))
        .route("/v1/agent/sessions", post(register_agent_session))
        .route(
            "/v1/agent/sessions/{agent_id}",
            axum::routing::delete(unregister_agent_session),
        )
        .route(
            "/v1/agent/sessions/{agent_id}/messages",
            post(post_agent_message),
        )
        .route(
            "/v1/agent/sessions/{agent_id}/messages/poll",
            get(poll_agent_messages),
        )
        .route("/v1/agent/sessions/latest", get(get_latest_agent_session))
        .route("/v1/agent/share", post(share_agent_message))
        .route("/v1/events", get(events))
        .route("/v1/runs/up", post(up))
        .route("/v1/runs/down", post(down))
        .route("/v1/runs/kill", post(kill))
        .route("/v1/runs/{run_id}/restart-service", post(restart_service))
        .route("/v1/runs/{run_id}/status", get(status))
        .route("/v1/tasks/run", post(start_task))
        .route("/v1/tasks/{execution_id}", get(task_status))
        .route("/v1/runs/{run_id}/tasks", get(run_tasks))
        .route("/v1/runs/{run_id}/watch", get(watch_status))
        .route("/v1/runs/{run_id}/watch/pause", post(watch_pause))
        .route("/v1/runs/{run_id}/watch/resume", post(watch_resume))
        .route("/v1/runs/{run_id}/logs/{service}", get(logs))
        .route("/v1/runs/{run_id}/logs", get(logs_view))
        .route("/v1/runs", get(list_runs))
        .route("/v1/globals", get(list_globals))
        .route("/v1/projects", get(list_projects))
        .route("/v1/projects/register", post(register_project))
        .route("/v1/projects/{project_id}", delete(remove_project))
        .route("/v1/sources", get(list_sources).post(add_source))
        .route("/v1/sources/{name}", delete(remove_source))
        .route("/v1/sources/{name}/logs", get(source_logs_view))
        .route(
            "/v1/navigation/intent",
            get(get_navigation_intent)
                .post(set_navigation_intent)
                .delete(clear_navigation_intent),
        )
        .route("/v1/gc", post(gc))
        .with_state(app_state);

    // Spawn dashboard process if installed
    let dashboard_handle = spawn_dashboard().await;

    #[cfg(target_os = "linux")]
    let _ = notify(false, &[sd_notify::NotifyState::Ready]);

    axum::serve(listener, app).await?;

    // Clean up dashboard on exit
    if let Some(mut child) = dashboard_handle {
        let _ = child.kill().await;
    }

    Ok(())
}

const DASHBOARD_PORT: u16 = 47832;

async fn spawn_dashboard() -> Option<tokio::process::Child> {
    let dashboard_dir = match paths::dashboard_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("[dashboard] failed to get dashboard dir: {}", e);
            return None;
        }
    };

    // Check if dashboard is installed (has package.json)
    if !dashboard_dir.join("package.json").exists() {
        eprintln!(
            "[dashboard] no package.json found at {:?}, skipping",
            dashboard_dir
        );
        return None;
    }

    // Find pnpm or npm - check common paths directly since systemd/launchd have limited PATH
    let home = std::env::var("HOME").unwrap_or_default();
    let pnpm_paths = [
        format!("{home}/.local/share/pnpm/pnpm"),
        "/opt/homebrew/bin/pnpm".to_string(),
        "/usr/local/bin/pnpm".to_string(),
        "/usr/bin/pnpm".to_string(),
    ];
    let npm_paths = [
        format!("{home}/.local/share/fnm/aliases/default/bin/npm"),
        format!("{home}/.nvm/current/bin/npm"),
        "/opt/homebrew/bin/npm".to_string(),
        "/usr/local/bin/npm".to_string(),
        "/usr/bin/npm".to_string(),
    ];

    let (cmd, is_pnpm) = pnpm_paths
        .iter()
        .find(|p| Path::new(p.as_str()).exists())
        .map(|p| (p.clone(), true))
        .or_else(|| {
            npm_paths
                .iter()
                .find(|p| Path::new(p.as_str()).exists())
                .map(|p| (p.clone(), false))
        })
        .unwrap_or_else(|| {
            eprintln!("[dashboard] pnpm/npm not found in common paths, skipping dashboard");
            (String::new(), false)
        });

    if cmd.is_empty() {
        return None;
    }

    let port_str = DASHBOARD_PORT.to_string();
    let args: Vec<&str> = if is_pnpm {
        vec!["dev", "--port", &port_str, "--host", "0.0.0.0"]
    } else {
        vec!["run", "dev", "--", "--port", &port_str, "--host", "0.0.0.0"]
    };

    eprintln!(
        "[dashboard] starting with {} {:?} in {:?}",
        cmd, args, dashboard_dir
    );

    // Create log file for dashboard output
    let log_path = match paths::daemon_dir() {
        Ok(dir) => dir.join("dashboard.log"),
        Err(_) => dashboard_dir.join("dashboard.log"),
    };

    let log_file = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[dashboard] failed to create log file: {}", e);
            return None;
        }
    };
    let log_file_err = match log_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[dashboard] failed to clone log file: {}", e);
            return None;
        }
    };

    // Set PATH to include common node locations since systemd/launchd have minimal PATH
    let path_env = format!(
        "{home}/.local/share/pnpm:{home}/.local/share/fnm/aliases/default/bin:{home}/.nvm/current/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
    );

    match tokio::process::Command::new(&cmd)
        .args(&args)
        .current_dir(&dashboard_dir)
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err))
        .spawn()
    {
        Ok(child) => {
            eprintln!(
                "[dashboard] started on http://localhost:{} (log: {:?})",
                DASHBOARD_PORT, log_path
            );
            Some(child)
        }
        Err(err) => {
            eprintln!("[dashboard] failed to start: {}", err);
            None
        }
    }
}

#[utoipa::path(
    get,
    path = "/v1/ping",
    responses(
        (status = 200, description = "Daemon is healthy", body = PingResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn ping() -> Json<PingResponse> {
    Json(PingResponse { ok: true })
}

#[utoipa::path(
    post,
    path = "/v1/agent/sessions",
    request_body = AgentSessionRegisterRequest,
    responses(
        (status = 200, description = "Agent session registered", body = AgentSession),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn register_agent_session(
    State(state): State<AppState>,
    Json(req): Json<AgentSessionRegisterRequest>,
) -> Result<Json<AgentSession>, AppError> {
    let session = {
        let mut guard = state.state.lock().await;
        cleanup_stale_agent_sessions(&mut guard.agent_sessions);
        register_agent_session_state(&mut guard.agent_sessions, req)
    };
    Ok(Json(session))
}

#[utoipa::path(
    delete,
    path = "/v1/agent/sessions/{agent_id}",
    params(("agent_id" = String, Path, description = "Agent session id")),
    responses(
        (status = 200, description = "Agent session unregistered"),
        (status = 404, description = "Agent session not found", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn unregister_agent_session(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let removed = {
        let mut guard = state.state.lock().await;
        cleanup_stale_agent_sessions(&mut guard.agent_sessions);
        guard.agent_sessions.remove(&agent_id).is_some()
    };

    if !removed {
        return Err(AppError::not_found(format!(
            "agent session {agent_id} not found"
        )));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/v1/agent/sessions/{agent_id}/messages",
    request_body = AgentSessionMessageRequest,
    params(("agent_id" = String, Path, description = "Agent session id")),
    responses(
        (status = 200, description = "Message queued", body = AgentSessionMessageResponse),
        (status = 404, description = "Agent session not found", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn post_agent_message(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(req): Json<AgentSessionMessageRequest>,
) -> Result<Json<AgentSessionMessageResponse>, AppError> {
    let queued = {
        let mut guard = state.state.lock().await;
        cleanup_stale_agent_sessions(&mut guard.agent_sessions);
        queue_agent_message(&mut guard.agent_sessions, &agent_id, req.message)?
    };

    Ok(Json(AgentSessionMessageResponse { queued }))
}

#[utoipa::path(
    get,
    path = "/v1/agent/sessions/{agent_id}/messages/poll",
    params(("agent_id" = String, Path, description = "Agent session id")),
    responses(
        (status = 200, description = "Queued messages for the session", body = AgentSessionPollResponse),
        (status = 404, description = "Agent session not found", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn poll_agent_messages(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Result<Json<AgentSessionPollResponse>, AppError> {
    let messages = {
        let mut guard = state.state.lock().await;
        cleanup_stale_agent_sessions(&mut guard.agent_sessions);
        poll_agent_session_messages(&mut guard.agent_sessions, &agent_id)?
    };

    Ok(Json(AgentSessionPollResponse { messages }))
}

#[utoipa::path(
    get,
    path = "/v1/agent/sessions/latest",
    params(("project_dir" = String, Query, description = "Project directory")),
    responses(
        (status = 200, description = "Latest agent session for project", body = LatestAgentSessionResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn get_latest_agent_session(
    State(state): State<AppState>,
    Query(query): Query<LatestAgentSessionQuery>,
) -> Result<Json<LatestAgentSessionResponse>, AppError> {
    let session = {
        let mut guard = state.state.lock().await;
        cleanup_stale_agent_sessions(&mut guard.agent_sessions);
        find_latest_agent_session_for_project(&guard.agent_sessions, &query.project_dir)
            .map(agent_session_from_state)
    };

    Ok(Json(LatestAgentSessionResponse { session }))
}

#[utoipa::path(
    post,
    path = "/v1/agent/share",
    request_body = ShareAgentMessageRequest,
    responses(
        (status = 200, description = "Message queued to latest project agent", body = ShareAgentMessageResponse),
        (status = 404, description = "No matching agent session", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn share_agent_message(
    State(state): State<AppState>,
    Json(req): Json<ShareAgentMessageRequest>,
) -> Result<Json<ShareAgentMessageResponse>, AppError> {
    let (agent_id, queued) = {
        let mut guard = state.state.lock().await;
        cleanup_stale_agent_sessions(&mut guard.agent_sessions);
        let agent_id =
            find_latest_agent_session_for_project(&guard.agent_sessions, &req.project_dir)
                .map(|session| session.agent_id.clone())
                .ok_or_else(|| {
                    AppError::not_found(format!(
                        "no active agent session found for project {}",
                        req.project_dir
                    ))
                })?;
        let full_message = match req.command {
            Some(cmd) if !cmd.is_empty() => format!("{}\nRun `{cmd}`", req.message),
            _ => req.message,
        };
        let queued = queue_agent_message(&mut guard.agent_sessions, &agent_id, full_message)?;
        (agent_id, queued)
    };

    Ok(Json(ShareAgentMessageResponse { agent_id, queued }))
}

#[utoipa::path(
    post,
    path = "/v1/runs/up",
    request_body = UpRequest,
    responses(
        (status = 200, description = "Run created or refreshed", body = RunManifest),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn up(
    State(state): State<AppState>,
    Json(req): Json<UpRequest>,
) -> Result<Json<RunManifest>, AppError> {
    let manifest = orchestrate_up(&state, req).await?;
    Ok(Json(manifest))
}

#[utoipa::path(
    post,
    path = "/v1/runs/down",
    request_body = DownRequest,
    responses(
        (status = 200, description = "Run stopped", body = RunManifest),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 404, description = "Run not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn down(
    State(state): State<AppState>,
    Json(req): Json<DownRequest>,
) -> Result<Json<RunManifest>, AppError> {
    let manifest = orchestrate_down(&state, &req.run_id, req.purge).await?;
    Ok(Json(manifest))
}

#[utoipa::path(
    post,
    path = "/v1/runs/kill",
    request_body = KillRequest,
    responses(
        (status = 200, description = "Run killed", body = RunManifest),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 404, description = "Run not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn kill(
    State(state): State<AppState>,
    Json(req): Json<KillRequest>,
) -> Result<Json<RunManifest>, AppError> {
    let manifest = orchestrate_kill(&state, &req.run_id).await?;
    Ok(Json(manifest))
}

#[derive(Debug, Deserialize)]
pub(crate) struct EventStreamQuery {
    run_id: Option<String>,
}

pub(crate) async fn events(
    State(state): State<AppState>,
    Query(query): Query<EventStreamQuery>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, AppError> {
    if let Some(run_id) = query.run_id.as_deref() {
        let exists = {
            let guard = state.state.lock().await;
            guard.runs.contains_key(run_id)
        };
        if !exists {
            return Err(AppError::not_found(format!("run {run_id} not found")));
        }
        retain_run_log_tail(&state, run_id).await?;
    }

    let mut event_rx = state.event_tx.subscribe();
    let run_filter = query.run_id.clone();
    let subscription = LogTailSubscription {
        state: state.clone(),
        run_id: run_filter.clone(),
    };
    let (stream_tx, stream_rx) = tokio::sync::mpsc::channel(32);

    tokio::spawn(async move {
        let _subscription = subscription;
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if !event.should_deliver(run_filter.as_deref()) {
                        continue;
                    }
                    let payload = match event.payload_json() {
                        Ok(payload) => payload,
                        Err(err) => {
                            eprintln!("devstack: failed to serialize SSE event: {err}");
                            continue;
                        }
                    };
                    let sse_event = Event::default().event(event.event_name()).data(payload);
                    if stream_tx.send(Ok(sse_event)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(stream_rx)).keep_alive(KeepAlive::default()))
}

#[utoipa::path(
    post,
    path = "/v1/tasks/run",
    request_body = StartTaskRequest,
    responses(
        (status = 200, description = "Detached task accepted", body = StartTaskResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn start_task(
    State(state): State<AppState>,
    Json(req): Json<StartTaskRequest>,
) -> Result<Json<StartTaskResponse>, AppError> {
    let project_dir = PathBuf::from(&req.project_dir);
    let config_path = req
        .file
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| ConfigFile::default_path(&project_dir));
    let config = ConfigFile::load_from_path(&config_path)
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let task = config
        .tasks
        .as_ref()
        .and_then(|tasks| tasks.as_map().get(&req.task))
        .cloned()
        .ok_or_else(|| AppError::bad_request(format!("unknown task '{}'", req.task)))?;

    let run_id = find_latest_active_run_for_project(&state, &project_dir)
        .await
        .map_err(AppError::from)?;
    let execution_id = format!("task-{:016x}", rand::rng().random::<u64>());
    let started_at = now_rfc3339();
    let detached_task = DetachedTaskExecution {
        execution_id: execution_id.clone(),
        task: req.task.clone(),
        project_dir: project_dir.clone(),
        run_id: run_id.clone(),
        state: TaskExecutionState::Running,
        started_at: started_at.clone(),
        started_at_instant: StdInstant::now(),
        finished_at: None,
        exit_code: None,
        duration_ms: None,
    };

    {
        let mut guard = state.state.lock().await;
        let duplicate = guard.detached_tasks.values().any(|task| {
            task.state == TaskExecutionState::Running
                && task.task == req.task
                && match (task.run_id.as_deref(), run_id.as_deref()) {
                    (Some(existing), Some(candidate)) => existing == candidate,
                    (None, None) => same_project_dir(&task.project_dir, &project_dir),
                    _ => false,
                }
        });
        if duplicate {
            return Err(AppError::bad_request(format!(
                "task '{}' is already running",
                req.task
            )));
        }
        guard
            .detached_tasks
            .insert(execution_id.clone(), detached_task.clone());
    }

    emit_event(
        &state,
        task_event(&detached_task, DaemonTaskEventKind::Started),
    );

    tokio::spawn(execute_detached_task(
        state.clone(),
        detached_task,
        task,
        req.args.clone(),
    ));

    Ok(Json(StartTaskResponse {
        execution_id,
        task: req.task,
        run_id,
    }))
}

#[utoipa::path(
    get,
    path = "/v1/tasks/{execution_id}",
    params(
        ("execution_id" = String, Path, description = "Task execution id")
    ),
    responses(
        (status = 200, description = "Task execution status", body = TaskStatusResponse),
        (status = 404, description = "Task execution not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn task_status(
    State(state): State<AppState>,
    AxumPath(execution_id): AxumPath<String>,
) -> Result<Json<TaskStatusResponse>, AppError> {
    let task = {
        let guard = state.state.lock().await;
        guard
            .detached_tasks
            .get(&execution_id)
            .cloned()
            .ok_or_else(|| {
                AppError::not_found(format!("task execution {execution_id} not found"))
            })?
    };
    Ok(Json(task_status_response(&task)))
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/status",
    params(
        ("run_id" = String, Path, description = "Run id")
    ),
    responses(
        (status = 200, description = "Run status", body = RunStatusResponse),
        (status = 404, description = "Run not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn status(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
) -> Result<Json<RunStatusResponse>, AppError> {
    let response = build_status(&state, &run_id).await?;
    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/tasks",
    params(
        ("run_id" = String, Path, description = "Run id")
    ),
    responses(
        (status = 200, description = "Latest task executions for the run", body = TasksResponse),
        (status = 404, description = "Run not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn run_tasks(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
) -> Result<Json<TasksResponse>, AppError> {
    let detached_tasks = {
        let guard = state.state.lock().await;
        guard
            .runs
            .get(&run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;
        guard
            .detached_tasks
            .values()
            .filter(|task| task.run_id.as_deref() == Some(run_id.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };

    let history = crate::tasks::TaskHistory::load(&paths::task_history_path(&RunId::new(&run_id))?)
        .map_err(AppError::from)?;
    let mut tasks = BTreeMap::new();
    for execution in history.latest_by_task().into_values() {
        merge_task_summary(&mut tasks, task_summary_from_history(execution));
    }
    for task in detached_tasks {
        merge_task_summary(&mut tasks, task_summary_from_detached(&task));
    }

    Ok(Json(TasksResponse {
        tasks: tasks.into_values().collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/watch",
    params(
        ("run_id" = String, Path, description = "Run id")
    ),
    responses(
        (status = 200, description = "Watch status", body = RunWatchResponse),
        (status = 404, description = "Run not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn watch_status(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
) -> Result<Json<RunWatchResponse>, AppError> {
    let response = build_watch_status(&state, &run_id).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/watch/pause",
    request_body = WatchControlRequest,
    params(
        ("run_id" = String, Path, description = "Run id")
    ),
    responses(
        (status = 200, description = "Updated watch status", body = RunWatchResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 404, description = "Run or service not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn watch_pause(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
    Json(req): Json<WatchControlRequest>,
) -> Result<Json<RunWatchResponse>, AppError> {
    let response = orchestrate_watch_pause(&state, &run_id, req.service.as_deref()).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/watch/resume",
    request_body = WatchControlRequest,
    params(
        ("run_id" = String, Path, description = "Run id")
    ),
    responses(
        (status = 200, description = "Updated watch status", body = RunWatchResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 404, description = "Run or service not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn watch_resume(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
    Json(req): Json<WatchControlRequest>,
) -> Result<Json<RunWatchResponse>, AppError> {
    let response = orchestrate_watch_resume(&state, &run_id, req.service.as_deref()).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/restart-service",
    request_body = RestartServiceRequest,
    params(
        ("run_id" = String, Path, description = "Run id")
    ),
    responses(
        (status = 200, description = "Service restarted", body = RunManifest),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 404, description = "Run or service not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn restart_service(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
    Json(req): Json<RestartServiceRequest>,
) -> Result<Json<RunManifest>, AppError> {
    let manifest = orchestrate_restart_service(&state, &run_id, &req.service, req.no_wait).await?;
    Ok(Json(manifest))
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/logs/{service}",
    params(
        ("run_id" = String, Path, description = "Run id"),
        ("service" = String, Path, description = "Service name"),
        ("last" = Option<usize>, Query, description = "Max lines to return (default 500; legacy alias: tail)"),
        ("since" = Option<String>, Query, description = "ISO8601 timestamp to filter logs after"),
        ("search" = Option<String>, Query, description = "Tantivy query string (legacy alias: q)"),
        ("level" = Option<String>, Query, description = "Filter by level: all, warn, error"),
        ("stream" = Option<String>, Query, description = "Filter by stream: stdout, stderr"),
        ("after" = Option<u64>, Query, description = "Cursor: return lines with seq > after (use next_after from response)")
    ),
    responses(
        (status = 200, description = "Log lines", body = LogsResponse),
        (status = 404, description = "Run or service not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn logs(
    State(state): State<AppState>,
    AxumPath((run_id, service)): AxumPath<(String, String)>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, AppError> {
    let response = read_service_logs(&state, &run_id, &service, query).await?;
    Ok(Json(response))
}

fn run_service_log_sources(run_id: &str, run: &RunState) -> Vec<LogSource> {
    run.services
        .iter()
        .map(|(service, runtime)| LogSource {
            run_id: run_id.to_string(),
            service: service.clone(),
            path: runtime.log_path.clone(),
        })
        .collect()
}

fn discover_task_log_sources(run_id: &str) -> Result<Vec<LogSource>> {
    let task_logs_dir = paths::run_task_logs_dir(&RunId::new(run_id))?;
    if !task_logs_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&task_logs_dir)
        .with_context(|| format!("read task log dir {}", task_logs_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        sources.push(LogSource {
            run_id: run_id.to_string(),
            service: format!("task:{name}"),
            path,
        });
    }
    sources.sort_by(|left, right| left.service.cmp(&right.service));
    Ok(sources)
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/logs",
    params(
        ("run_id" = String, Path, description = "Run id"),
        ("last" = Option<usize>, Query, description = "Max entries to return (default 500; legacy alias: tail)"),
        ("since" = Option<String>, Query, description = "ISO8601 timestamp to filter logs after"),
        ("search" = Option<String>, Query, description = "Tantivy query string (legacy alias: q)"),
        ("level" = Option<String>, Query, description = "Filter by level: all, warn, error"),
        ("stream" = Option<String>, Query, description = "Filter by stream: stdout, stderr"),
        ("service" = Option<String>, Query, description = "Filter by service name"),
        ("include_entries" = Option<bool>, Query, description = "Include log entries in the response (default true)"),
        ("include_facets" = Option<bool>, Query, description = "Include dynamic facet counts for the current scope")
    ),
    responses(
        (status = 200, description = "Combined log view", body = LogViewResponse),
        (status = 404, description = "Run not found", body = crate::api::ErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn logs_view(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
    Query(query): Query<LogViewQuery>,
) -> Result<Json<LogViewResponse>, AppError> {
    {
        let guard = state.state.lock().await;
        guard
            .runs
            .get(&run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;
    }

    let index = state.log_index.clone();
    let run_id_for_task = run_id.clone();
    let response = tokio::task::spawn_blocking(move || index.query_view(&run_id_for_task, query))
        .await
        .map_err(|e| AppError::Internal(anyhow!("log view task failed: {e}")))?
        .map_err(map_log_index_error)?;

    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/v1/runs",
    responses(
        (status = 200, description = "Run list", body = RunListResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn list_runs(
    State(state): State<AppState>,
) -> Result<Json<RunListResponse>, AppError> {
    let runs = {
        let state = state.state.lock().await;
        state
            .runs
            .values()
            .map(|run| RunSummary {
                run_id: run.run_id.clone(),
                stack: run.stack.clone(),
                project_dir: run.project_dir.to_string_lossy().to_string(),
                state: run.state.clone(),
                created_at: run.created_at.clone(),
                stopped_at: run.stopped_at.clone(),
            })
            .collect()
    };
    Ok(Json(RunListResponse { runs }))
}

#[utoipa::path(
    get,
    path = "/v1/globals",
    responses(
        (status = 200, description = "Global services", body = GlobalsResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn list_globals(
    State(_state): State<AppState>,
) -> Result<Json<GlobalsResponse>, AppError> {
    let globals = list_globals_from_disk().map_err(AppError::from)?;
    Ok(Json(GlobalsResponse { globals }))
}

#[utoipa::path(
    post,
    path = "/v1/gc",
    request_body = GcRequest,
    responses(
        (status = 200, description = "Garbage collection results", body = GcResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn gc(
    State(state): State<AppState>,
    Json(req): Json<GcRequest>,
) -> Result<Json<GcResponse>, AppError> {
    let response = run_gc(&state, req).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/v1/navigation/intent",
    request_body = SetNavigationIntentRequest,
    responses(
        (status = 200, description = "Navigation intent stored", body = NavigationIntentResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn set_navigation_intent(
    State(state): State<AppState>,
    Json(req): Json<SetNavigationIntentRequest>,
) -> Result<Json<NavigationIntentResponse>, AppError> {
    let intent = NavigationIntent {
        run_id: req.run_id,
        service: req.service,
        search: req.search,
        level: req.level,
        stream: req.stream,
        since: req.since,
        last: req.last,
        created_at: now_rfc3339(),
    };

    {
        let mut guard = state.state.lock().await;
        guard.navigation_intent = Some(intent.clone());
    }

    Ok(Json(NavigationIntentResponse {
        intent: Some(intent),
    }))
}

#[utoipa::path(
    get,
    path = "/v1/navigation/intent",
    responses(
        (status = 200, description = "Current navigation intent", body = NavigationIntentResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn get_navigation_intent(
    State(state): State<AppState>,
) -> Result<Json<NavigationIntentResponse>, AppError> {
    let intent = {
        let guard = state.state.lock().await;
        guard.navigation_intent.clone()
    };

    Ok(Json(NavigationIntentResponse { intent }))
}

#[utoipa::path(
    delete,
    path = "/v1/navigation/intent",
    responses(
        (status = 200, description = "Navigation intent cleared"),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn clear_navigation_intent(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    {
        let mut guard = state.state.lock().await;
        guard.navigation_intent = None;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get,
    path = "/v1/projects",
    responses(
        (status = 200, description = "List of registered projects", body = ProjectsResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn list_projects() -> Result<Json<ProjectsResponse>, AppError> {
    let ledger = ProjectsLedger::load().map_err(AppError::from)?;
    let projects = ledger.to_summaries();
    Ok(Json(ProjectsResponse { projects }))
}

#[utoipa::path(
    post,
    path = "/v1/projects/register",
    request_body = RegisterProjectRequest,
    responses(
        (status = 200, description = "Project registered", body = RegisterProjectResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn register_project(
    Json(req): Json<RegisterProjectRequest>,
) -> Result<Json<RegisterProjectResponse>, AppError> {
    let path = PathBuf::from(&req.path);
    if !path.exists() {
        return Err(AppError::bad_request(format!(
            "path does not exist: {}",
            req.path
        )));
    }

    let mut ledger = ProjectsLedger::load().map_err(AppError::from)?;
    let id = ledger.register(&path).map_err(AppError::from)?;

    let project = ledger
        .to_summaries()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("failed to find registered project")))?;

    Ok(Json(RegisterProjectResponse { project }))
}

#[utoipa::path(
    delete,
    path = "/v1/projects/{project_id}",
    params(
        ("project_id" = String, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Project removed"),
        (status = 404, description = "Project not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn remove_project(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut ledger = ProjectsLedger::load().map_err(AppError::from)?;
    let removed = ledger.remove(&project_id).map_err(AppError::from)?;

    if !removed {
        return Err(AppError::not_found(format!(
            "project {} not found",
            project_id
        )));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

fn source_summary(entry: &crate::sources::SourceEntry) -> SourceSummary {
    SourceSummary {
        name: entry.name.clone(),
        paths: entry.paths.clone(),
        created_at: entry.created_at.clone(),
    }
}

fn source_log_sources(ledger: &SourcesLedger, name: &str) -> Result<Vec<LogSource>> {
    let run_id = source_run_id(name);
    Ok(ledger
        .resolve_log_sources(name)?
        .into_iter()
        .map(|item| LogSource {
            run_id: run_id.clone(),
            service: item.service,
            path: item.path,
        })
        .collect())
}

fn all_source_log_sources(ledger: &SourcesLedger) -> Result<Vec<LogSource>> {
    let mut out = Vec::new();
    for name in ledger.sources.keys() {
        out.extend(source_log_sources(ledger, name)?);
    }
    Ok(out)
}

#[utoipa::path(
    get,
    path = "/v1/sources",
    responses(
        (status = 200, description = "List registered log sources", body = SourcesResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn list_sources() -> Result<Json<SourcesResponse>, AppError> {
    let ledger = SourcesLedger::load().map_err(AppError::from)?;
    let sources = ledger.list().iter().map(source_summary).collect();
    Ok(Json(SourcesResponse { sources }))
}

#[utoipa::path(
    post,
    path = "/v1/sources",
    request_body = AddSourceRequest,
    responses(
        (status = 200, description = "Source added", body = AddSourceResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn add_source(
    State(state): State<AppState>,
    Json(req): Json<AddSourceRequest>,
) -> Result<Json<AddSourceResponse>, AppError> {
    let mut ledger = SourcesLedger::load().map_err(AppError::from)?;
    ledger
        .add(&req.name, req.paths)
        .map_err(|err| AppError::bad_request(err.to_string()))?;

    let source = ledger
        .get(&req.name)
        .cloned()
        .ok_or_else(|| AppError::Internal(anyhow!("source {} was not persisted", req.name)))?;

    let index = state.log_index.clone();
    let name = req.name.clone();
    tokio::task::spawn_blocking(move || {
        let run_id = source_run_id(&name);
        index.delete_run(&run_id)?;
        let ledger = SourcesLedger::load()?;
        let sources = source_log_sources(&ledger, &name)?;
        if !sources.is_empty() {
            index.ingest_sources(&sources)?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|e| AppError::Internal(anyhow!("source ingest task failed: {e}")))?
    .map_err(AppError::from)?;

    Ok(Json(AddSourceResponse {
        source: source_summary(&source),
    }))
}

#[utoipa::path(
    delete,
    path = "/v1/sources/{name}",
    params(("name" = String, Path, description = "Source name")),
    responses(
        (status = 200, description = "Source removed"),
        (status = 404, description = "Source not found", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn remove_source(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut ledger = SourcesLedger::load().map_err(AppError::from)?;
    let removed = ledger.remove(&name).map_err(AppError::from)?;
    if !removed {
        return Err(AppError::not_found(format!("source {} not found", name)));
    }

    let index = state.log_index.clone();
    let run_id = source_run_id(&name);
    tokio::task::spawn_blocking(move || index.delete_run(&run_id))
        .await
        .map_err(|e| AppError::Internal(anyhow!("source cleanup task failed: {e}")))?
        .map_err(AppError::from)?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get,
    path = "/v1/sources/{name}/logs",
    params(
        ("name" = String, Path, description = "Source name"),
        ("last" = Option<usize>, Query, description = "Max entries to return (default 500; legacy alias: tail)"),
        ("since" = Option<String>, Query, description = "ISO8601 timestamp to filter logs after"),
        ("search" = Option<String>, Query, description = "Tantivy query string (legacy alias: q)"),
        ("level" = Option<String>, Query, description = "Filter by level: all, warn, error"),
        ("stream" = Option<String>, Query, description = "Filter by stream: stdout, stderr"),
        ("service" = Option<String>, Query, description = "Filter by service name"),
        ("include_entries" = Option<bool>, Query, description = "Include log entries in the response (default true)"),
        ("include_facets" = Option<bool>, Query, description = "Include dynamic facet counts for the current scope")
    ),
    responses(
        (status = 200, description = "Combined source log view", body = LogViewResponse),
        (status = 404, description = "Source not found", body = crate::api::ErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub(crate) async fn source_logs_view(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<LogViewQuery>,
) -> Result<Json<LogViewResponse>, AppError> {
    let ledger = SourcesLedger::load().map_err(AppError::from)?;
    if ledger.get(&name).is_none() {
        return Err(AppError::not_found(format!("source {} not found", name)));
    }

    let run_id = source_run_id(&name);
    let index = state.log_index.clone();
    let response = tokio::task::spawn_blocking(move || index.query_view(&run_id, query))
        .await
        .map_err(|e| AppError::Internal(anyhow!("source log view task failed: {e}")))?
        .map_err(map_log_index_error)?;

    Ok(Json(response))
}

fn register_agent_session_state(
    sessions: &mut BTreeMap<String, AgentSessionState>,
    req: AgentSessionRegisterRequest,
) -> AgentSession {
    let now = now_rfc3339();
    let entry = sessions
        .entry(req.agent_id.clone())
        .or_insert_with(|| AgentSessionState {
            agent_id: req.agent_id.clone(),
            project_dir: req.project_dir.clone(),
            stack: req.stack.clone(),
            command: req.command.clone(),
            pid: req.pid,
            created_at: now.clone(),
            pending_messages: VecDeque::new(),
        });

    entry.project_dir = req.project_dir;
    entry.stack = req.stack;
    entry.command = req.command;
    entry.pid = req.pid;

    agent_session_from_state(entry)
}

fn agent_session_from_state(session: &AgentSessionState) -> AgentSession {
    AgentSession {
        agent_id: session.agent_id.clone(),
        project_dir: session.project_dir.clone(),
        stack: session.stack.clone(),
        command: session.command.clone(),
        pid: session.pid,
        created_at: session.created_at.clone(),
    }
}

fn find_latest_agent_session_for_project<'a>(
    sessions: &'a BTreeMap<String, AgentSessionState>,
    project_dir: &str,
) -> Option<&'a AgentSessionState> {
    sessions
        .values()
        .filter(|session| session.project_dir == project_dir)
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.agent_id.cmp(&b.agent_id))
        })
}

fn queue_agent_message(
    sessions: &mut BTreeMap<String, AgentSessionState>,
    agent_id: &str,
    message: String,
) -> AppResult<usize> {
    let session = sessions
        .get_mut(agent_id)
        .ok_or_else(|| AppError::not_found(format!("agent session {agent_id} not found")))?;
    session.pending_messages.push_back(message);
    Ok(session.pending_messages.len())
}

fn poll_agent_session_messages(
    sessions: &mut BTreeMap<String, AgentSessionState>,
    agent_id: &str,
) -> AppResult<Vec<String>> {
    let session = sessions
        .get_mut(agent_id)
        .ok_or_else(|| AppError::not_found(format!("agent session {agent_id} not found")))?;
    Ok(session.pending_messages.drain(..).collect())
}

fn cleanup_stale_agent_sessions(sessions: &mut BTreeMap<String, AgentSessionState>) {
    sessions.retain(|_, session| is_pid_alive(session.pid));
}

fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }

    let err = std::io::Error::last_os_error();
    err.raw_os_error() == Some(libc::EPERM)
}

fn spawn_periodic_agent_session_cleanup(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.tick().await;
        loop {
            interval.tick().await;
            let mut guard = state.state.lock().await;
            cleanup_stale_agent_sessions(&mut guard.agent_sessions);
        }
    });
}

fn load_state_from_disk() -> Result<DaemonState> {
    let mut state = DaemonState::default();
    let runs_dir = paths::runs_dir()?;
    if runs_dir.exists() {
        for entry in std::fs::read_dir(runs_dir)? {
            let entry = entry?;
            let manifest_path = entry.path().join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            if let Ok(manifest) = RunManifest::load_from_path(&manifest_path) {
                if manifest.state == RunLifecycle::Stopped || manifest.stopped_at.is_some() {
                    continue;
                }
                let services = manifest
                    .services
                    .iter()
                    .map(|(name, svc)| {
                        (
                            name.clone(),
                            ServiceRuntime {
                                name: name.clone(),
                                unit_name: unit_name_for_run(&manifest.run_id, name),
                                port: svc.port,
                                scheme: "http".to_string(),
                                url: svc.url.clone(),
                                deps: Vec::new(),
                                readiness: ReadinessSpec::new(
                                    crate::readiness::ReadinessKind::None,
                                ),
                                log_path: entry.path().join("logs").join(format!("{name}.log")),
                                cwd: PathBuf::from(&manifest.project_dir),
                                env: manifest.env.clone(),
                                state: svc.state.clone(),
                                last_failure: None,
                                health: None,
                                last_started_at: Some(manifest.created_at.clone()),
                                watch_hash: svc.watch_hash.clone(),
                                watch_patterns: Vec::new(),
                                ignore_patterns: Vec::new(),
                                watch_extra_files: Vec::new(),
                                watch_fingerprint: Vec::new(),
                                auto_restart: false,
                                watch_paused: false,
                                watch_handle: None,
                            },
                        )
                    })
                    .collect();
                state.runs.insert(
                    manifest.run_id.clone(),
                    RunState {
                        run_id: manifest.run_id,
                        stack: manifest.stack,
                        project_dir: PathBuf::from(manifest.project_dir),
                        base_env: manifest.env.clone(),
                        services,
                        state: manifest.state,
                        created_at: manifest.created_at,
                        stopped_at: manifest.stopped_at,
                    },
                );
            }
        }
    }
    Ok(state)
}

async fn run_init_tasks_blocking(
    tasks_map: BTreeMap<String, TaskConfig>,
    init_tasks: Vec<String>,
    project_dir: PathBuf,
    run_id: RunId,
) -> Result<()> {
    let history_path = paths::task_history_path(&run_id)?;
    tokio::task::spawn_blocking(move || {
        crate::tasks::run_init_tasks(
            &tasks_map,
            &init_tasks,
            &project_dir,
            crate::tasks::TaskLogScope::Run(&run_id),
            &history_path,
            false,
        )
    })
    .await
    .map_err(|err| anyhow!("init task worker failed: {err}"))?
}

async fn run_post_init_tasks_blocking(
    tasks_map: BTreeMap<String, TaskConfig>,
    post_init_tasks: Vec<String>,
    project_dir: PathBuf,
    run_id: RunId,
) -> Result<()> {
    let history_path = paths::task_history_path(&run_id)?;
    tokio::task::spawn_blocking(move || {
        crate::tasks::run_post_init_tasks(
            &tasks_map,
            &post_init_tasks,
            &project_dir,
            crate::tasks::TaskLogScope::Run(&run_id),
            &history_path,
            false,
        )
    })
    .await
    .map_err(|err| anyhow!("post_init task worker failed: {err}"))?
}

async fn execute_detached_task(
    state: AppState,
    detached_task: DetachedTaskExecution,
    task: TaskConfig,
    args: Vec<String>,
) {
    let result = if let Some(run_id) = detached_task.run_id.clone() {
        let run_id = RunId::new(run_id);
        match paths::task_history_path(&run_id) {
            Ok(history_path) => tokio::task::spawn_blocking({
                let task_name = detached_task.task.clone();
                let project_dir = detached_task.project_dir.clone();
                let args = args.clone();
                let task = task.clone();
                move || {
                    crate::tasks::run_task(
                        &task_name,
                        &task,
                        &project_dir,
                        crate::tasks::TaskLogScope::Run(&run_id),
                        &history_path,
                        false,
                        &args,
                    )
                }
            })
            .await
            .map_err(|err| anyhow!("task worker failed: {err}"))
            .and_then(|result| result),
            Err(err) => Err(err),
        }
    } else {
        match paths::ad_hoc_task_history_path(&detached_task.project_dir) {
            Ok(history_path) => tokio::task::spawn_blocking({
                let task_name = detached_task.task.clone();
                let project_dir = detached_task.project_dir.clone();
                let args = args.clone();
                move || {
                    crate::tasks::run_task(
                        &task_name,
                        &task,
                        &project_dir,
                        crate::tasks::TaskLogScope::AdHoc,
                        &history_path,
                        false,
                        &args,
                    )
                }
            })
            .await
            .map_err(|err| anyhow!("task worker failed: {err}"))
            .and_then(|result| result),
            Err(err) => Err(err),
        }
    };

    let updated_task = {
        let mut guard = state.state.lock().await;
        let Some(entry) = guard.detached_tasks.get_mut(&detached_task.execution_id) else {
            return;
        };
        entry.finished_at = Some(now_rfc3339());
        entry.duration_ms = Some(
            detached_task
                .started_at_instant
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        match result {
            Ok(task_result) => {
                entry.exit_code = Some(task_result.exit_code);
                entry.state = if task_result.success() {
                    TaskExecutionState::Completed
                } else {
                    TaskExecutionState::Failed
                };
            }
            Err(err) => {
                eprintln!(
                    "devstack: detached task {} failed to execute: {err}",
                    detached_task.execution_id
                );
                entry.state = TaskExecutionState::Failed;
            }
        }
        entry.clone()
    };

    if let Some(run_id) = updated_task.run_id.clone() {
        let task_name = updated_task.task.clone();
        let log_index = state.log_index.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let path = paths::task_log_path(&RunId::new(&run_id), &task_name)?;
            if !path.exists() {
                return Ok::<(), anyhow::Error>(());
            }
            log_index.ingest_sources(&[LogSource {
                run_id: run_id.clone(),
                service: format!("task:{task_name}"),
                path,
            }])?;
            Ok(())
        })
        .await;
    }

    let kind = if updated_task.state == TaskExecutionState::Completed {
        DaemonTaskEventKind::Completed
    } else {
        DaemonTaskEventKind::Failed
    };
    emit_event(&state, task_event(&updated_task, kind));
}

async fn persist_manifest_on_error(state: &AppState, run_id: &str) {
    if let Err(err) = persist_manifest(state, run_id).await {
        eprintln!("failed to persist manifest for {run_id}: {err}");
    }
}

async fn orchestrate_up(state: &AppState, req: UpRequest) -> AppResult<RunManifest> {
    let project_dir = PathBuf::from(&req.project_dir);

    // Register/touch the project in the ledger
    if let Ok(mut ledger) = ProjectsLedger::load() {
        let _ = ledger.touch(&project_dir);
    }

    let config_path = req
        .file
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::ConfigFile::default_path(&project_dir));
    let config = ConfigFile::load_from_path(&config_path)
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let stack_plan = config
        .stack_plan(&req.stack)
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let config_dir = config_path.parent().unwrap_or(&project_dir).to_path_buf();

    if !req.new_run
        && req.run_id.is_none()
        && let Some(existing) = find_latest_run_for_project_stack(state, &project_dir, &req.stack)
            .await
            .map_err(AppError::from)?
    {
        return orchestrate_refresh_run(
            state,
            &existing,
            &config,
            &stack_plan,
            &project_dir,
            &config_path,
            req.no_wait,
            req.force,
        )
        .await;
    }

    let run_id = req.run_id.unwrap_or_else(|| generate_run_id(&req.stack));
    let run_id = RunId::new(run_id);

    paths::ensure_base_layout().map_err(AppError::from)?;
    let run_dir = paths::run_dir(&run_id).map_err(AppError::from)?;
    let logs_dir = paths::run_logs_dir(&run_id).map_err(AppError::from)?;
    std::fs::create_dir_all(&logs_dir).map_err(AppError::from)?;
    std::fs::create_dir_all(&run_dir).map_err(AppError::from)?;

    // Snapshot config for reproducibility.
    let snapshot_path = paths::run_snapshot_path(&run_id).map_err(AppError::from)?;
    let raw = std::fs::read(&config_path).map_err(AppError::from)?;
    atomic_write(&snapshot_path, &raw).map_err(AppError::from)?;

    let mut port_map = allocate_ports(&stack_plan.services)
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();

    let globals = config.globals_map();
    let global_ports = ensure_globals(state, &globals, &tasks_map, &project_dir, &config_dir)
        .await
        .map_err(AppError::from)?;

    let mut service_schemes = BTreeMap::new();
    for (name, svc) in &stack_plan.services {
        let scheme = svc.scheme();
        service_schemes.insert(name.clone(), scheme);
    }
    for (name, svc) in &globals {
        let scheme = svc.scheme();
        service_schemes.insert(name.clone(), scheme);
    }
    for (name, port) in &global_ports {
        port_map.entry(name.clone()).or_insert(*port);
    }

    let base_env = build_base_env(
        &run_id,
        &stack_plan.name,
        &project_dir,
        &port_map,
        &service_schemes,
    )
    .map_err(AppError::from)?;

    let run_state = RunState {
        run_id: run_id.as_str().to_string(),
        stack: stack_plan.name.clone(),
        project_dir: project_dir.clone(),
        base_env: base_env.clone(),
        services: BTreeMap::new(),
        state: RunLifecycle::Starting,
        created_at: now_rfc3339(),
        stopped_at: None,
    };

    let run_created = {
        let mut guard = state.state.lock().await;
        if guard.runs.contains_key(run_id.as_str()) {
            return Err(AppError::bad_request(format!(
                "run_id {} already exists",
                run_id.as_str()
            )));
        }
        guard.runs.insert(run_id.as_str().to_string(), run_state);
        guard.runs.get(run_id.as_str()).map(run_created_event)
    };
    if let Some(event) = run_created {
        emit_event(state, event);
    }

    for svc_name in &stack_plan.order {
        let svc = stack_plan
            .services
            .get(svc_name)
            .ok_or_else(|| AppError::bad_request(format!("service {svc_name} missing")))?;
        let prepared = prepare_service(
            &run_id,
            &stack_plan.name,
            &project_dir,
            &config_dir,
            svc_name,
            svc,
            &port_map,
            &service_schemes,
            &base_env,
        )
        .map_err(AppError::from)?;

        // Run init tasks before starting the service
        if let Some(init_tasks) = &svc.init
            && !init_tasks.is_empty()
            && let Err(err) = run_init_tasks_blocking(
                tasks_map.clone(),
                init_tasks.clone(),
                project_dir.clone(),
                run_id.clone(),
            )
            .await
        {
            eprintln!("[{svc_name}] init failed: {err}");
            // Record failure and skip starting the service
            let runtime = ServiceRuntime {
                name: prepared.name.clone(),
                unit_name: prepared.unit_name.clone(),
                port: prepared.port,
                scheme: prepared.scheme.clone(),
                url: prepared.url.clone(),
                deps: prepared.deps.clone(),
                readiness: prepared.readiness.clone(),
                log_path: prepared.log_path.clone(),
                cwd: prepared.cwd.clone(),
                env: prepared.env.clone(),
                state: ServiceState::Failed,
                last_failure: Some(format!("init task failed: {err}")),
                health: None,
                last_started_at: None,
                watch_hash: Some(prepared.watch_hash.clone()),
                watch_patterns: prepared.watch_patterns.clone(),
                ignore_patterns: prepared.ignore_patterns.clone(),
                watch_extra_files: prepared.watch_extra_files.clone(),
                watch_fingerprint: prepared.watch_fingerprint.clone(),
                auto_restart: prepared.auto_restart,
                watch_paused: false,
                watch_handle: None,
            };
            let service_event = {
                let mut guard = state.state.lock().await;
                if let Some(run) = guard.runs.get_mut(run_id.as_str()) {
                    run.services.insert(svc_name.clone(), runtime);
                    Some(service_state_changed_event(
                        run_id.as_str(),
                        svc_name,
                        ServiceState::Failed,
                    ))
                } else {
                    None
                }
            };
            if let Some(event) = service_event {
                emit_event(state, event);
            }
            let _ = sync_service_auto_restart_watcher(state, run_id.as_str(), svc_name).await;
            continue;
        }

        let start_result = start_prepared_service(state, &run_id, &prepared, false).await;

        let (initial_state, failure_reason, last_started_at) = match &start_result {
            Ok(()) => (ServiceState::Starting, None, Some(now_rfc3339())),
            Err(err) => (ServiceState::Failed, Some(err.to_string()), None),
        };

        let runtime = ServiceRuntime {
            name: prepared.name.clone(),
            unit_name: prepared.unit_name.clone(),
            port: prepared.port,
            scheme: prepared.scheme.clone(),
            url: prepared.url.clone(),
            deps: prepared.deps.clone(),
            readiness: prepared.readiness.clone(),
            log_path: prepared.log_path.clone(),
            cwd: prepared.cwd.clone(),
            env: prepared.env.clone(),
            state: initial_state,
            last_failure: failure_reason,
            health: None,
            last_started_at,
            watch_hash: Some(prepared.watch_hash.clone()),
            watch_patterns: prepared.watch_patterns.clone(),
            ignore_patterns: prepared.ignore_patterns.clone(),
            watch_extra_files: prepared.watch_extra_files.clone(),
            watch_fingerprint: prepared.watch_fingerprint.clone(),
            auto_restart: prepared.auto_restart,
            watch_paused: false,
            watch_handle: None,
        };

        let service_event = {
            let mut guard = state.state.lock().await;
            if let Some(run) = guard.runs.get_mut(run_id.as_str()) {
                let event =
                    service_state_changed_event(run_id.as_str(), svc_name, runtime.state.clone());
                run.services.insert(svc_name.clone(), runtime);
                Some(event)
            } else {
                None
            }
        };
        if let Some(event) = service_event {
            emit_event(state, event);
        }

        if let Err(err) = sync_service_auto_restart_watcher(state, run_id.as_str(), svc_name).await
        {
            eprintln!("devstack: failed to start watcher for {svc_name}: {err}");
        }

        // Only proceed with readiness check if service started successfully
        if start_result.is_ok()
            && let Err(err) = handle_readiness(
                state.clone(),
                run_id.as_str(),
                &prepared,
                req.no_wait,
                build_post_init_context(svc, &tasks_map, &project_dir, &run_id),
            )
            .await
        {
            persist_manifest_on_error(state, run_id.as_str()).await;
            return Err(err);
        }
    }

    let run_event = {
        let mut guard = state.state.lock().await;
        guard
            .runs
            .get_mut(run_id.as_str())
            .and_then(recompute_run_state)
    };
    if let Some(event) = run_event {
        emit_event(state, event);
    }

    let manifest = persist_manifest(state, run_id.as_str())
        .await
        .map_err(AppError::from)?;
    Ok(manifest)
}

struct ExistingServiceSnapshot {
    watch_hash: Option<String>,
    state: ServiceState,
    port: Option<u16>,
}

#[allow(clippy::too_many_arguments)]
async fn orchestrate_refresh_run(
    state: &AppState,
    run_id: &str,
    config: &ConfigFile,
    stack_plan: &StackPlan,
    project_dir: &Path,
    config_path: &Path,
    no_wait: bool,
    force: bool,
) -> AppResult<RunManifest> {
    let run_id = RunId::new(run_id.to_string());
    let config_dir = config_path.parent().unwrap_or(project_dir);

    let (existing, removed, reuse_ports) = {
        let guard = state.state.lock().await;
        let run = guard
            .runs
            .get(run_id.as_str())
            .ok_or_else(|| AppError::not_found(format!("run {} not found", run_id.as_str())))?;
        let reuse_ports = run.stopped_at.is_none();
        let mut existing = BTreeMap::new();
        for (name, svc) in &run.services {
            existing.insert(
                name.clone(),
                ExistingServiceSnapshot {
                    watch_hash: svc.watch_hash.clone(),
                    state: svc.state.clone(),
                    port: svc.port,
                },
            );
        }
        let mut removed = Vec::new();
        for name in run.services.keys() {
            if !stack_plan.services.contains_key(name) {
                removed.push(name.clone());
            }
        }
        (existing, removed, reuse_ports)
    };

    if !removed.is_empty() {
        for svc_name in &removed {
            let unit_name = unit_name_for_run(run_id.as_str(), svc_name);
            let _ = state.systemd.stop_unit(&unit_name).await;
        }
        let run_event = {
            let mut guard = state.state.lock().await;
            if let Some(run) = guard.runs.get_mut(run_id.as_str()) {
                for svc_name in &removed {
                    if let Some(mut svc) = run.services.remove(svc_name) {
                        stop_health_monitor_for_service(&mut svc);
                        stop_watch_for_service(&mut svc);
                    }
                }
                recompute_run_state(run)
            } else {
                None
            }
        };
        if let Some(event) = run_event {
            emit_event(state, event);
        }
    }

    let mut port_map = resolve_ports_for_refresh(&stack_plan.services, &existing, reuse_ports)
        .map_err(AppError::from)?;
    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();

    let globals = config.globals_map();
    let global_ports = ensure_globals(state, &globals, &tasks_map, project_dir, config_dir)
        .await
        .map_err(AppError::from)?;

    let mut service_schemes = BTreeMap::new();
    for (name, svc) in &stack_plan.services {
        service_schemes.insert(name.clone(), svc.scheme());
    }
    for (name, svc) in &globals {
        service_schemes.insert(name.clone(), svc.scheme());
    }
    for (name, port) in &global_ports {
        port_map.entry(name.clone()).or_insert(*port);
    }

    let base_env = build_base_env(
        &run_id,
        &stack_plan.name,
        project_dir,
        &port_map,
        &service_schemes,
    )
    .map_err(AppError::from)?;

    let run_event = {
        let mut guard = state.state.lock().await;
        if let Some(run) = guard.runs.get_mut(run_id.as_str()) {
            run.base_env = base_env.clone();
            let mut event = None;
            if matches!(run.state, RunLifecycle::Stopped) {
                run.state = RunLifecycle::Starting;
                run.stopped_at = None;
                event = Some(run_state_changed_event(run));
            }
            event
        } else {
            None
        }
    };
    if let Some(event) = run_event {
        emit_event(state, event);
    }

    let snapshot_path = paths::run_snapshot_path(&run_id).map_err(AppError::from)?;
    if let Ok(raw) = std::fs::read(config_path) {
        let _ = atomic_write(&snapshot_path, &raw);
    }

    for svc_name in &stack_plan.order {
        let svc = stack_plan
            .services
            .get(svc_name)
            .ok_or_else(|| AppError::bad_request(format!("service {svc_name} missing")))?;
        let prepared = prepare_service(
            &run_id,
            &stack_plan.name,
            project_dir,
            config_dir,
            svc_name,
            svc,
            &port_map,
            &service_schemes,
            &base_env,
        )
        .map_err(AppError::from)?;

        let mut needs_restart = force;
        if let Some(snapshot) = existing.get(svc_name) {
            if snapshot.watch_hash.as_deref() != Some(prepared.watch_hash.as_str()) {
                needs_restart = true;
            }
            if matches!(
                snapshot.state,
                ServiceState::Stopped | ServiceState::Failed | ServiceState::Degraded
            ) {
                needs_restart = true;
            }
        } else {
            needs_restart = true;
        }

        if needs_restart {
            // Run init tasks before (re)starting the service
            if let Some(init_tasks) = &svc.init
                && !init_tasks.is_empty()
                && let Err(err) = run_init_tasks_blocking(
                    tasks_map.clone(),
                    init_tasks.clone(),
                    project_dir.to_path_buf(),
                    run_id.clone(),
                )
                .await
            {
                eprintln!("[{svc_name}] init failed: {err}");
                let service_event = {
                    let mut guard = state.state.lock().await;
                    if let Some(run) = guard.runs.get_mut(run_id.as_str()) {
                        if let Some(runtime) = run.services.get_mut(svc_name) {
                            apply_prepared_to_runtime(runtime, &prepared, true);
                            let event = set_service_state(
                                run_id.as_str(),
                                svc_name,
                                runtime,
                                ServiceState::Failed,
                            )
                            .or_else(|| {
                                Some(service_state_changed_event(
                                    run_id.as_str(),
                                    svc_name,
                                    ServiceState::Failed,
                                ))
                            });
                            runtime.last_failure = Some(format!("init task failed: {err}"));
                            event
                        } else {
                            let runtime = ServiceRuntime {
                                name: prepared.name.clone(),
                                unit_name: prepared.unit_name.clone(),
                                port: prepared.port,
                                scheme: prepared.scheme.clone(),
                                url: prepared.url.clone(),
                                deps: prepared.deps.clone(),
                                readiness: prepared.readiness.clone(),
                                log_path: prepared.log_path.clone(),
                                cwd: prepared.cwd.clone(),
                                env: prepared.env.clone(),
                                state: ServiceState::Failed,
                                last_failure: Some(format!("init task failed: {err}")),
                                health: None,
                                last_started_at: None,
                                watch_hash: Some(prepared.watch_hash.clone()),
                                watch_patterns: prepared.watch_patterns.clone(),
                                ignore_patterns: prepared.ignore_patterns.clone(),
                                watch_extra_files: prepared.watch_extra_files.clone(),
                                watch_fingerprint: prepared.watch_fingerprint.clone(),
                                auto_restart: prepared.auto_restart,
                                watch_paused: false,
                                watch_handle: None,
                            };
                            run.services.insert(svc_name.clone(), runtime);
                            Some(service_state_changed_event(
                                run_id.as_str(),
                                svc_name,
                                ServiceState::Failed,
                            ))
                        }
                    } else {
                        None
                    }
                };
                if let Some(event) = service_event {
                    emit_event(state, event);
                }
                let _ = sync_service_auto_restart_watcher(state, run_id.as_str(), svc_name).await;
                continue;
            }

            let restart_existing = existing.contains_key(svc_name);
            let start_result =
                start_prepared_service(state, &run_id, &prepared, restart_existing).await;

            let (initial_state, failure_reason, last_started_at) = match &start_result {
                Ok(()) => (ServiceState::Starting, None, Some(now_rfc3339())),
                Err(err) => (ServiceState::Failed, Some(err.to_string()), None),
            };

            let service_event = {
                let mut guard = state.state.lock().await;
                if let Some(run) = guard.runs.get_mut(run_id.as_str()) {
                    if let Some(runtime) = run.services.get_mut(svc_name) {
                        let previous_state = runtime.state.clone();
                        apply_prepared_to_runtime(runtime, &prepared, true);
                        runtime.state = initial_state.clone();
                        runtime.last_failure = failure_reason.clone();
                        runtime.last_started_at = last_started_at.clone();
                        (previous_state != initial_state).then(|| {
                            service_state_changed_event(
                                run_id.as_str(),
                                svc_name,
                                initial_state.clone(),
                            )
                        })
                    } else {
                        let runtime = ServiceRuntime {
                            name: prepared.name.clone(),
                            unit_name: prepared.unit_name.clone(),
                            port: prepared.port,
                            scheme: prepared.scheme.clone(),
                            url: prepared.url.clone(),
                            deps: prepared.deps.clone(),
                            readiness: prepared.readiness.clone(),
                            log_path: prepared.log_path.clone(),
                            cwd: prepared.cwd.clone(),
                            env: prepared.env.clone(),
                            state: initial_state.clone(),
                            last_failure: failure_reason,
                            health: None,
                            last_started_at,
                            watch_hash: Some(prepared.watch_hash.clone()),
                            watch_patterns: prepared.watch_patterns.clone(),
                            ignore_patterns: prepared.ignore_patterns.clone(),
                            watch_extra_files: prepared.watch_extra_files.clone(),
                            watch_fingerprint: prepared.watch_fingerprint.clone(),
                            auto_restart: prepared.auto_restart,
                            watch_paused: false,
                            watch_handle: None,
                        };
                        run.services.insert(svc_name.clone(), runtime);
                        Some(service_state_changed_event(
                            run_id.as_str(),
                            svc_name,
                            initial_state.clone(),
                        ))
                    }
                } else {
                    None
                }
            };
            if let Some(event) = service_event {
                emit_event(state, event);
            }

            if let Err(err) =
                sync_service_auto_restart_watcher(state, run_id.as_str(), svc_name).await
            {
                eprintln!("devstack: failed to start watcher for {svc_name}: {err}");
            }

            // Only proceed with readiness check if service started successfully
            if start_result.is_ok()
                && let Err(err) = handle_readiness(
                    state.clone(),
                    run_id.as_str(),
                    &prepared,
                    no_wait,
                    build_post_init_context(svc, &tasks_map, project_dir, &run_id),
                )
                .await
            {
                persist_manifest_on_error(state, run_id.as_str()).await;
                return Err(err);
            }
        } else {
            let mut guard = state.state.lock().await;
            if let Some(run) = guard.runs.get_mut(run_id.as_str())
                && let Some(runtime) = run.services.get_mut(svc_name)
            {
                apply_prepared_to_runtime(runtime, &prepared, false);
            }
            drop(guard);
            if let Err(err) =
                sync_service_auto_restart_watcher(state, run_id.as_str(), svc_name).await
            {
                eprintln!("devstack: failed to sync watcher for {svc_name}: {err}");
            }
        }
    }

    let run_event = {
        let mut guard = state.state.lock().await;
        guard
            .runs
            .get_mut(run_id.as_str())
            .and_then(recompute_run_state)
    };
    if let Some(event) = run_event {
        emit_event(state, event);
    }

    let manifest = persist_manifest(state, run_id.as_str())
        .await
        .map_err(AppError::from)?;
    Ok(manifest)
}

fn resolve_ports_for_refresh(
    services: &BTreeMap<String, ServiceConfig>,
    existing: &BTreeMap<String, ExistingServiceSnapshot>,
    reuse_ports: bool,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut port_map = BTreeMap::new();
    let mut needs_alloc = BTreeMap::new();

    for (name, svc) in services {
        let port = match &svc.port {
            Some(config) if config.is_none() => None,
            Some(crate::config::PortConfig::Fixed(value)) => {
                let existing_port = existing.get(name).and_then(|svc| svc.port);
                if existing_port != Some(*value) {
                    crate::port::ensure_available(*value)?;
                }
                Some(*value)
            }
            Some(crate::config::PortConfig::None(_)) => None,
            None => {
                if reuse_ports {
                    if let Some(existing_port) = existing.get(name).and_then(|svc| svc.port) {
                        Some(existing_port)
                    } else {
                        needs_alloc.insert(name.clone(), svc.clone());
                        None
                    }
                } else {
                    needs_alloc.insert(name.clone(), svc.clone());
                    None
                }
            }
        };
        port_map.insert(name.clone(), port);
    }

    if !needs_alloc.is_empty() {
        let allocated = allocate_ports(&needs_alloc)?;
        for (name, port) in allocated {
            port_map.insert(name, port);
        }
    }

    Ok(port_map)
}

fn build_base_env(
    run_id: &RunId,
    stack: &str,
    project_dir: &Path,
    port_map: &BTreeMap<String, Option<u16>>,
    schemes: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    env.insert("DEV_RUN_ID".to_string(), run_id.as_str().to_string());
    env.insert("DEV_STACK".to_string(), stack.to_string());
    env.insert(
        "DEV_PROJECT_DIR".to_string(),
        project_dir.to_string_lossy().to_string(),
    );

    for (service, port) in port_map {
        if let Some(port) = port {
            let key = sanitize_env_key(service);
            env.insert(format!("DEV_PORT_{key}"), port.to_string());
            let scheme = schemes
                .get(service)
                .cloned()
                .unwrap_or_else(|| "http".to_string());
            env.insert(format!("DEV_URL_{key}"), readiness_url(&scheme, *port));
        }
    }

    Ok(env)
}

fn inject_dep_env(
    env: &mut BTreeMap<String, String>,
    svc: &ServiceConfig,
    port_map: &BTreeMap<String, Option<u16>>,
    schemes: &BTreeMap<String, String>,
) {
    for dep in &svc.deps {
        if let Some(Some(port)) = port_map.get(dep) {
            let key = sanitize_env_key(dep);
            env.insert(format!("DEV_DEP_{key}_PORT"), port.to_string());
            let scheme = schemes
                .get(dep)
                .cloned()
                .unwrap_or_else(|| "http".to_string());
            env.insert(format!("DEV_DEP_{key}_URL"), readiness_url(&scheme, *port));
        }
    }
}

fn build_template_context(
    run_id: &RunId,
    stack: &str,
    project_dir: &Path,
    port_map: &BTreeMap<String, Option<u16>>,
    schemes: &BTreeMap<String, String>,
) -> Result<serde_json::Value> {
    let mut services = serde_json::Map::new();
    for (service, port) in port_map {
        let mut entry = serde_json::Map::new();
        if let Some(port) = port {
            entry.insert("port".to_string(), serde_json::json!(port));
            let scheme = schemes
                .get(service)
                .cloned()
                .unwrap_or_else(|| "http".to_string());
            entry.insert(
                "url".to_string(),
                serde_json::json!(readiness_url(&scheme, *port)),
            );
        } else {
            entry.insert("port".to_string(), serde_json::Value::Null);
            entry.insert("url".to_string(), serde_json::Value::Null);
        }
        services.insert(service.clone(), serde_json::Value::Object(entry));
    }

    Ok(serde_json::json!({
        "run": { "id": run_id.as_str() },
        "project": { "dir": project_dir.to_string_lossy() },
        "stack": { "name": stack },
        "services": services,
    }))
}

fn render_env(
    env: &BTreeMap<String, String>,
    ctx: &serde_json::Value,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (key, value) in env {
        let rendered = render_template(value, ctx)?;
        out.insert(key.clone(), rendered);
    }
    Ok(out)
}

fn render_template(template: &str, ctx: &serde_json::Value) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.set_trim_blocks(true);
    let tmpl = env.template_from_str(template)?;
    let rendered = tmpl.render(ctx)?;
    Ok(rendered)
}

fn render_patterns(patterns: &[String], ctx: &serde_json::Value) -> Result<Vec<String>> {
    let mut rendered = Vec::new();
    for pattern in patterns {
        rendered.push(render_template(pattern, ctx)?);
    }
    Ok(rendered)
}

fn resolve_rendered_path(template: &str, ctx: &serde_json::Value) -> Result<PathBuf> {
    let rendered = render_template(template, ctx)?;
    let path = PathBuf::from(rendered);
    Ok(expand_home(&path))
}

fn resolve_cwd_path(template: &str, ctx: &serde_json::Value, base_dir: &Path) -> Result<PathBuf> {
    let rendered = resolve_rendered_path(template, ctx)?;
    if rendered.is_absolute() {
        Ok(rendered)
    } else {
        Ok(base_dir.join(rendered))
    }
}

fn resolve_env_file_path(
    svc: &ServiceConfig,
    cwd: &Path,
    ctx: &serde_json::Value,
) -> Result<PathBuf> {
    if let Some(env_file) = &svc.env_file {
        let rendered = resolve_rendered_path(&env_file.to_string_lossy(), ctx)?;
        if rendered.is_absolute() {
            return Ok(rendered);
        }
        return Ok(cwd.join(rendered));
    }
    Ok(cwd.join(".env"))
}

fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let iter = dotenvy::from_path_iter(path)
        .with_context(|| format!("read env file {}", path.to_string_lossy()))?;
    let mut env = BTreeMap::new();
    for item in iter {
        let (key, value) = item?;
        env.insert(key, value);
    }
    Ok(env)
}

fn merge_env_file(into: &mut BTreeMap<String, String>, file_env: BTreeMap<String, String>) {
    for (key, value) in file_env {
        if key.starts_with("DEV_") {
            continue;
        }
        into.insert(key, value);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_watch_fingerprint(
    svc: &ServiceConfig,
    rendered_cmd: &str,
    rendered_cwd: &Path,
    port: Option<u16>,
    scheme: &str,
    readiness: &ReadinessSpec,
    env_file_path: &Path,
    env: &BTreeMap<String, String>,
    watch: &[String],
    ignore: &[String],
) -> Result<Vec<u8>> {
    let payload = serde_json::json!({
        "cmd": rendered_cmd,
        "cwd": rendered_cwd.to_string_lossy(),
        "port": port,
        "scheme": scheme,
        "deps": svc.deps,
        "port_env": svc.port_env(),
        "readiness": format!("{:?}", readiness),
        "env_file": env_file_path.to_string_lossy(),
        "env": env,
        "watch": watch,
        "ignore": ignore,
    });
    let bytes = serde_json::to_vec(&payload)?;
    Ok(bytes)
}

fn stop_health_monitor_for_service(svc: &mut ServiceRuntime) {
    if let Some(health) = svc.health.take() {
        health.stop_flag.store(true, Ordering::SeqCst);
    }
}

fn stop_watch_for_service(svc: &mut ServiceRuntime) {
    if let Some(handle) = svc.watch_handle.take() {
        handle.stop_flag.store(true, Ordering::SeqCst);
    }
}

fn apply_prepared_to_runtime(
    svc: &mut ServiceRuntime,
    prepared: &PreparedService,
    reset_state: bool,
) {
    if reset_state {
        stop_health_monitor_for_service(svc);
        stop_watch_for_service(svc);
        svc.state = ServiceState::Starting;
        svc.last_failure = None;
    }
    svc.unit_name = prepared.unit_name.clone();
    svc.port = prepared.port;
    svc.scheme = prepared.scheme.clone();
    svc.url = prepared.url.clone();
    svc.deps = prepared.deps.clone();
    svc.readiness = prepared.readiness.clone();
    svc.log_path = prepared.log_path.clone();
    svc.cwd = prepared.cwd.clone();
    svc.env = prepared.env.clone();
    svc.watch_hash = Some(prepared.watch_hash.clone());
    svc.watch_patterns = prepared.watch_patterns.clone();
    svc.ignore_patterns = prepared.ignore_patterns.clone();
    svc.watch_extra_files = prepared.watch_extra_files.clone();
    svc.watch_fingerprint = prepared.watch_fingerprint.clone();
    if svc.auto_restart != prepared.auto_restart {
        svc.watch_paused = false;
    }
    svc.auto_restart = prepared.auto_restart;
}

#[allow(clippy::too_many_arguments)]
fn spawn_service_auto_restart_watcher(
    state: AppState,
    run_id: String,
    service: String,
    cwd: PathBuf,
    watch_patterns: Vec<String>,
    ignore_patterns: Vec<String>,
    watch_extra_files: Vec<PathBuf>,
    watch_fingerprint: Vec<u8>,
    paused: bool,
) -> Result<ServiceWatchHandle> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = event_tx.send(event);
    })
    .context("create filesystem watcher")?;
    watcher
        .watch(&cwd, RecursiveMode::Recursive)
        .with_context(|| format!("watch directory {}", cwd.to_string_lossy()))?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let paused_flag = Arc::new(AtomicBool::new(paused));
    let stop_flag_task = stop_flag.clone();
    let paused_flag_task = paused_flag.clone();

    tokio::spawn(async move {
        let _watcher = watcher;
        let debounce = Duration::from_millis(500);
        let mut pending = false;
        let mut last_event_at = Instant::now();

        loop {
            if stop_flag_task.load(Ordering::SeqCst) {
                break;
            }

            match tokio::time::timeout(Duration::from_millis(100), event_rx.recv()).await {
                Ok(Some(Ok(event))) => {
                    if matches!(
                        event.kind,
                        EventKind::Any
                            | EventKind::Create(_)
                            | EventKind::Modify(_)
                            | EventKind::Remove(_)
                    ) {
                        pending = true;
                        last_event_at = Instant::now();
                    }
                }
                Ok(Some(Err(err))) => {
                    eprintln!("devstack: watch error for {}.{}: {}", run_id, service, err);
                }
                Ok(None) => break,
                Err(_) => {}
            }

            if !pending || last_event_at.elapsed() < debounce {
                continue;
            }
            pending = false;

            if paused_flag_task.load(Ordering::SeqCst) {
                continue;
            }

            let watch_patterns = if watch_patterns.is_empty() {
                None
            } else {
                Some(watch_patterns.as_slice())
            };
            let next_hash = match compute_watch_hash(
                &cwd,
                watch_patterns,
                &ignore_patterns,
                &watch_extra_files,
                &watch_fingerprint,
            ) {
                Ok(hash) => hash,
                Err(err) => {
                    eprintln!(
                        "devstack: failed to compute watch hash for {}.{}: {}",
                        run_id, service, err
                    );
                    continue;
                }
            };

            let should_restart = {
                let mut guard = state.state.lock().await;
                let Some(run) = guard.runs.get_mut(&run_id) else {
                    break;
                };
                let Some(svc) = run.services.get_mut(&service) else {
                    break;
                };
                paused_flag_task.store(svc.watch_paused, Ordering::SeqCst);
                if !svc.auto_restart
                    || svc.watch_paused
                    || svc.watch_hash.as_deref() == Some(next_hash.as_str())
                {
                    false
                } else {
                    svc.watch_hash = Some(next_hash.clone());
                    true
                }
            };

            if should_restart
                && let Err(err) = orchestrate_restart_service(&state, &run_id, &service, true).await
            {
                eprintln!(
                    "devstack: auto-restart failed for {}.{}: {:?}",
                    run_id, service, err
                );
            }
        }
    });

    Ok(ServiceWatchHandle {
        stop_flag,
        paused: paused_flag,
    })
}

async fn sync_service_auto_restart_watcher(
    state: &AppState,
    run_id: &str,
    service: &str,
) -> Result<()> {
    let start_args: Option<WatchStartArgs> = {
        let mut guard = state.state.lock().await;
        let Some(run) = guard.runs.get_mut(run_id) else {
            return Ok(());
        };
        let Some(svc) = run.services.get_mut(service) else {
            return Ok(());
        };

        if !svc.auto_restart || svc.last_started_at.is_none() {
            stop_watch_for_service(svc);
            return Ok(());
        }

        if let Some(handle) = svc.watch_handle.as_ref() {
            handle.paused.store(svc.watch_paused, Ordering::SeqCst);
            return Ok(());
        }

        Some((
            svc.cwd.clone(),
            svc.watch_patterns.clone(),
            svc.ignore_patterns.clone(),
            svc.watch_extra_files.clone(),
            svc.watch_fingerprint.clone(),
            svc.watch_paused,
        ))
    };

    if let Some((
        cwd,
        watch_patterns,
        ignore_patterns,
        watch_extra_files,
        watch_fingerprint,
        paused,
    )) = start_args
    {
        let handle = spawn_service_auto_restart_watcher(
            state.clone(),
            run_id.to_string(),
            service.to_string(),
            cwd,
            watch_patterns,
            ignore_patterns,
            watch_extra_files,
            watch_fingerprint,
            paused,
        )?;

        let mut guard = state.state.lock().await;
        if let Some(run) = guard.runs.get_mut(run_id)
            && let Some(svc) = run.services.get_mut(service)
        {
            if svc.auto_restart && svc.last_started_at.is_some() && svc.watch_handle.is_none() {
                handle.paused.store(svc.watch_paused, Ordering::SeqCst);
                svc.watch_handle = Some(handle);
            } else {
                handle.stop_flag.store(true, Ordering::SeqCst);
            }
        } else {
            handle.stop_flag.store(true, Ordering::SeqCst);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_service(
    run_id: &RunId,
    stack: &str,
    project_dir: &Path,
    config_dir: &Path,
    svc_name: &str,
    svc: &ServiceConfig,
    port_map: &BTreeMap<String, Option<u16>>,
    service_schemes: &BTreeMap<String, String>,
    base_env: &BTreeMap<String, String>,
) -> Result<PreparedService> {
    let port = *port_map.get(svc_name).unwrap_or(&None);
    let scheme = svc.scheme();
    let url = port.map(|p| readiness_url(&scheme, p));

    let tmpl_context =
        build_template_context(run_id, stack, project_dir, port_map, service_schemes)?;
    let rendered_cwd = resolve_cwd_path(
        &svc.cwd_or(project_dir).to_string_lossy(),
        &tmpl_context,
        config_dir,
    )?;
    let env_file_path = resolve_env_file_path(svc, &rendered_cwd, &tmpl_context)?;

    let mut env = base_env.clone();
    let file_env = load_env_file(&env_file_path)?;
    merge_env_file(&mut env, file_env);
    inject_dep_env(&mut env, svc, port_map, service_schemes);
    if let Some(port) = port {
        env.insert(svc.port_env(), port.to_string());
    }
    let rendered_env = render_env(&svc.env, &tmpl_context)?;
    env.extend(rendered_env);
    // Resolve $VAR and ${VAR} references from process environment
    env = crate::config::resolve_env_map(&env);
    env.insert("DEV_GRACE_MS".to_string(), "2000".to_string());

    let readiness = svc.readiness_spec(port.is_some())?;
    let unit_name = unit_name_for_run(run_id.as_str(), svc_name);
    let log_path = paths::run_log_path(run_id, &ServiceName::new(svc_name.to_string()))?;
    let cmd = render_template(&svc.cmd, &tmpl_context)?;

    let rendered_watch = render_patterns(&svc.watch, &tmpl_context)?;
    let rendered_ignore = render_patterns(&svc.ignore, &tmpl_context)?;
    if svc.auto_restart
        && rendered_watch
            .iter()
            .all(|pattern| pattern.trim().is_empty())
    {
        return Err(anyhow!(
            "service {svc_name} sets auto_restart=true but has no watch patterns"
        ));
    }
    let fingerprint = build_watch_fingerprint(
        svc,
        &cmd,
        &rendered_cwd,
        port,
        &scheme,
        &readiness,
        &env_file_path,
        &env,
        &rendered_watch,
        &rendered_ignore,
    )?;
    let watch_patterns = if rendered_watch.is_empty() {
        None
    } else {
        Some(rendered_watch.as_slice())
    };
    let watch_extra_files = vec![env_file_path];
    let watch_hash = compute_watch_hash(
        &rendered_cwd,
        watch_patterns,
        &rendered_ignore,
        &watch_extra_files,
        &fingerprint,
    )?;

    Ok(PreparedService {
        name: svc_name.to_string(),
        unit_name,
        port,
        scheme,
        url,
        deps: svc.deps.clone(),
        readiness,
        log_path,
        cwd: rendered_cwd,
        env,
        cmd,
        watch_hash,
        watch_patterns: rendered_watch,
        ignore_patterns: rendered_ignore,
        watch_extra_files,
        watch_fingerprint: fingerprint,
        auto_restart: svc.auto_restart,
    })
}

async fn start_prepared_service(
    state: &AppState,
    run_id: &RunId,
    prepared: &PreparedService,
    restart_existing: bool,
) -> Result<()> {
    if restart_existing {
        let _ = state.systemd.stop_unit(&prepared.unit_name).await;
        for _ in 0..50 {
            match state.systemd.unit_status(&prepared.unit_name).await {
                Ok(Some(status)) => {
                    let state = status.active_state.as_str();
                    if matches!(
                        state,
                        "active" | "activating" | "deactivating" | "reloading" | "maintenance"
                    ) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
            break;
        }
    }
    let binary = state.binary_path.to_string_lossy().to_string();
    let exec = ExecStart {
        path: binary.clone(),
        argv: vec![
            binary.clone(),
            "__shim".to_string(),
            "--run-id".to_string(),
            run_id.as_str().to_string(),
            "--service".to_string(),
            prepared.name.clone(),
            "--cmd".to_string(),
            prepared.cmd.clone(),
            "--cwd".to_string(),
            prepared.cwd.to_string_lossy().to_string(),
            "--log-file".to_string(),
            prepared.log_path.to_string_lossy().to_string(),
        ],
        ignore_failure: false,
    };
    // Daemon-managed restarts must always flow back through readiness + post_init.
    // Let the daemon decide when to restart a service instead of having systemd
    // do an out-of-band on-failure restart.
    let properties = UnitProperties::new(
        format!("devstack {} {}", run_id.as_str(), prepared.name),
        &prepared.cwd,
        prepared
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect(),
        exec,
    )
    .with_restart("no")
    .with_remain_after_exit(matches!(prepared.readiness.kind, ReadinessKind::Exit));

    state
        .systemd
        .start_transient_service(&prepared.unit_name, properties)
        .await
        .with_context(|| format!("start unit {}", prepared.unit_name))?;
    Ok(())
}

fn format_terminal_unit_status(status: &crate::systemd::UnitStatus) -> Option<String> {
    let failed = status.active_state == "failed"
        || (status.active_state == "inactive"
            && status
                .result
                .as_deref()
                .map(|r| r != "success")
                .unwrap_or(false));
    if !failed {
        return None;
    }

    let result = status
        .result
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    Some(format!(
        "exited before readiness (active_state={}, sub_state={}, result={result})",
        status.active_state, status.sub_state
    ))
}

fn tail_log_messages(log_path: &Path, limit: usize) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(log_path) else {
        return Vec::new();
    };

    content
        .lines()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(|line| {
            let (_, message) = extract_log_content(line);
            let trimmed = message.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

async fn enrich_readiness_error(
    state: &AppState,
    service: &str,
    unit_name: &str,
    log_path: &Path,
    err: anyhow::Error,
) -> anyhow::Error {
    let mut message = err.to_string();

    if let Ok(Some(status)) = state.systemd.unit_status(unit_name).await
        && let Some(reason) = format_terminal_unit_status(&status)
    {
        message = format!("service '{service}' {reason}");
    }

    let recent_logs = tail_log_messages(log_path, 10);
    if !recent_logs.is_empty() {
        message.push_str("\nlast log lines:\n");
        message.push_str(&recent_logs.join("\n"));
    }

    anyhow!(message)
}

struct PostInitContext {
    tasks_map: BTreeMap<String, TaskConfig>,
    post_init_tasks: Vec<String>,
    project_dir: PathBuf,
    run_id: RunId,
}

fn build_post_init_context(
    svc: &ServiceConfig,
    tasks_map: &BTreeMap<String, TaskConfig>,
    project_dir: &Path,
    run_id: &RunId,
) -> Option<PostInitContext> {
    let post_init = svc.post_init.as_ref()?;
    if post_init.is_empty() {
        return None;
    }
    Some(PostInitContext {
        tasks_map: tasks_map.clone(),
        post_init_tasks: post_init.clone(),
        project_dir: project_dir.to_path_buf(),
        run_id: run_id.clone(),
    })
}

fn load_post_init_context_for_run_service(
    run_id: &str,
    stack: &str,
    project_dir: &Path,
    service: &str,
) -> Result<Option<PostInitContext>> {
    let snapshot_path = paths::run_snapshot_path(&RunId::new(run_id))?;
    if !snapshot_path.exists() {
        return Ok(None);
    }

    let config = ConfigFile::load_from_path(&snapshot_path)?;
    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();
    let service_config = if stack == "globals" {
        config.globals_map().get(service).cloned()
    } else {
        config.stack_plan(stack)?.services.get(service).cloned()
    };

    Ok(service_config.and_then(|svc| {
        build_post_init_context(&svc, &tasks_map, project_dir, &RunId::new(run_id))
    }))
}

async fn handle_readiness(
    state: AppState,
    run_id: &str,
    prepared: &PreparedService,
    no_wait: bool,
    post_init: Option<PostInitContext>,
) -> AppResult<()> {
    if no_wait {
        spawn_readiness_task(
            state.clone(),
            run_id.to_string(),
            prepared.name.clone(),
            prepared.readiness.clone(),
            prepared.port,
            prepared.scheme.clone(),
            prepared.log_path.clone(),
            prepared.cwd.clone(),
            prepared.env.clone(),
            prepared.unit_name.clone(),
            post_init,
        );
        return Ok(());
    }

    let ctx = ReadinessContext {
        port: prepared.port,
        scheme: prepared.scheme.clone(),
        log_path: prepared.log_path.clone(),
        cwd: prepared.cwd.clone(),
        env: prepared.env.clone(),
        unit_name: Some(prepared.unit_name.clone()),
        systemd: Some(state.systemd.clone()),
    };
    match crate::readiness::wait_for_ready(&prepared.readiness, &ctx).await {
        Ok(()) => {
            if let Some(post_init) = post_init {
                if let Err(err) = run_post_init_tasks_blocking(
                    post_init.tasks_map,
                    post_init.post_init_tasks,
                    post_init.project_dir,
                    post_init.run_id,
                )
                .await
                {
                    let reason = format!("post_init task failed: {err}");
                    eprintln!("[{}] {reason}", prepared.name);
                    mark_service_failed(&state, run_id, &prepared.name, &reason)
                        .await
                        .map_err(AppError::from)?;
                    return Err(AppError::Internal(anyhow!(reason)));
                }
            }
            mark_service_ready(&state, run_id, &prepared.name)
                .await
                .map_err(AppError::from)?;
        }
        Err(err) => {
            let detailed = enrich_readiness_error(
                &state,
                &prepared.name,
                &prepared.unit_name,
                &prepared.log_path,
                err,
            )
            .await;
            mark_service_failed(&state, run_id, &prepared.name, &detailed.to_string())
                .await
                .map_err(AppError::from)?;
            return Err(AppError::Internal(detailed));
        }
    }
    Ok(())
}

fn generate_run_id(stack: &str) -> String {
    let mut rng = rand::rng();
    let suffix: String = (0..8)
        .map(|_| format!("{:x}", rng.random_range(0..16)))
        .collect();
    format!("{}-{}", stack, suffix)
}

fn unit_name_for_run(run_id: &str, service: &str) -> String {
    let run = sanitize_env_key(run_id);
    let svc = sanitize_env_key(service);
    format!("devstack-run-{run}-{svc}.service")
}

async fn mark_service_ready(state: &AppState, run_id: &str, service: &str) -> Result<()> {
    let (start_monitor, events) = {
        let mut guard = state.state.lock().await;
        let mut start_monitor = false;
        let mut events = Vec::new();
        if let Some(run) = guard.runs.get_mut(run_id) {
            if let Some(svc) = run.services.get_mut(service) {
                if let Some(event) = set_service_state(run_id, service, svc, ServiceState::Ready) {
                    events.push(event);
                }
                svc.last_failure = None;
                if svc.health.is_none() && !matches!(svc.readiness.kind, ReadinessKind::Exit) {
                    svc.health = Some(HealthHandle {
                        stop_flag: Arc::new(AtomicBool::new(false)),
                        stats: Arc::new(std::sync::Mutex::new(HealthSnapshot::default())),
                    });
                    start_monitor = true;
                }
            }
            if let Some(event) = recompute_run_state(run) {
                events.push(event);
            }
        }
        (start_monitor, events)
    };
    emit_events(state, events);
    if start_monitor {
        start_health_monitor(state.clone(), run_id.to_string(), service.to_string());
    }
    Ok(())
}

async fn mark_service_failed(
    state: &AppState,
    run_id: &str,
    service: &str,
    reason: &str,
) -> Result<()> {
    let events = {
        let mut guard = state.state.lock().await;
        let mut events = Vec::new();
        if let Some(run) = guard.runs.get_mut(run_id) {
            if let Some(svc) = run.services.get_mut(service) {
                if let Some(event) = set_service_state(run_id, service, svc, ServiceState::Failed) {
                    events.push(event);
                }
                svc.last_failure = Some(reason.to_string());
            }
            if let Some(event) = recompute_run_state(run) {
                events.push(event);
            }
        }
        events
    };
    emit_events(state, events);
    Ok(())
}

fn start_health_monitor(state: AppState, run_id: String, service: String) {
    tokio::spawn(async move {
        let (readiness, ctx, stop_flag, stats) = {
            let guard = state.state.lock().await;
            let run = match guard.runs.get(&run_id) {
                Some(run) => run,
                None => return,
            };
            let svc = match run.services.get(&service) {
                Some(svc) => svc,
                None => return,
            };
            let handle = match &svc.health {
                Some(h) => h,
                None => return,
            };
            let stop_flag = handle.stop_flag.clone();
            let stats = handle.stats.clone();
            let ctx = ReadinessContext {
                port: svc.port,
                scheme: svc.scheme.clone(),
                log_path: svc.log_path.clone(),
                cwd: svc.cwd.clone(),
                env: svc.env.clone(),
                unit_name: Some(svc.unit_name.clone()),
                systemd: Some(state.systemd.clone()),
            };
            (svc.readiness.clone(), ctx, stop_flag, stats)
        };

        // All bookkeeping lives in the task — the global DaemonState mutex is
        // only acquired for the rare svc.state transitions.
        let mut restart_count: u32 = 0;
        let mut consecutive_failures: u32 = 0;

        loop {
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }

            let ok = crate::readiness::check_ready_once(&readiness, &ctx)
                .await
                .unwrap_or(false);
            let checked_at = now_rfc3339();

            // Update stats through the per-service Arc — no global lock.
            if ok {
                consecutive_failures = 0;
                restart_count = 0;
            } else {
                consecutive_failures = consecutive_failures.saturating_add(1);
            }
            {
                let mut snap = stats.lock().unwrap_or_else(|e| e.into_inner());
                snap.last_check_at = Some(checked_at);
                snap.last_ok = Some(ok);
                snap.consecutive_failures = consecutive_failures;
                if ok {
                    snap.passes += 1;
                } else {
                    snap.failures += 1;
                }
            }

            // Determine if a state transition is needed.  Only then do we
            // touch the global DaemonState.
            enum Transition {
                Healthy,
                RecoverToReady,
                MarkDegraded,
            }
            let transition = if ok && consecutive_failures == 0 {
                Transition::RecoverToReady
            } else if consecutive_failures >= 3 {
                Transition::MarkDegraded
            } else {
                Transition::Healthy
            };

            let (events, should_restart) = match transition {
                Transition::Healthy => (Vec::new(), false),
                Transition::RecoverToReady => {
                    let mut guard = state.state.lock().await;
                    let mut events = Vec::new();
                    if let Some(run) = guard.runs.get_mut(&run_id) {
                        if let Some(svc) = run.services.get_mut(&service)
                            && svc.state == ServiceState::Degraded
                            && svc.last_failure.as_deref() == Some("health checks failing")
                        {
                            if let Some(event) =
                                set_service_state(&run_id, &service, svc, ServiceState::Ready)
                            {
                                events.push(event);
                            }
                            svc.last_failure = None;
                        }
                        if let Some(event) = recompute_run_state(run) {
                            events.push(event);
                        }
                    }
                    (events, false)
                }
                Transition::MarkDegraded => {
                    let mut guard = state.state.lock().await;
                    let mut events = Vec::new();
                    if let Some(run) = guard.runs.get_mut(&run_id) {
                        if let Some(svc) = run.services.get_mut(&service)
                            && svc.state == ServiceState::Ready
                        {
                            if let Some(event) =
                                set_service_state(&run_id, &service, svc, ServiceState::Degraded)
                            {
                                events.push(event);
                            }
                            svc.last_failure = Some("health checks failing".to_string());
                        }
                        if let Some(event) = recompute_run_state(run) {
                            events.push(event);
                        }
                    }
                    let needs_restart = !events.is_empty() && restart_count < 3;
                    (events, needs_restart)
                }
            };
            let changed = !events.is_empty();
            emit_events(&state, events);

            if changed && let Err(err) = persist_manifest(&state, &run_id).await {
                eprintln!(
                    "devstack: failed to persist manifest after health transition for {run_id}/{service}: {err}"
                );
            }

            if should_restart {
                restart_count += 1;

                // Backoff: 1st restart immediate, 2nd after 5s, 3rd after 30s
                let backoff = match restart_count {
                    1 => Duration::from_secs(0),
                    2 => Duration::from_secs(5),
                    _ => Duration::from_secs(30),
                };

                if backoff.as_secs() > 0 {
                    eprintln!(
                        "devstack: restarting {} in {}s (attempt {})",
                        service,
                        backoff.as_secs(),
                        restart_count
                    );
                    tokio::time::sleep(backoff).await;
                } else {
                    eprintln!(
                        "devstack: restarting {} (attempt {})",
                        service, restart_count
                    );
                }

                if let Err(err) = orchestrate_restart_service(&state, &run_id, &service, true).await
                {
                    eprintln!("devstack: failed to restart {}: {:?}", service, err);
                } else {
                    eprintln!("devstack: restarted service {}", service);
                }

                if restart_count >= 3 {
                    let events = {
                        let mut guard = state.state.lock().await;
                        let mut events = Vec::new();
                        if let Some(run) = guard.runs.get_mut(&run_id) {
                            if let Some(svc) = run.services.get_mut(&service) {
                                if let Some(event) =
                                    set_service_state(&run_id, &service, svc, ServiceState::Failed)
                                {
                                    events.push(event);
                                }
                                svc.last_failure =
                                    Some("health restart limit exceeded".to_string());
                            }
                            if let Some(event) = recompute_run_state(run) {
                                events.push(event);
                            }
                        }
                        events
                    };
                    emit_events(&state, events);
                    if let Err(err) = persist_manifest(&state, &run_id).await {
                        eprintln!(
                            "devstack: failed to persist manifest after restart limit for {run_id}/{service}: {err}"
                        );
                    }
                    break;
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_readiness_task(
    state: AppState,
    run_id: String,
    service: String,
    readiness: ReadinessSpec,
    port: Option<u16>,
    scheme: String,
    log_path: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    unit_name: String,
    post_init: Option<PostInitContext>,
) {
    tokio::spawn(async move {
        let ctx = ReadinessContext {
            port,
            scheme,
            log_path: log_path.clone(),
            cwd,
            env,
            unit_name: Some(unit_name.clone()),
            systemd: Some(state.systemd.clone()),
        };
        match crate::readiness::wait_for_ready(&readiness, &ctx).await {
            Ok(()) => {
                if let Some(post_init) = post_init {
                    if let Err(err) = run_post_init_tasks_blocking(
                        post_init.tasks_map,
                        post_init.post_init_tasks,
                        post_init.project_dir,
                        post_init.run_id,
                    )
                    .await
                    {
                        let reason = format!("post_init task failed: {err}");
                        eprintln!("[{service}] {reason}");
                        if let Err(mark_err) =
                            mark_service_failed(&state, &run_id, &service, &reason).await
                        {
                            eprintln!(
                                "devstack: failed to mark service {service} failed for run {run_id}: {mark_err}"
                            );
                        }
                        if let Err(err) = persist_manifest(&state, &run_id).await {
                            eprintln!(
                                "devstack: failed to persist manifest after post_init failure for {run_id}/{service}: {err}"
                            );
                        }
                        return;
                    }
                }
                if let Err(err) = mark_service_ready(&state, &run_id, &service).await {
                    eprintln!(
                        "devstack: failed to mark service {service} ready for run {run_id}: {err}"
                    );
                }
                if let Err(err) = persist_manifest(&state, &run_id).await {
                    eprintln!(
                        "devstack: failed to persist manifest after readiness success for {run_id}/{service}: {err}"
                    );
                }
            }
            Err(err) => {
                let detailed =
                    enrich_readiness_error(&state, &service, &unit_name, &log_path, err).await;
                if let Err(mark_err) =
                    mark_service_failed(&state, &run_id, &service, &detailed.to_string()).await
                {
                    eprintln!(
                        "devstack: failed to mark service {service} failed for run {run_id}: {mark_err}"
                    );
                }
                if let Err(err) = persist_manifest(&state, &run_id).await {
                    eprintln!(
                        "devstack: failed to persist manifest after readiness failure for {run_id}/{service}: {err}"
                    );
                }
            }
        }
    });
}

async fn persist_manifest(state: &AppState, run_id: &str) -> Result<RunManifest> {
    let (manifest, path) = {
        let guard = state.state.lock().await;
        let run = guard
            .runs
            .get(run_id)
            .ok_or_else(|| anyhow!("run {run_id} not found"))?;
        let services = run
            .services
            .iter()
            .map(|(name, svc)| {
                (
                    name.clone(),
                    ServiceManifest {
                        port: svc.port,
                        url: svc.url.clone(),
                        state: svc.state.clone(),
                        watch_hash: svc.watch_hash.clone(),
                    },
                )
            })
            .collect();
        let manifest = RunManifest {
            run_id: run.run_id.clone(),
            project_dir: run.project_dir.to_string_lossy().to_string(),
            stack: run.stack.clone(),
            manifest_path: paths::run_manifest_path(&RunId::new(run.run_id.clone()))?
                .to_string_lossy()
                .to_string(),
            services,
            env: run.base_env.clone(),
            state: run.state.clone(),
            created_at: run.created_at.clone(),
            stopped_at: run.stopped_at.clone(),
        };
        let path = paths::run_manifest_path(&RunId::new(run.run_id.clone()))?;
        (manifest, path)
    };

    manifest.write_to_path(&path)?;
    write_daemon_state(state).await.ok();
    Ok(manifest)
}

async fn write_daemon_state(state: &AppState) -> Result<()> {
    let runs = {
        let guard = state.state.lock().await;
        guard.runs.keys().cloned().collect()
    };
    let state_file = DaemonStateFile {
        runs,
        updated_at: now_rfc3339(),
    };
    let data = serde_json::to_vec_pretty(&state_file)?;
    let path = paths::daemon_state_path()?;
    atomic_write(&path, &data)?;
    Ok(())
}

async fn orchestrate_down(state: &AppState, run_id: &str, purge: bool) -> AppResult<RunManifest> {
    stop_health_monitors(state, run_id).await;
    stop_watchers(state, run_id).await;
    let services: Vec<String> = {
        let guard = state.state.lock().await;
        guard
            .runs
            .get(run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?
            .services
            .keys()
            .cloned()
            .collect()
    };

    for svc in &services {
        let unit_name = unit_name_for_run(run_id, svc);
        let _ = state.systemd.stop_unit(&unit_name).await;
    }

    let events = {
        let mut guard = state.state.lock().await;
        let mut events = Vec::new();
        if let Some(run) = guard.runs.get_mut(run_id) {
            run.state = RunLifecycle::Stopped;
            run.stopped_at = Some(now_rfc3339());
            for (service_name, svc) in &mut run.services {
                if let Some(event) =
                    set_service_state(run_id, service_name, svc, ServiceState::Stopped)
                {
                    events.push(event);
                }
            }
            events.push(run_state_changed_event(run));
        }
        events
    };
    emit_events(state, events);

    let manifest = persist_manifest(state, run_id)
        .await
        .map_err(AppError::from)?;
    if purge {
        let run_dir = paths::run_dir(&RunId::new(run_id)).map_err(AppError::from)?;
        let _ = std::fs::remove_dir_all(run_dir);
        let removed = {
            let mut guard = state.state.lock().await;
            guard.runs.remove(run_id).is_some()
        };
        if removed {
            emit_event(state, run_removed_event(run_id));
        }
        // Best-effort: remove derived index entries for this run.
        let index = state.log_index.clone();
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || index.delete_run(&run_id))
            .await
            .ok();
        write_daemon_state(state).await.ok();
    }
    Ok(manifest)
}

async fn orchestrate_kill(state: &AppState, run_id: &str) -> AppResult<RunManifest> {
    stop_health_monitors(state, run_id).await;
    stop_watchers(state, run_id).await;
    let services: Vec<String> = {
        let guard = state.state.lock().await;
        guard
            .runs
            .get(run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?
            .services
            .keys()
            .cloned()
            .collect()
    };

    for svc in &services {
        let unit_name = unit_name_for_run(run_id, svc);
        let _ = state.systemd.kill_unit(&unit_name, 9).await;
        let _ = state.systemd.stop_unit(&unit_name).await;
    }

    let events = {
        let mut guard = state.state.lock().await;
        let mut events = Vec::new();
        if let Some(run) = guard.runs.get_mut(run_id) {
            run.state = RunLifecycle::Stopped;
            run.stopped_at = Some(now_rfc3339());
            for (service_name, svc) in &mut run.services {
                if let Some(event) =
                    set_service_state(run_id, service_name, svc, ServiceState::Stopped)
                {
                    events.push(event);
                }
            }
            events.push(run_state_changed_event(run));
        }
        events
    };
    emit_events(state, events);

    let manifest = persist_manifest(state, run_id)
        .await
        .map_err(AppError::from)?;
    Ok(manifest)
}

async fn orchestrate_restart_service(
    state: &AppState,
    run_id: &str,
    service: &str,
    no_wait: bool,
) -> AppResult<RunManifest> {
    let (unit_name, readiness, port, scheme, log_path, cwd, env, stack, project_dir, events) = {
        let mut guard = state.state.lock().await;
        let run = guard
            .runs
            .get_mut(run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;
        let mut events = Vec::new();
        let stack = run.stack.clone();
        let project_dir = run.project_dir.clone();
        let (unit_name, readiness, port, scheme, log_path, cwd, env) = {
            let svc = run
                .services
                .get_mut(service)
                .ok_or_else(|| AppError::not_found(format!("service {service} not found")))?;
            if let Some(event) = set_service_state(run_id, service, svc, ServiceState::Starting) {
                events.push(event);
            }
            svc.last_failure = None;
            (
                svc.unit_name.clone(),
                svc.readiness.clone(),
                svc.port,
                svc.scheme.clone(),
                svc.log_path.clone(),
                svc.cwd.clone(),
                svc.env.clone(),
            )
        };
        if let Some(event) = recompute_run_state(run) {
            events.push(event);
        }
        (
            unit_name,
            readiness,
            port,
            scheme,
            log_path,
            cwd,
            env,
            stack,
            project_dir,
            events,
        )
    };
    emit_events(state, events);

    let post_init = load_post_init_context_for_run_service(run_id, &stack, &project_dir, service)
        .map_err(AppError::from)?;

    state
        .systemd
        .restart_unit(&unit_name)
        .await
        .map_err(AppError::from)?;

    {
        let mut guard = state.state.lock().await;
        if let Some(run) = guard.runs.get_mut(run_id)
            && let Some(svc) = run.services.get_mut(service)
        {
            svc.last_started_at = Some(now_rfc3339());
        }
    }

    if let Err(err) = sync_service_auto_restart_watcher(state, run_id, service).await {
        eprintln!("devstack: failed to sync watcher for {service}: {err}");
    }

    if no_wait {
        spawn_readiness_task(
            state.clone(),
            run_id.to_string(),
            service.to_string(),
            readiness,
            port,
            scheme,
            log_path,
            cwd,
            env,
            unit_name.clone(),
            post_init,
        );
        return persist_manifest(state, run_id)
            .await
            .map_err(AppError::from);
    }

    let ctx = ReadinessContext {
        port,
        scheme,
        log_path: log_path.clone(),
        cwd,
        env,
        unit_name: Some(unit_name.clone()),
        systemd: Some(state.systemd.clone()),
    };
    match crate::readiness::wait_for_ready(&readiness, &ctx).await {
        Ok(()) => {
            if let Some(post_init) = post_init {
                if let Err(err) = run_post_init_tasks_blocking(
                    post_init.tasks_map,
                    post_init.post_init_tasks,
                    post_init.project_dir,
                    post_init.run_id,
                )
                .await
                {
                    let reason = format!("post_init task failed: {err}");
                    eprintln!("[{service}] {reason}");
                    mark_service_failed(state, run_id, service, &reason)
                        .await
                        .map_err(AppError::from)?;
                    persist_manifest_on_error(state, run_id).await;
                    return Err(AppError::Internal(anyhow!(reason)));
                }
            }
            mark_service_ready(state, run_id, service)
                .await
                .map_err(AppError::from)?;
        }
        Err(err) => {
            let detailed = enrich_readiness_error(state, service, &unit_name, &log_path, err).await;
            mark_service_failed(state, run_id, service, &detailed.to_string())
                .await
                .map_err(AppError::from)?;
            persist_manifest_on_error(state, run_id).await;
            return Err(AppError::Internal(detailed));
        }
    }
    persist_manifest(state, run_id)
        .await
        .map_err(AppError::from)
}

async fn stop_health_monitors(state: &AppState, run_id: &str) {
    let mut guard = state.state.lock().await;
    if let Some(run) = guard.runs.get_mut(run_id) {
        for svc in run.services.values_mut() {
            if let Some(health) = &svc.health {
                health.stop_flag.store(true, Ordering::SeqCst);
            }
        }
    }
}

async fn stop_watchers(state: &AppState, run_id: &str) {
    let mut guard = state.state.lock().await;
    if let Some(run) = guard.runs.get_mut(run_id) {
        for svc in run.services.values_mut() {
            stop_watch_for_service(svc);
        }
    }
}

fn same_project_dir(run_project_dir: &Path, project_dir: &Path) -> bool {
    if run_project_dir == project_dir {
        return true;
    }
    let run_canon =
        std::fs::canonicalize(run_project_dir).unwrap_or_else(|_| run_project_dir.to_path_buf());
    let project_canon =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    run_canon == project_canon
}

async fn find_latest_active_run_for_project(
    state: &AppState,
    project_dir: &Path,
) -> Result<Option<String>> {
    let mut candidates = Vec::new();
    {
        let guard = state.state.lock().await;
        for run in guard.runs.values() {
            if !same_project_dir(&run.project_dir, project_dir) {
                continue;
            }
            if run.state == RunLifecycle::Stopped || run.stopped_at.is_some() {
                continue;
            }
            candidates.push((run.created_at.clone(), run.run_id.clone()));
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(Some(candidates[0].1.clone()))
}

async fn find_latest_run_for_project_stack(
    state: &AppState,
    project_dir: &Path,
    stack: &str,
) -> Result<Option<String>> {
    let mut candidates = Vec::new();
    {
        let guard = state.state.lock().await;
        for run in guard.runs.values() {
            if run.stack != stack {
                continue;
            }
            if !same_project_dir(&run.project_dir, project_dir) {
                continue;
            }
            if run.state == RunLifecycle::Stopped || run.stopped_at.is_some() {
                continue;
            }
            candidates.push((run.created_at.clone(), run.run_id.clone()));
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(Some(candidates[0].1.clone()))
}

async fn read_service_logs(
    state: &AppState,
    run_id: &str,
    service: &str,
    query: LogsQuery,
) -> AppResult<LogsResponse> {
    let log_path = {
        let guard = state.state.lock().await;
        let run = guard
            .runs
            .get(run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;
        let svc = run
            .services
            .get(service)
            .ok_or_else(|| AppError::not_found(format!("service {service} not found")))?;
        svc.log_path.clone()
    };

    if !log_path.exists() {
        return Ok(LogsResponse {
            lines: vec![],
            truncated: false,
            total: 0,
            error_count: 0,
            warn_count: 0,
            next_after: None,
            matched_total: 0,
        });
    }

    let index = state.log_index.clone();
    let run_id = run_id.to_string();
    let service = service.to_string();
    let log_path = log_path.clone();
    let response = tokio::task::spawn_blocking(move || {
        index.search_service(&run_id, &service, log_path.as_path(), query)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow!("log search task failed: {e}")))?
    .map_err(map_log_index_error)?;

    Ok(response)
}

fn map_log_index_error(err: anyhow::Error) -> AppError {
    let msg = err.to_string();
    if let Some(rest) = msg.strip_prefix("bad_query:") {
        return AppError::bad_request(rest.trim().to_string());
    }
    AppError::Internal(err)
}

fn health_status_from_handle(handle: &HealthHandle) -> HealthStatus {
    let snap = handle.stats.lock().unwrap_or_else(|e| e.into_inner());
    HealthStatus {
        passes: snap.passes,
        failures: snap.failures,
        consecutive_failures: snap.consecutive_failures,
        last_check_at: snap.last_check_at.clone(),
        last_ok: snap.last_ok,
    }
}

fn health_check_stats_from_status(health: &HealthStatus) -> HealthCheckStats {
    HealthCheckStats {
        passes: health.passes,
        failures: health.failures,
        consecutive_failures: health.consecutive_failures,
        last_check_at: health.last_check_at.clone(),
        last_ok: health.last_ok,
    }
}

fn uptime_seconds_since(last_started_at: Option<&str>) -> Option<u64> {
    let started_at = last_started_at?;
    let started_at = OffsetDateTime::parse(started_at, &Rfc3339).ok()?;
    let now = OffsetDateTime::now_utc();
    let elapsed = (now - started_at).whole_seconds();
    if elapsed < 0 {
        return Some(0);
    }
    Some(elapsed as u64)
}

fn recent_error_from_raw(raw_line: &str) -> RecentErrorLine {
    let timestamp = extract_timestamp_str(raw_line);
    let (_, message) = extract_log_content(raw_line);
    RecentErrorLine { timestamp, message }
}

fn push_recent_stderr_line(lines: &mut Vec<RecentErrorLine>, raw_line: &[u8], limit: usize) {
    if lines.len() >= limit {
        return;
    }

    let raw_line = String::from_utf8_lossy(raw_line);
    let raw_line = crate::util::strip_ansi_if_needed(raw_line.trim_end_matches(['\r', '\n']));
    if raw_line.is_empty() {
        return;
    }

    let (stream, _) = extract_log_content(&raw_line);
    if stream != "stderr" {
        return;
    }

    lines.push(recent_error_from_raw(&raw_line));
}

fn recent_stderr_lines_from_file(log_path: &Path, limit: usize) -> Result<Vec<RecentErrorLine>> {
    if limit == 0 || !log_path.exists() {
        return Ok(Vec::new());
    }

    const CHUNK_SIZE: usize = 64 * 1024;

    let mut file = File::open(log_path)
        .with_context(|| format!("open log file {}", log_path.to_string_lossy()))?;
    let mut offset = file.metadata()?.len();
    let mut trailing = Vec::new();
    let mut lines = Vec::with_capacity(limit);

    while offset > 0 && lines.len() < limit {
        let read_len = (offset as usize).min(CHUNK_SIZE);
        offset -= read_len as u64;

        let mut chunk = vec![0; read_len];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&trailing);

        let mut line_end = chunk.len();
        while let Some(newline_idx) = chunk[..line_end].iter().rposition(|byte| *byte == b'\n') {
            push_recent_stderr_line(&mut lines, &chunk[newline_idx + 1..line_end], limit);
            line_end = newline_idx;
            if lines.len() >= limit {
                break;
            }
        }

        trailing = chunk[..line_end].to_vec();
    }

    if lines.len() < limit && !trailing.is_empty() {
        push_recent_stderr_line(&mut lines, &trailing, limit);
    }

    lines.reverse();
    Ok(lines)
}

async fn recent_stderr_lines(log_path: &Path, limit: usize) -> Vec<RecentErrorLine> {
    let log_path = log_path.to_path_buf();
    tokio::task::spawn_blocking(move || recent_stderr_lines_from_file(log_path.as_path(), limit))
        .await
        .ok()
        .and_then(|result| result.ok())
        .unwrap_or_default()
}

async fn build_status(state: &AppState, run_id: &str) -> AppResult<RunStatusResponse> {
    let run = {
        let guard = state.state.lock().await;
        guard
            .runs
            .get(run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?
            .clone()
    };

    let mut services = BTreeMap::new();
    let mut reconcile: Vec<(String, ServiceState, Option<String>)> = Vec::new();

    for (name, svc) in run.services {
        let desired = if run.state == RunLifecycle::Stopped {
            "stopped".to_string()
        } else {
            "running".to_string()
        };
        let status = state
            .systemd
            .unit_status(&svc.unit_name)
            .await
            .unwrap_or(None)
            .map(|unit| SystemdStatus {
                active_state: unit.active_state,
                sub_state: unit.sub_state,
                result: unit.result,
            });

        let mut derived_state = svc.state.clone();
        let mut derived_failure = svc.last_failure.clone();
        if let Some(sys) = &status
            && (sys.active_state == "failed" || sys.result.as_deref() == Some("start-limit-hit"))
        {
            derived_state = match svc.state {
                ServiceState::Starting => ServiceState::Failed,
                ServiceState::Ready => ServiceState::Degraded,
                ServiceState::Failed => ServiceState::Failed,
                ServiceState::Stopped => ServiceState::Stopped,
                ServiceState::Degraded => ServiceState::Degraded,
            };

            if derived_failure.is_none() && sys.result.as_deref() != Some("success") {
                derived_failure = sys.result.clone();
            }
        }

        if derived_state != svc.state || derived_failure != svc.last_failure {
            reconcile.push((name.clone(), derived_state.clone(), derived_failure.clone()));
        }

        let health = svc.health.as_ref().map(health_status_from_handle);
        let health_check_stats = health.as_ref().map(health_check_stats_from_status);
        let recent_errors = recent_stderr_lines(&svc.log_path, 3).await;

        services.insert(
            name.clone(),
            ServiceStatus {
                desired,
                systemd: status,
                ready: derived_state == ServiceState::Ready,
                state: derived_state,
                last_failure: derived_failure,
                health,
                health_check_stats,
                uptime_seconds: uptime_seconds_since(svc.last_started_at.as_deref()),
                recent_errors,
                url: svc.url.clone(),
                auto_restart: svc.auto_restart,
                watch_paused: svc.watch_paused,
                watch_active: svc.auto_restart && svc.watch_handle.is_some() && !svc.watch_paused,
            },
        );
    }

    let mut any_degraded = false;
    let mut all_ready = true;
    for svc in services.values() {
        match svc.state {
            ServiceState::Ready => {}
            ServiceState::Starting => all_ready = false,
            ServiceState::Degraded | ServiceState::Failed => {
                any_degraded = true;
                all_ready = false;
            }
            ServiceState::Stopped => all_ready = false,
        }
    }
    let derived_run_state = if any_degraded {
        RunLifecycle::Degraded
    } else if all_ready {
        RunLifecycle::Running
    } else {
        run.state.clone()
    };

    if !reconcile.is_empty() {
        let events = {
            let mut guard = state.state.lock().await;
            let mut events = Vec::new();
            if let Some(run) = guard.runs.get_mut(run_id) {
                for (svc_name, new_state, new_failure) in &reconcile {
                    if let Some(svc) = run.services.get_mut(svc_name) {
                        if let Some(event) =
                            set_service_state(run_id, svc_name, svc, new_state.clone())
                        {
                            events.push(event);
                        }
                        svc.last_failure = new_failure.clone();
                    }
                }
                if let Some(event) = recompute_run_state(run) {
                    events.push(event);
                }
            }
            events
        };
        emit_events(state, events);
        let _ = persist_manifest(state, run_id).await;
    }

    Ok(RunStatusResponse {
        run_id: run.run_id,
        stack: run.stack,
        project_dir: run.project_dir.to_string_lossy().to_string(),
        state: derived_run_state,
        services,
    })
}

async fn build_watch_status(state: &AppState, run_id: &str) -> AppResult<RunWatchResponse> {
    let guard = state.state.lock().await;
    let run = guard
        .runs
        .get(run_id)
        .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;

    let services = run
        .services
        .iter()
        .map(|(name, svc)| {
            let active = svc.auto_restart && svc.watch_handle.is_some() && !svc.watch_paused;
            (
                name.clone(),
                WatchServiceStatus {
                    auto_restart: svc.auto_restart,
                    active,
                    paused: svc.watch_paused,
                },
            )
        })
        .collect();

    Ok(RunWatchResponse {
        run_id: run_id.to_string(),
        services,
    })
}

async fn orchestrate_watch_pause(
    state: &AppState,
    run_id: &str,
    service: Option<&str>,
) -> AppResult<RunWatchResponse> {
    {
        let mut guard = state.state.lock().await;
        let run = guard
            .runs
            .get_mut(run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;

        if let Some(service) = service {
            let svc = run
                .services
                .get_mut(service)
                .ok_or_else(|| AppError::not_found(format!("service {service} not found")))?;
            if !svc.auto_restart {
                return Err(AppError::bad_request(format!(
                    "service {service} does not have auto_restart enabled"
                )));
            }
            svc.watch_paused = true;
            if let Some(handle) = &svc.watch_handle {
                handle.paused.store(true, Ordering::SeqCst);
            }
        } else {
            for svc in run.services.values_mut() {
                if !svc.auto_restart {
                    continue;
                }
                svc.watch_paused = true;
                if let Some(handle) = &svc.watch_handle {
                    handle.paused.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    build_watch_status(state, run_id).await
}

async fn orchestrate_watch_resume(
    state: &AppState,
    run_id: &str,
    service: Option<&str>,
) -> AppResult<RunWatchResponse> {
    let mut targets = Vec::new();
    {
        let mut guard = state.state.lock().await;
        let run = guard
            .runs
            .get_mut(run_id)
            .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;

        if let Some(service) = service {
            let svc = run
                .services
                .get_mut(service)
                .ok_or_else(|| AppError::not_found(format!("service {service} not found")))?;
            if !svc.auto_restart {
                return Err(AppError::bad_request(format!(
                    "service {service} does not have auto_restart enabled"
                )));
            }
            svc.watch_paused = false;
            if let Some(handle) = &svc.watch_handle {
                handle.paused.store(false, Ordering::SeqCst);
            }
            targets.push(service.to_string());
        } else {
            for (name, svc) in &mut run.services {
                if !svc.auto_restart {
                    continue;
                }
                svc.watch_paused = false;
                if let Some(handle) = &svc.watch_handle {
                    handle.paused.store(false, Ordering::SeqCst);
                }
                targets.push(name.clone());
            }
        }
    }

    for target in targets {
        if let Err(err) = sync_service_auto_restart_watcher(state, run_id, &target).await {
            eprintln!("devstack: failed to resume watcher for {target}: {err}");
        }
    }

    build_watch_status(state, run_id).await
}

/// Periodically evicts stopped runs from in-memory state and the log index
/// to prevent unbounded memory growth over long daemon lifetimes.
fn spawn_periodic_gc(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60)); // every hour
        interval.tick().await; // first tick is immediate — skip it to let startup finish
        loop {
            interval.tick().await;
            let evicted: Vec<String> = {
                let mut guard = state.state.lock().await;
                let stopped: Vec<String> = guard
                    .runs
                    .iter()
                    .filter(|(_, run)| run.state == RunLifecycle::Stopped)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &stopped {
                    guard.runs.remove(id);
                }
                stopped
            };
            if !evicted.is_empty() {
                eprintln!(
                    "[gc] periodic: evicted {} stopped runs from memory",
                    evicted.len()
                );
                for run_id in &evicted {
                    emit_event(&state, run_removed_event(run_id.clone()));
                }
                let index = state.log_index.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    for run_id in &evicted {
                        let _ = index.delete_run(run_id);
                    }
                    let _ = index.force_compact();
                })
                .await;
                write_daemon_state(&state).await.ok();
            }
        }
    });
}

fn spawn_periodic_ingest(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(err) = ingest_all_once(&state).await {
                eprintln!("devstack: periodic ingest failed: {err}");
            }
        }
    });
}

fn spawn_periodic_compaction(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10 * 60)); // every 10 min
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            let index = state.log_index.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(err) = index.force_compact() {
                    eprintln!("devstack: periodic compaction failed: {err}");
                }
            })
            .await;
        }
    });
}

async fn ingest_all_once(state: &AppState) -> Result<()> {
    let active_runs: Vec<(String, Vec<LogSource>)> = {
        let guard = state.state.lock().await;
        guard
            .runs
            .iter()
            .filter(|(_, run)| run.state != RunLifecycle::Stopped)
            .map(|(run_id, run)| (run_id.clone(), run_service_log_sources(run_id, run)))
            .collect()
    };

    let index = state.log_index.clone();
    tokio::task::spawn_blocking(move || {
        let mut run_ids = Vec::new();
        let mut sources = Vec::new();
        for (run_id, service_sources) in active_runs {
            run_ids.push(run_id.clone());
            sources.extend(service_sources);
            sources.extend(discover_task_log_sources(&run_id)?);
        }

        let ledger = SourcesLedger::load()?;
        let external_sources = all_source_log_sources(&ledger)?;
        sources.extend(external_sources);

        if !sources.is_empty() {
            index.ingest_sources(&sources).context("periodic ingest")?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|err| anyhow!("ingest worker failed: {err}"))??;

    Ok(())
}

async fn run_gc(state: &AppState, req: GcRequest) -> AppResult<GcResponse> {
    let older_than = req
        .older_than
        .as_deref()
        .map(humantime::parse_duration)
        .transpose()
        .map_err(|err| AppError::bad_request(format!("invalid older_than duration: {err}")))?
        .unwrap_or_else(|| Duration::from_secs(7 * 24 * 3600));
    let threshold = SystemTime::now()
        .checked_sub(older_than)
        .ok_or_else(|| AppError::bad_request("older_than duration is too large".to_string()))?;

    let (removed_runs, removed_globals) = {
        let mut removed_runs = Vec::new();
        let mut guard = state.state.lock().await;
        let run_ids: Vec<String> = guard.runs.keys().cloned().collect();
        for run_id in run_ids {
            if let Some(run) = guard.runs.get(&run_id) {
                if run.state != RunLifecycle::Stopped {
                    continue;
                }
                if let Some(stopped_at) = &run.stopped_at {
                    if let Ok(stopped_time) = time::OffsetDateTime::parse(
                        stopped_at,
                        &time::format_description::well_known::Rfc3339,
                    ) {
                        if stopped_time > time::OffsetDateTime::from(threshold) {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                let run_dir = paths::run_dir(&RunId::new(&run_id))?;
                let _ = std::fs::remove_dir_all(run_dir);
                guard.runs.remove(&run_id);
                removed_runs.push(run_id);
            }
        }

        let mut removed_globals = Vec::new();
        if req.all {
            let globals_root = paths::globals_root()?;
            if globals_root.exists() {
                for entry in std::fs::read_dir(globals_root)? {
                    let entry = entry?;
                    let manifest_path = entry.path().join("manifest.json");
                    if !manifest_path.exists() {
                        continue;
                    }
                    if let Ok(manifest) = RunManifest::load_from_path(&manifest_path) {
                        if manifest.state != RunLifecycle::Stopped {
                            continue;
                        }
                        if let Some(stopped_at) = &manifest.stopped_at {
                            if let Ok(stopped_time) = time::OffsetDateTime::parse(
                                stopped_at,
                                &time::format_description::well_known::Rfc3339,
                            ) {
                                if stopped_time > time::OffsetDateTime::from(threshold) {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        }
                        let _ = std::fs::remove_dir_all(entry.path());
                        removed_globals.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
        }
        (removed_runs, removed_globals)
    };

    if !removed_runs.is_empty() {
        for run_id in &removed_runs {
            emit_event(state, run_removed_event(run_id.clone()));
        }
        let index = state.log_index.clone();
        let removed = removed_runs.clone();
        // Best-effort cleanup of derived index entries.
        tokio::task::spawn_blocking(move || {
            for run_id in removed {
                let _ = index.delete_run(&run_id);
            }
        })
        .await
        .ok();
    }

    write_daemon_state(state).await.ok();
    Ok(GcResponse {
        removed_runs,
        removed_globals,
    })
}

fn list_globals_from_disk() -> Result<Vec<GlobalSummary>> {
    let mut globals = Vec::new();
    let root = paths::globals_root()?;
    if !root.exists() {
        return Ok(globals);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let key = entry.file_name().to_string_lossy().to_string();
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest = RunManifest::load_from_path(&manifest_path)?;
        let (name, svc) = match manifest.services.iter().next() {
            Some((name, svc)) => (name.clone(), svc),
            None => continue,
        };
        globals.push(GlobalSummary {
            key,
            name,
            project_dir: manifest.project_dir.clone(),
            state: manifest.state,
            port: svc.port,
            url: svc.url.clone(),
        });
    }
    Ok(globals)
}

async fn ensure_globals(
    state: &AppState,
    globals: &BTreeMap<String, ServiceConfig>,
    tasks_map: &BTreeMap<String, TaskConfig>,
    project_dir: &Path,
    config_dir: &Path,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut ports = BTreeMap::new();

    for (name, svc) in globals {
        let global_dir = paths::global_dir(project_dir, name)?;
        std::fs::create_dir_all(&global_dir)?;
        let log_path = paths::global_log_path(project_dir, name)?;
        std::fs::create_dir_all(paths::global_logs_dir(project_dir, name)?)?;

        let key = paths::global_key(project_dir, name)?;
        let run_id = RunId::new(format!("global-{key}"));
        let unit_name = unit_name_for_run(run_id.as_str(), name);
        let manifest_path = paths::global_manifest_path(project_dir, name)?;
        let mut reuse_port: Option<u16> = None;
        let mut previous_lifecycle: Option<RunLifecycle> = None;

        if manifest_path.exists()
            && let Ok(existing) = RunManifest::load_from_path(&manifest_path)
            && let Some(service) = existing.services.get(name)
        {
            reuse_port = service.port;
            previous_lifecycle = Some(existing.state.clone());
            let status = state.systemd.unit_status(&unit_name).await.ok().flatten();
            if let Some(status) = status
                && status.active_state == "active"
            {
                ports.insert(name.clone(), service.port);
                continue;
            }
        }

        let port = match &svc.port {
            Some(cfg) if cfg.is_none() => None,
            Some(crate::config::PortConfig::Fixed(p)) => Some(*p),
            Some(crate::config::PortConfig::None(_)) => None,
            None => {
                if reuse_port.is_some() {
                    reuse_port
                } else {
                    let mut map = BTreeMap::new();
                    map.insert(name.clone(), svc.clone());
                    let allocated = allocate_ports(&map)?;
                    *allocated.get(name).unwrap_or(&None)
                }
            }
        };
        let scheme = svc.scheme();
        let url = port.map(|p| readiness_url(&scheme, p));

        let tmpl_context = build_template_context(
            &run_id,
            "globals",
            project_dir,
            &BTreeMap::from([(name.clone(), port)]),
            &BTreeMap::from([(name.clone(), scheme.clone())]),
        )?;
        let rendered_cwd = resolve_cwd_path(
            &svc.cwd_or(project_dir).to_string_lossy(),
            &tmpl_context,
            config_dir,
        )?;
        let env_file_path = resolve_env_file_path(svc, &rendered_cwd, &tmpl_context)?;
        let mut env = build_base_env(
            &run_id,
            "globals",
            project_dir,
            &BTreeMap::from([(name.clone(), port)]),
            &BTreeMap::from([(name.clone(), scheme.clone())]),
        )?;
        let file_env = load_env_file(&env_file_path)?;
        merge_env_file(&mut env, file_env);
        if let Some(port) = port {
            env.insert(svc.port_env(), port.to_string());
        }
        let rendered_env = render_env(&svc.env, &tmpl_context)?;
        env.extend(rendered_env);
        env = crate::config::resolve_env_map(&env);
        env.insert("DEV_GRACE_MS".to_string(), "2000".to_string());

        let binary = state.binary_path.to_string_lossy().to_string();
        let exec = ExecStart {
            path: binary.clone(),
            argv: vec![
                binary.clone(),
                "__shim".to_string(),
                "--run-id".to_string(),
                run_id.as_str().to_string(),
                "--service".to_string(),
                name.clone(),
                "--cmd".to_string(),
                render_template(&svc.cmd, &tmpl_context)?,
                "--cwd".to_string(),
                rendered_cwd.to_string_lossy().to_string(),
                "--log-file".to_string(),
                log_path.to_string_lossy().to_string(),
            ],
            ignore_failure: false,
        };
        let readiness = svc.readiness_spec(port.is_some())?;
        let properties = UnitProperties::new(
            format!("devstack global {}", name),
            &rendered_cwd,
            env.iter().map(|(k, v)| format!("{k}={v}")).collect(),
            exec,
        )
        .with_restart("no")
        .with_remain_after_exit(matches!(readiness.kind, ReadinessKind::Exit));
        let ctx = ReadinessContext {
            port,
            scheme: scheme.clone(),
            log_path: log_path.clone(),
            cwd: rendered_cwd.clone(),
            env: env.clone(),
            unit_name: Some(unit_name.clone()),
            systemd: Some(state.systemd.clone()),
        };
        let post_init = build_post_init_context(svc, tasks_map, project_dir, &run_id);

        let mut service_state = ServiceState::Ready;
        let mut lifecycle = RunLifecycle::Running;
        let mut startup_error: Option<anyhow::Error> = None;

        match state
            .systemd
            .start_transient_service(&unit_name, properties)
            .await
        {
            Ok(()) => {
                if let Err(err) = crate::readiness::wait_for_ready(&readiness, &ctx).await {
                    service_state = ServiceState::Failed;
                    lifecycle = RunLifecycle::Degraded;
                    startup_error =
                        Some(anyhow!("global service '{name}' failed readiness: {err}"));
                } else if let Some(post_init) = post_init {
                    if let Err(err) = run_post_init_tasks_blocking(
                        post_init.tasks_map,
                        post_init.post_init_tasks,
                        post_init.project_dir,
                        post_init.run_id,
                    )
                    .await
                    {
                        service_state = ServiceState::Failed;
                        lifecycle = RunLifecycle::Degraded;
                        startup_error =
                            Some(anyhow!("global service '{name}' post_init failed: {err}"));
                    }
                }
            }
            Err(err) => {
                service_state = ServiceState::Failed;
                lifecycle = RunLifecycle::Degraded;
                startup_error = Some(err.context(format!("start global service '{name}'")));
            }
        }

        let manifest = RunManifest {
            run_id: run_id.as_str().to_string(),
            project_dir: project_dir.to_string_lossy().to_string(),
            stack: "globals".to_string(),
            manifest_path: manifest_path.to_string_lossy().to_string(),
            services: BTreeMap::from([(
                name.clone(),
                ServiceManifest {
                    port,
                    url: url.clone(),
                    state: service_state,
                    watch_hash: None,
                },
            )]),
            env: env.clone(),
            state: lifecycle,
            created_at: now_rfc3339(),
            stopped_at: None,
        };
        manifest.write_to_path(&manifest_path)?;
        if previous_lifecycle.as_ref() != Some(&manifest.state) {
            emit_event(
                state,
                global_state_changed_event(&key, manifest.state.clone()),
            );
        }

        if let Some(err) = startup_error {
            return Err(err);
        }

        ports.insert(name.clone(), port);
    }

    Ok(ports)
}

pub async fn doctor() -> Result<DoctorResponse> {
    let mut checks = Vec::new();

    // Daemon socket check.
    let daemon_ok = ping_daemon_socket().await;
    checks.push(DoctorCheck {
        name: "daemon_socket".to_string(),
        ok: daemon_ok,
        message: if daemon_ok {
            "daemon socket present".to_string()
        } else {
            "daemon socket missing; run devstack daemon or devstack install".to_string()
        },
    });

    #[cfg(target_os = "linux")]
    {
        // systemd DBus check and transient unit test.
        match RealSystemd::connect().await {
            Ok(systemd) => {
                let unit_name = format!("devstack-doctor-{}.service", std::process::id());
                let exec = ExecStart {
                    path: "/bin/true".to_string(),
                    argv: vec!["/bin/true".to_string()],
                    ignore_failure: false,
                };
                let props = UnitProperties::new(
                    "devstack doctor".to_string(),
                    Path::new("/"),
                    vec![],
                    exec,
                );
                let start = systemd.start_transient_service(&unit_name, props).await;
                let stop = systemd.stop_unit(&unit_name).await;
                let ok = start.is_ok() && stop.is_ok();
                checks.push(DoctorCheck {
                    name: "systemd_user".to_string(),
                    ok,
                    message: if ok {
                        "systemd user instance reachable".to_string()
                    } else {
                        "systemd user instance unavailable".to_string()
                    },
                });
            }
            Err(err) => {
                checks.push(DoctorCheck {
                    name: "systemd_user".to_string(),
                    ok: false,
                    message: format!("systemd user instance error: {err}"),
                });
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        checks.push(DoctorCheck {
            name: "process_manager".to_string(),
            ok: true,
            message: "local process manager active".to_string(),
        });
    }

    // Filesystem permissions.
    let base_ok = paths::ensure_base_layout().is_ok();
    checks.push(DoctorCheck {
        name: "filesystem".to_string(),
        ok: base_ok,
        message: if base_ok {
            "filesystem layout ok".to_string()
        } else {
            "cannot create base directories".to_string()
        },
    });

    Ok(DoctorResponse { checks })
}

async fn ping_daemon_socket() -> bool {
    let socket_path = match paths::daemon_socket_path() {
        Ok(path) => path,
        Err(_) => return false,
    };
    if !socket_path.exists() {
        return false;
    }
    let stream = match UnixStream::connect(socket_path).await {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let io = TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(parts) => parts,
        Err(_) => return false,
    };
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let request = match hyper::Request::builder()
        .method("GET")
        .uri("http://localhost/v1/ping")
        .body(Full::new(hyper::body::Bytes::new()))
    {
        Ok(req) => req,
        Err(_) => return false,
    };
    let response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(_) => return false,
    };
    if !response.status().is_success() {
        return false;
    }
    let body = response.into_body().collect().await.ok();
    if let Some(body) = body {
        let value: serde_json::Value = serde_json::from_slice(&body.to_bytes()).unwrap_or_default();
        return value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    }
    false
}

#[derive(Debug)]
pub(crate) enum AppError {
    NotFound(String),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(value: E) -> Self {
        AppError::Internal(value.into())
    }
}

impl AppError {
    fn not_found(message: impl Into<String>) -> Self {
        AppError::NotFound(message.into())
    }

    fn bad_request(message: impl Into<String>) -> Self {
        AppError::BadRequest(message.into())
    }
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self {
            AppError::NotFound(message) => {
                let body = serde_json::json!({ "error": message });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            AppError::BadRequest(message) => {
                let body = serde_json::json!({ "error": message });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            AppError::Internal(err) => {
                let body = serde_json::json!({ "error": err.to_string() });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
        }
    }
}

impl Clone for RunState {
    fn clone(&self) -> Self {
        Self {
            run_id: self.run_id.clone(),
            stack: self.stack.clone(),
            project_dir: self.project_dir.clone(),
            base_env: self.base_env.clone(),
            services: self
                .services
                .iter()
                .map(|(name, svc)| (name.clone(), svc.clone()))
                .collect(),
            state: self.state.clone(),
            created_at: self.created_at.clone(),
            stopped_at: self.stopped_at.clone(),
        }
    }
}

impl Clone for ServiceRuntime {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            unit_name: self.unit_name.clone(),
            port: self.port,
            scheme: self.scheme.clone(),
            url: self.url.clone(),
            deps: self.deps.clone(),
            readiness: self.readiness.clone(),
            log_path: self.log_path.clone(),
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            state: self.state.clone(),
            last_failure: self.last_failure.clone(),
            health: self.health.clone(),
            last_started_at: self.last_started_at.clone(),
            watch_hash: self.watch_hash.clone(),
            watch_patterns: self.watch_patterns.clone(),
            ignore_patterns: self.ignore_patterns.clone(),
            watch_extra_files: self.watch_extra_files.clone(),
            watch_fingerprint: self.watch_fingerprint.clone(),
            auto_restart: self.auto_restart,
            watch_paused: self.watch_paused,
            watch_handle: self.watch_handle.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    fn base_service() -> ServiceConfig {
        ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: None,
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        }
    }

    #[test]
    fn base_env_includes_ports_and_urls() {
        let run_id = RunId::new("test-run");
        let project = PathBuf::from("/tmp/project");
        let port_map = BTreeMap::from([
            ("api".to_string(), Some(1234)),
            ("worker".to_string(), None),
        ]);
        let schemes = BTreeMap::from([("api".to_string(), "http".to_string())]);
        let env = build_base_env(&run_id, "stack", &project, &port_map, &schemes).unwrap();
        assert_eq!(env.get("DEV_RUN_ID").unwrap(), "test-run");
        assert_eq!(env.get("DEV_PORT_API").unwrap(), "1234");
        assert_eq!(env.get("DEV_URL_API").unwrap(), "http://localhost:1234");
        assert!(!env.contains_key("DEV_PORT_WORKER"));
    }

    #[test]
    fn render_env_templates() {
        let run_id = RunId::new("test-run");
        let project = PathBuf::from("/tmp/project");
        let port_map = BTreeMap::from([("api".to_string(), Some(5555))]);
        let schemes = BTreeMap::from([("api".to_string(), "http".to_string())]);
        let ctx = build_template_context(&run_id, "stack", &project, &port_map, &schemes).unwrap();
        let env = BTreeMap::from([(
            "VITE_API_URL".to_string(),
            "{{ services.api.url }}".to_string(),
        )]);
        let rendered = render_env(&env, &ctx).unwrap();
        assert_eq!(
            rendered.get("VITE_API_URL").unwrap(),
            "http://localhost:5555"
        );
    }

    #[test]
    fn load_env_file_parses_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "FOO=bar\nQUOTED=\"hello world\"\n").unwrap();
        let env = load_env_file(&path).unwrap();
        assert_eq!(env.get("FOO").unwrap(), "bar");
        assert_eq!(env.get("QUOTED").unwrap(), "hello world");
    }

    #[test]
    fn merge_env_file_skips_dev_vars() {
        let mut env = BTreeMap::from([("DEV_STACK".to_string(), "base".to_string())]);
        let file_env = BTreeMap::from([
            ("DEV_STACK".to_string(), "override".to_string()),
            ("FOO".to_string(), "bar".to_string()),
        ]);
        merge_env_file(&mut env, file_env);
        assert_eq!(env.get("DEV_STACK").unwrap(), "base");
        assert_eq!(env.get("FOO").unwrap(), "bar");
    }

    #[test]
    fn resolve_env_file_defaults_to_dotenv() {
        let svc = base_service();
        let dir = tempfile::tempdir().unwrap();
        let ctx = serde_json::json!({});
        let path = resolve_env_file_path(&svc, dir.path(), &ctx).unwrap();
        assert_eq!(path, dir.path().join(".env"));
    }

    #[test]
    fn resolve_env_file_uses_relative_path() {
        let mut svc = base_service();
        svc.env_file = Some(PathBuf::from(".env.local"));
        let dir = tempfile::tempdir().unwrap();
        let ctx = serde_json::json!({});
        let path = resolve_env_file_path(&svc, dir.path(), &ctx).unwrap();
        assert_eq!(path, dir.path().join(".env.local"));
    }

    #[test]
    fn resolve_env_file_renders_template() {
        let mut svc = base_service();
        svc.env_file = Some(PathBuf::from("{{ project.dir }}/.env.custom"));
        let dir = tempfile::tempdir().unwrap();
        let run_id = RunId::new("test-run");
        let ctx = build_template_context(
            &run_id,
            "stack",
            dir.path(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let path = resolve_env_file_path(&svc, dir.path(), &ctx).unwrap();
        assert_eq!(path, dir.path().join(".env.custom"));
    }

    #[derive(Clone)]
    struct MockSystemd;

    #[async_trait]
    impl SystemdManager for MockSystemd {
        async fn start_transient_service(
            &self,
            _unit_name: &str,
            _props: UnitProperties,
        ) -> Result<()> {
            Ok(())
        }

        async fn stop_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn restart_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn kill_unit(&self, _unit_name: &str, _signal: i32) -> Result<()> {
            Ok(())
        }

        async fn unit_status(
            &self,
            _unit_name: &str,
        ) -> Result<Option<crate::systemd::UnitStatus>> {
            Ok(None)
        }
    }

    fn test_state_for(systemd: Arc<dyn SystemdManager>) -> AppState {
        let lock_dir = tempfile::tempdir().unwrap();
        let lock_file = std::fs::File::create(lock_dir.path().join("test.lock")).unwrap();
        let log_index_dir = tempfile::tempdir().unwrap();
        let log_index_path = log_index_dir.path().to_path_buf();
        std::mem::forget(log_index_dir);
        let (event_tx, _) = broadcast::channel(1024);
        AppState {
            systemd,
            state: Arc::new(Mutex::new(DaemonState::default())),
            binary_path: PathBuf::from("/bin/true"),
            log_index: Arc::new(LogIndex::open_or_create_in(&log_index_path).unwrap()),
            event_tx,
            log_tails: Arc::new(Mutex::new(RunLogTailRegistry::default())),
            _lock: Arc::new(lock_file),
        }
    }

    fn test_state() -> AppState {
        test_state_for(Arc::new(MockSystemd))
    }

    fn runtime_service(name: &str, cwd: &Path) -> ServiceRuntime {
        ServiceRuntime {
            name: name.to_string(),
            unit_name: format!("{name}.service"),
            port: None,
            scheme: "http".to_string(),
            url: None,
            deps: Vec::new(),
            readiness: ReadinessSpec::new(ReadinessKind::None),
            log_path: cwd.join(format!("{name}.log")),
            cwd: cwd.to_path_buf(),
            env: BTreeMap::new(),
            state: ServiceState::Ready,
            last_failure: None,
            health: None,
            last_started_at: Some(now_rfc3339()),
            watch_hash: Some("hash".to_string()),
            watch_patterns: Vec::new(),
            ignore_patterns: Vec::new(),
            watch_extra_files: Vec::new(),
            watch_fingerprint: Vec::new(),
            auto_restart: false,
            watch_paused: false,
            watch_handle: None,
        }
    }

    fn write_run_snapshot(run_id: &RunId, marker_path: &Path) {
        let snapshot_path = paths::run_snapshot_path(run_id).unwrap();
        std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
        std::fs::write(
            snapshot_path,
            format!(
                "version = 1\n\n[tasks.seed]\ncmd = \"printf ready > '{}'\"\n\n[stacks.dev.services.api]\ncmd = \"echo api\"\npost_init = [\"seed\"]\n",
                marker_path.display()
            ),
        )
        .unwrap();
    }

    #[derive(Clone)]
    struct ExitReadySystemd {
        remain_after_exit: Arc<Mutex<Option<bool>>>,
    }

    #[async_trait]
    impl SystemdManager for ExitReadySystemd {
        async fn start_transient_service(
            &self,
            _unit_name: &str,
            props: UnitProperties,
        ) -> Result<()> {
            let mut guard = self.remain_after_exit.lock().await;
            *guard = Some(props.remain_after_exit);
            Ok(())
        }

        async fn stop_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn restart_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn kill_unit(&self, _unit_name: &str, _signal: i32) -> Result<()> {
            Ok(())
        }

        async fn unit_status(
            &self,
            _unit_name: &str,
        ) -> Result<Option<crate::systemd::UnitStatus>> {
            let guard = self.remain_after_exit.lock().await;
            if matches!(*guard, Some(true)) {
                return Ok(Some(crate::systemd::UnitStatus {
                    active_state: "active".to_string(),
                    sub_state: "exited".to_string(),
                    result: Some("success".to_string()),
                }));
            }
            Ok(None)
        }
    }

    #[derive(Clone)]
    struct RestartPolicySystemd {
        restart_policy: Arc<Mutex<Option<String>>>,
    }

    #[async_trait]
    impl SystemdManager for RestartPolicySystemd {
        async fn start_transient_service(
            &self,
            _unit_name: &str,
            props: UnitProperties,
        ) -> Result<()> {
            let mut guard = self.restart_policy.lock().await;
            *guard = Some(props.restart);
            Ok(())
        }

        async fn stop_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn restart_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn kill_unit(&self, _unit_name: &str, _signal: i32) -> Result<()> {
            Ok(())
        }

        async fn unit_status(
            &self,
            _unit_name: &str,
        ) -> Result<Option<crate::systemd::UnitStatus>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn start_prepared_service_disables_systemd_auto_restart_for_restarts() {
        let manager = RestartPolicySystemd {
            restart_policy: Arc::new(Mutex::new(None)),
        };
        let restart_policy = manager.restart_policy.clone();
        let state = test_state_for(Arc::new(manager));
        let run_id = RunId::new("app-test");
        let prepared = PreparedService {
            name: "api".to_string(),
            unit_name: "devstack-run-APP-TEST-API.service".to_string(),
            port: Some(3000),
            scheme: "http".to_string(),
            url: Some("http://localhost:3000".to_string()),
            deps: vec![],
            readiness: ReadinessSpec::new(ReadinessKind::Tcp),
            log_path: PathBuf::from("/tmp/api.log"),
            cwd: PathBuf::from("/tmp"),
            env: BTreeMap::new(),
            cmd: "echo api".to_string(),
            watch_hash: "hash".to_string(),
            watch_patterns: Vec::new(),
            ignore_patterns: Vec::new(),
            watch_extra_files: Vec::new(),
            watch_fingerprint: Vec::new(),
            auto_restart: false,
        };

        start_prepared_service(&state, &run_id, &prepared, true)
            .await
            .unwrap();

        let guard = restart_policy.lock().await;
        assert_eq!(guard.as_deref(), Some("no"));
    }

    #[tokio::test]
    async fn ensure_globals_disables_systemd_auto_restart() {
        let manager = RestartPolicySystemd {
            restart_policy: Arc::new(Mutex::new(None)),
        };
        let restart_policy = manager.restart_policy.clone();
        let state = test_state_for(Arc::new(manager));
        let project_dir = tempfile::tempdir().unwrap();
        let globals = BTreeMap::from([(
            "moto".to_string(),
            ServiceConfig {
                cmd: "echo moto".to_string(),
                deps: Vec::new(),
                scheme: None,
                port_env: None,
                port: Some(crate::config::PortConfig::None("none".to_string())),
                readiness: None,
                env_file: None,
                env: BTreeMap::new(),
                cwd: None,
                watch: Vec::new(),
                ignore: Vec::new(),
                auto_restart: false,
                init: None,
                post_init: None,
            },
        )]);

        ensure_globals(
            &state,
            &globals,
            &BTreeMap::new(),
            project_dir.path(),
            project_dir.path(),
        )
        .await
        .unwrap();

        let guard = restart_policy.lock().await;
        assert_eq!(guard.as_deref(), Some("no"));
    }

    #[tokio::test]
    async fn exit_readiness_handles_fast_exit_units() {
        let manager = ExitReadySystemd {
            remain_after_exit: Arc::new(Mutex::new(None)),
        };
        let state = test_state_for(Arc::new(manager));
        let run_id = RunId::new("app-test");
        let prepared = PreparedService {
            name: "migrate".to_string(),
            unit_name: "devstack-run-APP-TEST-MIGRATE.service".to_string(),
            port: None,
            scheme: "http".to_string(),
            url: None,
            deps: vec![],
            readiness: ReadinessSpec {
                kind: ReadinessKind::Exit,
                timeout: Duration::from_secs(1),
            },
            log_path: PathBuf::from("/tmp/migrate.log"),
            cwd: PathBuf::from("/tmp"),
            env: BTreeMap::new(),
            cmd: "echo migration-done".to_string(),
            watch_hash: "hash".to_string(),
            watch_patterns: Vec::new(),
            ignore_patterns: Vec::new(),
            watch_extra_files: Vec::new(),
            watch_fingerprint: Vec::new(),
            auto_restart: false,
        };

        start_prepared_service(&state, &run_id, &prepared, false)
            .await
            .unwrap();

        let ctx = ReadinessContext {
            port: None,
            scheme: prepared.scheme.clone(),
            log_path: prepared.log_path.clone(),
            cwd: prepared.cwd.clone(),
            env: prepared.env.clone(),
            unit_name: Some(prepared.unit_name.clone()),
            systemd: Some(state.systemd.clone()),
        };

        crate::readiness::wait_for_ready(&prepared.readiness, &ctx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn restart_service_runs_post_init_tasks() {
        let state = test_state();
        let project_dir = tempfile::tempdir().unwrap();
        let run_id = RunId::new(format!("restart-post-init-{}", std::process::id()));
        let marker_path = project_dir.path().join("post-init.txt");
        write_run_snapshot(&run_id, &marker_path);

        let services = BTreeMap::from([(
            "api".to_string(),
            runtime_service("api", project_dir.path()),
        )]);
        {
            let mut guard = state.state.lock().await;
            guard.runs.insert(
                run_id.as_str().to_string(),
                RunState {
                    run_id: run_id.as_str().to_string(),
                    stack: "dev".to_string(),
                    project_dir: project_dir.path().to_path_buf(),
                    base_env: BTreeMap::new(),
                    services,
                    state: RunLifecycle::Running,
                    created_at: now_rfc3339(),
                    stopped_at: None,
                },
            );
        }

        let manifest = orchestrate_restart_service(&state, run_id.as_str(), "api", false)
            .await
            .unwrap();

        assert_eq!(manifest.services["api"].state, ServiceState::Ready);
        assert_eq!(std::fs::read_to_string(marker_path).unwrap(), "ready");
    }

    #[tokio::test]
    async fn restart_service_runs_post_init_tasks_when_no_wait_is_enabled() {
        let state = test_state();
        let project_dir = tempfile::tempdir().unwrap();
        let run_id = RunId::new(format!("restart-post-init-nowait-{}", std::process::id()));
        let marker_path = project_dir.path().join("post-init.txt");
        write_run_snapshot(&run_id, &marker_path);

        let services = BTreeMap::from([(
            "api".to_string(),
            runtime_service("api", project_dir.path()),
        )]);
        {
            let mut guard = state.state.lock().await;
            guard.runs.insert(
                run_id.as_str().to_string(),
                RunState {
                    run_id: run_id.as_str().to_string(),
                    stack: "dev".to_string(),
                    project_dir: project_dir.path().to_path_buf(),
                    base_env: BTreeMap::new(),
                    services,
                    state: RunLifecycle::Running,
                    created_at: now_rfc3339(),
                    stopped_at: None,
                },
            );
        }

        orchestrate_restart_service(&state, run_id.as_str(), "api", true)
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let service_ready = {
                    let guard = state.state.lock().await;
                    guard.runs[run_id.as_str()].services["api"].state == ServiceState::Ready
                };
                if marker_path.exists() && service_ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();

        let guard = state.state.lock().await;
        assert_eq!(
            guard.runs[run_id.as_str()].services["api"].state,
            ServiceState::Ready
        );
        assert_eq!(std::fs::read_to_string(marker_path).unwrap(), "ready");
    }

    #[tokio::test]
    async fn run_gc_rejects_overflowing_duration() {
        let state = test_state();
        let response = run_gc(
            &state,
            GcRequest {
                older_than: Some("1000000000000000000000000000s".to_string()),
                all: false,
            },
        )
        .await;

        assert!(matches!(response, Err(AppError::BadRequest(_))));
    }

    #[tokio::test]
    async fn watch_pause_resume_updates_service_state() {
        let state = test_state();

        let watch_handle = ServiceWatchHandle {
            stop_flag: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
        };

        let mut services = BTreeMap::new();
        services.insert(
            "worker".to_string(),
            ServiceRuntime {
                name: "worker".to_string(),
                unit_name: "worker.service".to_string(),
                port: None,
                scheme: "http".to_string(),
                url: None,
                deps: Vec::new(),
                readiness: ReadinessSpec::new(ReadinessKind::None),
                log_path: PathBuf::from("/tmp/worker.log"),
                cwd: PathBuf::from("/tmp"),
                env: BTreeMap::new(),
                state: ServiceState::Ready,
                last_failure: None,
                health: None,
                last_started_at: Some(now_rfc3339()),
                watch_hash: Some("hash".to_string()),
                watch_patterns: vec!["src/**".to_string()],
                ignore_patterns: Vec::new(),
                watch_extra_files: Vec::new(),
                watch_fingerprint: b"fingerprint".to_vec(),
                auto_restart: true,
                watch_paused: false,
                watch_handle: Some(watch_handle.clone()),
            },
        );
        services.insert(
            "web".to_string(),
            ServiceRuntime {
                name: "web".to_string(),
                unit_name: "web.service".to_string(),
                port: None,
                scheme: "http".to_string(),
                url: None,
                deps: Vec::new(),
                readiness: ReadinessSpec::new(ReadinessKind::None),
                log_path: PathBuf::from("/tmp/web.log"),
                cwd: PathBuf::from("/tmp"),
                env: BTreeMap::new(),
                state: ServiceState::Ready,
                last_failure: None,
                health: None,
                last_started_at: Some(now_rfc3339()),
                watch_hash: Some("hash".to_string()),
                watch_patterns: vec!["src/**".to_string()],
                ignore_patterns: Vec::new(),
                watch_extra_files: Vec::new(),
                watch_fingerprint: b"fingerprint".to_vec(),
                auto_restart: false,
                watch_paused: false,
                watch_handle: None,
            },
        );

        {
            let mut guard = state.state.lock().await;
            guard.runs.insert(
                "run-1".to_string(),
                RunState {
                    run_id: "run-1".to_string(),
                    stack: "dev".to_string(),
                    project_dir: PathBuf::from("/tmp/project"),
                    base_env: BTreeMap::new(),
                    services,
                    state: RunLifecycle::Running,
                    created_at: now_rfc3339(),
                    stopped_at: None,
                },
            );
        }

        let paused = watch_pause(
            State(state.clone()),
            AxumPath("run-1".to_string()),
            Json(WatchControlRequest { service: None }),
        )
        .await
        .unwrap()
        .0;
        assert!(paused.services.get("worker").unwrap().paused);
        assert!(!paused.services.get("worker").unwrap().active);
        assert!(!paused.services.get("web").unwrap().paused);

        let resumed = watch_resume(
            State(state.clone()),
            AxumPath("run-1".to_string()),
            Json(WatchControlRequest {
                service: Some("worker".to_string()),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(!resumed.services.get("worker").unwrap().paused);
        assert!(resumed.services.get("worker").unwrap().active);
        assert!(!watch_handle.paused.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn navigation_intent_can_be_stored_replaced_and_cleared() {
        let state = test_state();

        let first = set_navigation_intent(
            State(state.clone()),
            Json(crate::api::SetNavigationIntentRequest {
                run_id: Some("run-1".to_string()),
                service: Some("api".to_string()),
                search: Some("timeout".to_string()),
                level: Some("error".to_string()),
                stream: Some("stderr".to_string()),
                since: Some("2025-01-01T00:00:00Z".to_string()),
                last: Some(100),
            }),
        )
        .await
        .unwrap()
        .0;

        let first_intent = first.intent.expect("intent should be stored");
        assert_eq!(first_intent.run_id.as_deref(), Some("run-1"));
        assert_eq!(first_intent.service.as_deref(), Some("api"));
        assert_eq!(first_intent.search.as_deref(), Some("timeout"));

        let fetched = get_navigation_intent(State(state.clone())).await.unwrap().0;
        let fetched_intent = fetched.intent.expect("intent should be retrievable");
        assert_eq!(fetched_intent.service.as_deref(), Some("api"));
        assert_eq!(fetched_intent.last, Some(100));

        let replaced = set_navigation_intent(
            State(state.clone()),
            Json(crate::api::SetNavigationIntentRequest {
                run_id: Some("run-2".to_string()),
                service: Some("worker".to_string()),
                search: Some("panic".to_string()),
                level: Some("warn".to_string()),
                stream: Some("stdout".to_string()),
                since: Some("2025-01-02T00:00:00Z".to_string()),
                last: Some(50),
            }),
        )
        .await
        .unwrap()
        .0;

        let replaced_intent = replaced.intent.expect("intent should be replaced");
        assert_eq!(replaced_intent.run_id.as_deref(), Some("run-2"));
        assert_eq!(replaced_intent.service.as_deref(), Some("worker"));
        assert_eq!(replaced_intent.search.as_deref(), Some("panic"));

        let _ = clear_navigation_intent(State(state.clone())).await.unwrap();

        let cleared = get_navigation_intent(State(state)).await.unwrap().0;
        assert!(cleared.intent.is_none());
    }

    #[tokio::test]
    async fn logs_facets_returns_filter_metadata() {
        let state = test_state();
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            "[2025-01-01T00:00:00Z] [stdout] hello\n[2025-01-01T00:00:01Z] [stderr] Error: boom\n",
        )
        .unwrap();

        let services = BTreeMap::from([(
            "api".to_string(),
            ServiceRuntime {
                name: "api".to_string(),
                unit_name: "api.service".to_string(),
                port: None,
                scheme: "http".to_string(),
                url: None,
                deps: Vec::new(),
                readiness: ReadinessSpec::new(ReadinessKind::None),
                log_path,
                cwd: dir.path().to_path_buf(),
                env: BTreeMap::new(),
                state: ServiceState::Ready,
                last_failure: None,
                health: None,
                last_started_at: Some(now_rfc3339()),
                watch_hash: Some("hash".to_string()),
                watch_patterns: vec![],
                ignore_patterns: vec![],
                watch_extra_files: vec![],
                watch_fingerprint: vec![],
                auto_restart: false,
                watch_paused: false,
                watch_handle: None,
            },
        )]);

        {
            let mut guard = state.state.lock().await;
            guard.runs.insert(
                "run-1".to_string(),
                RunState {
                    run_id: "run-1".to_string(),
                    stack: "dev".to_string(),
                    project_dir: dir.path().to_path_buf(),
                    base_env: BTreeMap::new(),
                    services,
                    state: RunLifecycle::Running,
                    created_at: now_rfc3339(),
                    stopped_at: None,
                },
            );
        }

        state
            .log_index
            .ingest_sources(&[LogSource {
                run_id: "run-1".to_string(),
                service: "api".to_string(),
                path: dir.path().join("api.log"),
            }])
            .unwrap();

        let response = logs_view(
            State(state),
            AxumPath("run-1".to_string()),
            Query(LogViewQuery {
                last: None,
                since: None,
                search: None,
                service: None,
                level: None,
                stream: None,
                include_entries: false,
                include_facets: true,
            }),
        )
        .await
        .unwrap()
        .0;

        assert!(
            response
                .filters
                .iter()
                .any(|filter| filter.field == "service")
        );
        assert!(
            response
                .filters
                .iter()
                .any(|filter| filter.field == "level")
        );
        assert!(
            response
                .filters
                .iter()
                .any(|filter| filter.field == "stream")
        );
    }

    #[tokio::test]
    async fn find_latest_run_for_project_stack_picks_newest() {
        let state = test_state();

        let project = PathBuf::from("/tmp/project");
        let mut guard = state.state.lock().await;
        guard.runs.insert(
            "run-old".to_string(),
            RunState {
                run_id: "run-old".to_string(),
                stack: "voice".to_string(),
                project_dir: project.clone(),
                base_env: BTreeMap::new(),
                services: BTreeMap::new(),
                state: RunLifecycle::Running,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                stopped_at: None,
            },
        );
        guard.runs.insert(
            "run-new".to_string(),
            RunState {
                run_id: "run-new".to_string(),
                stack: "voice".to_string(),
                project_dir: project.clone(),
                base_env: BTreeMap::new(),
                services: BTreeMap::new(),
                state: RunLifecycle::Running,
                created_at: "2025-01-02T00:00:00Z".to_string(),
                stopped_at: None,
            },
        );
        drop(guard);

        let found = find_latest_run_for_project_stack(&state, &project, "voice")
            .await
            .unwrap();
        assert_eq!(found, Some("run-new".to_string()));
    }

    #[test]
    fn agent_session_register_and_unregister() {
        let mut sessions = BTreeMap::new();
        let request = AgentSessionRegisterRequest {
            agent_id: "agent-1".to_string(),
            project_dir: "/tmp/project".to_string(),
            stack: Some("dev".to_string()),
            command: "claude".to_string(),
            pid: std::process::id(),
        };

        let registered = register_agent_session_state(&mut sessions, request);
        assert_eq!(registered.agent_id, "agent-1");
        assert!(sessions.contains_key("agent-1"));

        let removed = sessions.remove("agent-1");
        assert!(removed.is_some());
        assert!(sessions.is_empty());
    }

    #[test]
    fn agent_session_message_queue_polls_and_clears() {
        let mut sessions = BTreeMap::new();
        register_agent_session_state(
            &mut sessions,
            AgentSessionRegisterRequest {
                agent_id: "agent-1".to_string(),
                project_dir: "/tmp/project".to_string(),
                stack: None,
                command: "claude".to_string(),
                pid: std::process::id(),
            },
        );

        let queued = queue_agent_message(&mut sessions, "agent-1", "first".to_string()).unwrap();
        assert_eq!(queued, 1);
        let queued = queue_agent_message(&mut sessions, "agent-1", "second".to_string()).unwrap();
        assert_eq!(queued, 2);

        let messages = poll_agent_session_messages(&mut sessions, "agent-1").unwrap();
        assert_eq!(messages, vec!["first".to_string(), "second".to_string()]);

        let messages = poll_agent_session_messages(&mut sessions, "agent-1").unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn share_agent_message_targets_latest_session_for_project() {
        let state = test_state();

        let _ = register_agent_session(
            State(state.clone()),
            Json(AgentSessionRegisterRequest {
                agent_id: "agent-old".to_string(),
                project_dir: "/tmp/project".to_string(),
                stack: None,
                command: "claude".to_string(),
                pid: std::process::id(),
            }),
        )
        .await
        .unwrap();

        let _ = register_agent_session(
            State(state.clone()),
            Json(AgentSessionRegisterRequest {
                agent_id: "agent-new".to_string(),
                project_dir: "/tmp/project".to_string(),
                stack: None,
                command: "claude".to_string(),
                pid: std::process::id(),
            }),
        )
        .await
        .unwrap();

        let _ = register_agent_session(
            State(state.clone()),
            Json(AgentSessionRegisterRequest {
                agent_id: "agent-other".to_string(),
                project_dir: "/tmp/other".to_string(),
                stack: None,
                command: "claude".to_string(),
                pid: std::process::id(),
            }),
        )
        .await
        .unwrap();

        {
            let mut guard = state.state.lock().await;
            guard
                .agent_sessions
                .get_mut("agent-old")
                .unwrap()
                .created_at = "2025-01-01T00:00:00Z".to_string();
            guard
                .agent_sessions
                .get_mut("agent-new")
                .unwrap()
                .created_at = "2025-01-02T00:00:00Z".to_string();
        }

        let latest = get_latest_agent_session(
            State(state.clone()),
            Query(LatestAgentSessionQuery {
                project_dir: "/tmp/project".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(latest.session.unwrap().agent_id, "agent-new");

        let shared = share_agent_message(
            State(state.clone()),
            Json(ShareAgentMessageRequest {
                project_dir: "/tmp/project".to_string(),
                command: Some("devstack logs --run run-1 --service api --level error".to_string()),
                message: "Can you look at this?".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(shared.agent_id, "agent-new");
        assert_eq!(shared.queued, 1);

        let mut guard = state.state.lock().await;
        let old_messages =
            poll_agent_session_messages(&mut guard.agent_sessions, "agent-old").unwrap();
        let new_messages =
            poll_agent_session_messages(&mut guard.agent_sessions, "agent-new").unwrap();
        assert!(old_messages.is_empty());
        assert_eq!(
            new_messages,
            vec!["Can you look at this?\nRun `devstack logs --run run-1 --service api --level error`".to_string()]
        );
    }

    #[test]
    fn merge_task_summaries_prefers_live_detached_execution() {
        let mut tasks = BTreeMap::new();
        merge_task_summary(
            &mut tasks,
            task_summary_from_history(&crate::tasks::TaskExecution {
                task: "migrate".to_string(),
                started_at: "2025-01-01T00:00:00Z".to_string(),
                finished_at: "2025-01-01T00:00:03Z".to_string(),
                exit_code: 0,
                duration_ms: 3000,
                log_file: "migrate.log".to_string(),
                scope: "run:run-1".to_string(),
            }),
        );
        merge_task_summary(
            &mut tasks,
            task_summary_from_detached(&DetachedTaskExecution {
                execution_id: "task-1".to_string(),
                task: "migrate".to_string(),
                project_dir: PathBuf::from("/tmp/project"),
                run_id: Some("run-1".to_string()),
                state: TaskExecutionState::Running,
                started_at: "2025-01-01T00:00:05Z".to_string(),
                started_at_instant: StdInstant::now(),
                finished_at: None,
                exit_code: None,
                duration_ms: None,
            }),
        );

        let task = tasks.get("migrate").unwrap();
        assert_eq!(task.execution_id.as_deref(), Some("task-1"));
        assert_eq!(task.state, TaskExecutionState::Running);
    }

    #[tokio::test]
    async fn mark_service_ready_emits_service_and_run_events() {
        let state = test_state();
        let services = BTreeMap::from([(
            "api".to_string(),
            ServiceRuntime {
                name: "api".to_string(),
                unit_name: "api.service".to_string(),
                port: Some(3000),
                scheme: "http".to_string(),
                url: Some("http://localhost:3000".to_string()),
                deps: Vec::new(),
                readiness: ReadinessSpec::new(ReadinessKind::Tcp),
                log_path: PathBuf::from("/tmp/api.log"),
                cwd: PathBuf::from("/tmp"),
                env: BTreeMap::new(),
                state: ServiceState::Starting,
                last_failure: None,
                health: None,
                last_started_at: Some(now_rfc3339()),
                watch_hash: Some("hash".to_string()),
                watch_patterns: Vec::new(),
                ignore_patterns: Vec::new(),
                watch_extra_files: Vec::new(),
                watch_fingerprint: Vec::new(),
                auto_restart: false,
                watch_paused: false,
                watch_handle: None,
            },
        )]);

        {
            let mut guard = state.state.lock().await;
            guard.runs.insert(
                "run-1".to_string(),
                RunState {
                    run_id: "run-1".to_string(),
                    stack: "dev".to_string(),
                    project_dir: PathBuf::from("/tmp/project"),
                    base_env: BTreeMap::new(),
                    services,
                    state: RunLifecycle::Starting,
                    created_at: now_rfc3339(),
                    stopped_at: None,
                },
            );
        }

        let mut rx = state.event_tx.subscribe();
        mark_service_ready(&state, "run-1", "api").await.unwrap();

        assert_eq!(
            rx.recv().await.unwrap(),
            DaemonEvent::Service(DaemonServiceEvent {
                kind: DaemonServiceEventKind::StateChanged,
                run_id: "run-1".to_string(),
                service: "api".to_string(),
                state: ServiceState::Ready,
            })
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            DaemonEvent::Run(DaemonRunEvent {
                kind: DaemonRunEventKind::StateChanged,
                run_id: "run-1".to_string(),
                state: Some(RunLifecycle::Running),
                stack: None,
                project_dir: None,
            })
        );
    }

    #[test]
    fn read_new_log_events_parses_incremental_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"level\":\"info\",\"msg\":\"ready\",\"code\":200}",
        )
        .unwrap();

        let mut cursor = LogTailCursor { offset: 0 };
        let events = read_new_log_events("run-1", &log_path, &mut cursor).unwrap();
        assert!(events.is_empty());
        assert_eq!(cursor.offset, 0);

        std::fs::write(
            &log_path,
            concat!(
                "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"level\":\"info\",\"msg\":\"ready\",\"code\":200}\n",
                "{\"time\":\"2025-01-01T00:00:01Z\",\"stream\":\"stderr\",\"level\":\"error\",\"msg\":\"boom\",\"requestId\":\"abc\"}\n"
            ),
        )
        .unwrap();

        let events = read_new_log_events("run-1", &log_path, &mut cursor).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].service, "api");
        assert_eq!(events[0].message, "ready");
        assert_eq!(
            events[0].attributes.get("code").map(String::as_str),
            Some("200")
        );
        assert_eq!(events[1].stream, "stderr");
        assert_eq!(events[1].level, "error");
        assert_eq!(
            events[1].attributes.get("requestid").map(String::as_str),
            Some("abc")
        );
    }

    #[test]
    fn recent_stderr_lines_from_file_returns_latest_stderr_entries() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("api.log");
        let large_stdout = "x".repeat(70_000);
        let content = [
            "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"msg\":\"ready\"}".to_string(),
            "{\"time\":\"2025-01-01T00:00:01Z\",\"stream\":\"stderr\",\"msg\":\"first error\"}".to_string(),
            format!(
                "{{\"time\":\"2025-01-01T00:00:02Z\",\"stream\":\"stdout\",\"msg\":\"{large_stdout}\"}}"
            ),
            "{\"time\":\"2025-01-01T00:00:03Z\",\"stream\":\"stderr\",\"msg\":\"second error\"}".to_string(),
            "{\"time\":\"2025-01-01T00:00:04Z\",\"stream\":\"stderr\",\"msg\":\"third error\"}".to_string(),
        ]
        .join("\n");
        std::fs::write(&log_path, content).unwrap();

        let lines = recent_stderr_lines_from_file(&log_path, 2).unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.message.as_str())
                .collect::<Vec<_>>(),
            vec!["second error", "third error"]
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| line.timestamp.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("2025-01-01T00:00:03Z"), Some("2025-01-01T00:00:04Z")]
        );
    }

    #[test]
    fn recent_stderr_lines_from_file_returns_empty_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let lines = recent_stderr_lines_from_file(&dir.path().join("missing.log"), 3).unwrap();
        assert!(lines.is_empty());
    }

    #[test]
    fn stale_agent_session_cleanup_removes_dead_pids() {
        let mut sessions = BTreeMap::new();
        register_agent_session_state(
            &mut sessions,
            AgentSessionRegisterRequest {
                agent_id: "alive".to_string(),
                project_dir: "/tmp/project".to_string(),
                stack: None,
                command: "claude".to_string(),
                pid: std::process::id(),
            },
        );
        register_agent_session_state(
            &mut sessions,
            AgentSessionRegisterRequest {
                agent_id: "dead".to_string(),
                project_dir: "/tmp/project".to_string(),
                stack: None,
                command: "claude".to_string(),
                pid: u32::MAX,
            },
        );

        cleanup_stale_agent_sessions(&mut sessions);

        assert!(sessions.contains_key("alive"));
        assert!(!sessions.contains_key("dead"));
    }
}

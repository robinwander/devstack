use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::manifest::{RunLifecycle, ServiceState};

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct UpRequest {
    pub stack: String,
    pub project_dir: String,
    pub run_id: Option<String>,
    pub file: Option<String>,
    pub no_wait: bool,
    #[serde(default)]
    pub new_run: bool,
    #[serde(default)]
    pub force: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct DownRequest {
    pub run_id: String,
    pub purge: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct KillRequest {
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RestartServiceRequest {
    pub service: String,
    #[serde(default)]
    pub no_wait: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct WatchControlRequest {
    #[serde(default)]
    pub service: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct WatchServiceStatus {
    pub auto_restart: bool,
    pub active: bool,
    pub paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RunWatchResponse {
    pub run_id: String,
    pub services: BTreeMap<String, WatchServiceStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct GcRequest {
    pub older_than: Option<String>,
    pub all: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct SetNavigationIntentRequest {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub last: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct NavigationIntent {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub last: Option<usize>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct NavigationIntentResponse {
    pub intent: Option<NavigationIntent>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct PingResponse {
    pub ok: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentSessionRegisterRequest {
    pub agent_id: String,
    pub project_dir: String,
    #[serde(default)]
    pub stack: Option<String>,
    pub command: String,
    pub pid: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentSession {
    pub agent_id: String,
    pub project_dir: String,
    #[serde(default)]
    pub stack: Option<String>,
    pub command: String,
    pub pid: u32,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentSessionMessageRequest {
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentSessionMessageResponse {
    pub queued: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentSessionPollResponse {
    pub messages: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct LatestAgentSessionQuery {
    pub project_dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct LatestAgentSessionResponse {
    pub session: Option<AgentSession>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ShareAgentMessageRequest {
    pub project_dir: String,
    pub command: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ShareAgentMessageResponse {
    pub agent_id: String,
    pub queued: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RunSummary {
    pub run_id: String,
    pub stack: String,
    pub project_dir: String,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RunListResponse {
    pub runs: Vec<RunSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceResponse {
    pub port: Option<u16>,
    pub url: Option<String>,
    pub state: ServiceState,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RunResponse {
    pub run_id: String,
    pub project_dir: String,
    pub stack: String,
    pub manifest_path: String,
    pub services: BTreeMap<String, ServiceResponse>,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct SystemdStatus {
    pub active_state: String,
    pub sub_state: String,
    pub result: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthStatus {
    pub passes: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
    pub last_check_at: Option<String>,
    pub last_ok: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthCheckStats {
    pub passes: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
    pub last_check_at: Option<String>,
    pub last_ok: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RecentErrorLine {
    pub timestamp: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceStatus {
    pub desired: String,
    pub systemd: Option<SystemdStatus>,
    pub ready: bool,
    pub state: ServiceState,
    pub last_failure: Option<String>,
    pub health: Option<HealthStatus>,
    #[serde(default)]
    pub health_check_stats: Option<HealthCheckStats>,
    #[serde(default)]
    pub uptime_seconds: Option<u64>,
    #[serde(default)]
    pub recent_errors: Vec<RecentErrorLine>,
    pub url: Option<String>,
    #[serde(default)]
    pub auto_restart: bool,
    #[serde(default)]
    pub watch_paused: bool,
    #[serde(default)]
    pub watch_active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RunStatusResponse {
    pub run_id: String,
    pub stack: String,
    pub project_dir: String,
    pub state: RunLifecycle,
    pub services: BTreeMap<String, ServiceStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskExecutionState {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct TaskExecutionSummary {
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    pub state: TaskExecutionState,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct TasksResponse {
    pub tasks: Vec<TaskExecutionSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct StartTaskRequest {
    pub project_dir: String,
    #[serde(default)]
    pub file: Option<String>,
    pub task: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct StartTaskResponse {
    pub execution_id: String,
    pub task: String,
    #[serde(default)]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct TaskStatusResponse {
    pub execution_id: String,
    pub task: String,
    pub state: TaskExecutionState,
    pub project_dir: String,
    #[serde(default)]
    pub run_id: Option<String>,
    pub started_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct GlobalSummary {
    pub key: String,
    pub name: String,
    pub project_dir: String,
    pub state: RunLifecycle,
    pub port: Option<u16>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct GlobalsResponse {
    pub globals: Vec<GlobalSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonEvent {
    Run(DaemonRunEvent),
    Service(DaemonServiceEvent),
    Task(DaemonTaskEvent),
    Global(DaemonGlobalEvent),
    Log(DaemonLogEvent),
}

impl DaemonEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Run(_) => "run",
            Self::Service(_) => "service",
            Self::Task(_) => "task",
            Self::Global(_) => "global",
            Self::Log(_) => "log",
        }
    }

    pub fn payload_json(&self) -> serde_json::Result<String> {
        match self {
            Self::Run(event) => serde_json::to_string(event),
            Self::Service(event) => serde_json::to_string(event),
            Self::Task(event) => serde_json::to_string(event),
            Self::Global(event) => serde_json::to_string(event),
            Self::Log(event) => serde_json::to_string(event),
        }
    }

    pub fn should_deliver(&self, run_id: Option<&str>) -> bool {
        match self {
            Self::Log(event) => run_id == Some(event.run_id.as_str()),
            Self::Task(event) => match run_id {
                Some(filter) => event.run_id.as_deref() == Some(filter),
                None => true,
            },
            _ => true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonRunEventKind {
    Created,
    StateChanged,
    Removed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonRunEvent {
    pub kind: DaemonRunEventKind,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<RunLifecycle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonServiceEventKind {
    StateChanged,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonServiceEvent {
    pub kind: DaemonServiceEventKind,
    pub run_id: String,
    pub service: String,
    pub state: ServiceState,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonTaskEventKind {
    Started,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonTaskEvent {
    pub kind: DaemonTaskEventKind,
    pub execution_id: String,
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub state: TaskExecutionState,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonGlobalEventKind {
    StateChanged,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonGlobalEvent {
    pub kind: DaemonGlobalEventKind,
    pub key: String,
    pub state: RunLifecycle,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonLogEvent {
    pub run_id: String,
    pub service: String,
    pub ts: String,
    pub stream: String,
    pub level: String,
    pub message: String,
    pub raw: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct DoctorResponse {
    pub checks: Vec<DoctorCheck>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct GcResponse {
    pub removed_runs: Vec<String>,
    pub removed_globals: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct LogsQuery {
    #[serde(default, alias = "tail")]
    pub last: Option<usize>,
    #[serde(default)]
    pub since: Option<String>,
    /// Tantivy query string (supports boolean ops, phrases, etc.)
    #[serde(default, alias = "q")]
    pub search: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    /// Filter by stream: stdout, stderr
    #[serde(default)]
    pub stream: Option<String>,
    /// Cursor: return log lines with seq > after
    #[serde(default)]
    pub after: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct LogsResponse {
    pub lines: Vec<String>,
    pub truncated: bool,
    pub total: usize,
    pub error_count: usize,
    pub warn_count: usize,
    /// Cursor for follow-style polling (`after` for the next request)
    pub next_after: Option<u64>,
    /// Total matches for the requested filters (may exceed `lines.len()` due to `last`)
    pub matched_total: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct LogEntry {
    pub ts: String,
    pub service: String,
    pub stream: String,
    pub level: String,
    pub message: String,
    pub raw: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub attributes: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct LogViewQuery {
    #[serde(default, alias = "tail")]
    pub last: Option<usize>,
    #[serde(default)]
    pub since: Option<String>,
    /// Tantivy query string (supports boolean ops, phrases, etc.)
    #[serde(default, alias = "q")]
    pub search: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default = "default_true")]
    pub include_entries: bool,
    #[serde(default)]
    pub include_facets: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct FacetValueCount {
    pub value: String,
    pub count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct FacetFilter {
    pub field: String,
    pub kind: String,
    pub values: Vec<FacetValueCount>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct LogViewResponse {
    pub entries: Vec<LogEntry>,
    pub truncated: bool,
    pub total: usize,
    pub filters: Vec<FacetFilter>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ProjectSummary {
    pub id: String,
    pub path: String,
    pub name: String,
    pub stacks: Vec<String>,
    pub last_used: Option<String>,
    pub config_exists: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ProjectsResponse {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RegisterProjectRequest {
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RegisterProjectResponse {
    pub project: ProjectSummary,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct SourceSummary {
    pub name: String,
    pub paths: Vec<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct SourcesResponse {
    pub sources: Vec<SourceSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AddSourceRequest {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AddSourceResponse {
    pub source: SourceSummary,
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonEvent, DaemonTaskEvent, DaemonTaskEventKind, LogViewQuery, LogsQuery,
        TaskExecutionState,
    };

    #[test]
    fn logs_query_accepts_new_and_legacy_params() {
        let modern: LogsQuery = serde_json::from_value(serde_json::json!({
            "last": 25,
            "search": "timeout"
        }))
        .unwrap();
        assert_eq!(modern.last, Some(25));
        assert_eq!(modern.search.as_deref(), Some("timeout"));

        let legacy: LogsQuery = serde_json::from_value(serde_json::json!({
            "tail": 10,
            "q": "error"
        }))
        .unwrap();
        assert_eq!(legacy.last, Some(10));
        assert_eq!(legacy.search.as_deref(), Some("error"));
    }

    #[test]
    fn log_search_query_accepts_new_and_legacy_params() {
        let modern: LogViewQuery = serde_json::from_value(serde_json::json!({
            "last": 50,
            "search": "worker",
            "service": "api",
            "include_facets": true
        }))
        .unwrap();
        assert_eq!(modern.last, Some(50));
        assert_eq!(modern.search.as_deref(), Some("worker"));
        assert_eq!(modern.service.as_deref(), Some("api"));
        assert!(modern.include_entries);
        assert!(modern.include_facets);

        let legacy: LogViewQuery = serde_json::from_value(serde_json::json!({
            "tail": 5,
            "q": "panic"
        }))
        .unwrap();
        assert_eq!(legacy.last, Some(5));
        assert_eq!(legacy.search.as_deref(), Some("panic"));
        assert!(legacy.include_entries);
        assert!(!legacy.include_facets);
    }

    #[test]
    fn task_events_respect_run_filters() {
        let event = DaemonEvent::Task(DaemonTaskEvent {
            kind: DaemonTaskEventKind::Started,
            execution_id: "task-1".to_string(),
            task: "migrate".to_string(),
            run_id: Some("run-1".to_string()),
            state: TaskExecutionState::Running,
            started_at: "2025-01-01T00:00:00Z".to_string(),
            finished_at: None,
            exit_code: None,
            duration_ms: None,
        });

        assert!(event.should_deliver(None));
        assert!(event.should_deliver(Some("run-1")));
        assert!(!event.should_deliver(Some("run-2")));
    }
}

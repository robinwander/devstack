use utoipa::OpenApi;

use crate::api::{
    AddSourceRequest, AddSourceResponse, AgentSession, AgentSessionMessageRequest,
    AgentSessionMessageResponse, AgentSessionPollResponse, AgentSessionRegisterRequest,
    DownRequest, ErrorResponse, FacetFilter, FacetValueCount, GcRequest, GcResponse, GlobalSummary,
    GlobalsResponse, HealthStatus, KillRequest, LatestAgentSessionResponse, LogEntry, LogViewQuery,
    LogViewResponse, LogsQuery, LogsResponse, NavigationIntent, NavigationIntentResponse,
    PingResponse, RestartServiceRequest, RunListResponse, RunStatusResponse, RunSummary,
    RunWatchResponse, ServiceStatus, SetNavigationIntentRequest, ShareAgentMessageRequest,
    ShareAgentMessageResponse, SourceSummary, SourcesResponse, SystemdStatus, TaskExecutionSummary,
    TasksResponse, UpRequest, WatchControlRequest, WatchServiceStatus,
};
use crate::manifest::{RunLifecycle, RunManifest, ServiceManifest, ServiceState};

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::daemon::ping,
        crate::daemon::register_agent_session,
        crate::daemon::unregister_agent_session,
        crate::daemon::post_agent_message,
        crate::daemon::poll_agent_messages,
        crate::daemon::get_latest_agent_session,
        crate::daemon::share_agent_message,
        crate::daemon::up,
        crate::daemon::down,
        crate::daemon::kill,
        crate::daemon::restart_service,
        crate::daemon::status,
        crate::daemon::run_tasks,
        crate::daemon::watch_status,
        crate::daemon::watch_pause,
        crate::daemon::watch_resume,
        crate::daemon::logs,
        crate::daemon::logs_view,
        crate::daemon::list_runs,
        crate::daemon::list_globals,
        crate::daemon::list_sources,
        crate::daemon::add_source,
        crate::daemon::remove_source,
        crate::daemon::source_logs_view,
        crate::daemon::set_navigation_intent,
        crate::daemon::get_navigation_intent,
        crate::daemon::clear_navigation_intent,
        crate::daemon::gc
    ),
    components(
        schemas(
            UpRequest,
            DownRequest,
            KillRequest,
            RestartServiceRequest,
            WatchControlRequest,
            GcRequest,
            PingResponse,
            AgentSessionRegisterRequest,
            AgentSession,
            AgentSessionMessageRequest,
            AgentSessionMessageResponse,
            AgentSessionPollResponse,
            LatestAgentSessionResponse,
            ShareAgentMessageRequest,
            ShareAgentMessageResponse,
            RunSummary,
            RunListResponse,
            RunManifest,
            ServiceManifest,
            RunStatusResponse,
            TasksResponse,
            TaskExecutionSummary,
            RunWatchResponse,
            ServiceStatus,
            WatchServiceStatus,
            HealthStatus,
            SystemdStatus,
            GlobalsResponse,
            GlobalSummary,
            GcResponse,
            LogsQuery,
            LogsResponse,
            SetNavigationIntentRequest,
            NavigationIntent,
            NavigationIntentResponse,
            LogEntry,
            LogViewQuery,
            FacetValueCount,
            FacetFilter,
            LogViewResponse,
            SourceSummary,
            SourcesResponse,
            AddSourceRequest,
            AddSourceResponse,
            ErrorResponse,
            RunLifecycle,
            ServiceState
        )
    ),
    tags(
        (name = "daemon", description = "Devstack daemon API")
    )
)]
pub struct ApiDoc;

pub fn openapi() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

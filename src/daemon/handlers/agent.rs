use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
};

use crate::api::{
    AgentSession, AgentSessionMessageRequest, AgentSessionMessageResponse,
    AgentSessionPollResponse, AgentSessionRegisterRequest, LatestAgentSessionQuery,
    LatestAgentSessionResponse, ShareAgentMessageRequest, ShareAgentMessageResponse,
};
use crate::app::commands;
use crate::app::error::AppError;
use crate::app::queries;
use crate::daemon::router::DaemonState;

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
pub async fn register_agent_session(
    State(state): State<DaemonState>,
    Json(request): Json<AgentSessionRegisterRequest>,
) -> Result<Json<AgentSession>, AppError> {
    Ok(Json(
        commands::agent::register_agent_session(&state.app, request).await,
    ))
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
pub async fn unregister_agent_session(
    State(state): State<DaemonState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    commands::agent::unregister_agent_session(&state.app, &agent_id).await?;
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
pub async fn post_agent_message(
    State(state): State<DaemonState>,
    AxumPath(agent_id): AxumPath<String>,
    Json(request): Json<AgentSessionMessageRequest>,
) -> Result<Json<AgentSessionMessageResponse>, AppError> {
    Ok(Json(
        commands::agent::post_agent_message(&state.app, &agent_id, request).await?,
    ))
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
pub async fn poll_agent_messages(
    State(state): State<DaemonState>,
    AxumPath(agent_id): AxumPath<String>,
) -> Result<Json<AgentSessionPollResponse>, AppError> {
    Ok(Json(
        commands::agent::poll_agent_messages(&state.app, &agent_id).await?,
    ))
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
pub async fn get_latest_agent_session(
    State(state): State<DaemonState>,
    Query(query): Query<LatestAgentSessionQuery>,
) -> Result<Json<LatestAgentSessionResponse>, AppError> {
    Ok(Json(
        queries::agent::latest_agent_session(&state.app, &query.project_dir).await,
    ))
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
pub async fn share_agent_message(
    State(state): State<DaemonState>,
    Json(request): Json<ShareAgentMessageRequest>,
) -> Result<Json<ShareAgentMessageResponse>, AppError> {
    Ok(Json(
        commands::agent::share_agent_message(&state.app, request).await?,
    ))
}

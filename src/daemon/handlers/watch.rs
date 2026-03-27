use axum::{
    Json, Json as AxumJson,
    extract::{Path as AxumPath, State},
};

use crate::api::{RunWatchResponse, WatchControlRequest};
use crate::app::commands;
use crate::app::error::AppError;
use crate::app::queries;
use crate::daemon::router::DaemonState;

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/watch",
    params(("run_id" = String, Path, description = "Run id")),
    responses((status = 200, description = "Watch status", body = RunWatchResponse)),
    tag = "daemon"
)]
pub async fn watch_status(
    State(state): State<DaemonState>,
    AxumPath(run_id): AxumPath<String>,
) -> Result<Json<RunWatchResponse>, AppError> {
    Ok(Json(
        queries::watch::build_watch_status(&state.app, &run_id).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/watch/pause",
    request_body = WatchControlRequest,
    params(("run_id" = String, Path, description = "Run id")),
    responses((status = 200, description = "Updated watch status", body = RunWatchResponse)),
    tag = "daemon"
)]
pub async fn watch_pause(
    State(state): State<DaemonState>,
    AxumPath(run_id): AxumPath<String>,
    AxumJson(request): AxumJson<WatchControlRequest>,
) -> Result<Json<RunWatchResponse>, AppError> {
    Ok(Json(
        commands::watch::pause_watch(&state.app, &run_id, request.service.as_deref()).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/watch/resume",
    request_body = WatchControlRequest,
    params(("run_id" = String, Path, description = "Run id")),
    responses((status = 200, description = "Updated watch status", body = RunWatchResponse)),
    tag = "daemon"
)]
pub async fn watch_resume(
    State(state): State<DaemonState>,
    AxumPath(run_id): AxumPath<String>,
    AxumJson(request): AxumJson<WatchControlRequest>,
) -> Result<Json<RunWatchResponse>, AppError> {
    Ok(Json(
        commands::watch::resume_watch(&state.app, &run_id, request.service.as_deref()).await?,
    ))
}

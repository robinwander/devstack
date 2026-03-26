use axum::{Json, extract::{Path as AxumPath, State}};

use crate::api::{DownRequest, KillRequest, RestartServiceRequest, RunListResponse, RunStatusResponse, UpRequest};
use crate::app::commands;
use crate::app::queries;
use crate::daemon::error::AppError;
use crate::daemon::router::DaemonState;
use crate::manifest::RunManifest;

#[utoipa::path(
    post,
    path = "/v1/runs/up",
    request_body = UpRequest,
    responses((status = 200, description = "Run created or refreshed", body = RunManifest)),
    tag = "daemon"
)]
pub async fn up(
    State(state): State<DaemonState>,
    Json(request): Json<UpRequest>,
) -> Result<Json<RunManifest>, AppError> {
    Ok(Json(commands::runs::up(&state.app, request).await?))
}

#[utoipa::path(
    post,
    path = "/v1/runs/down",
    request_body = DownRequest,
    responses((status = 200, description = "Run stopped", body = RunManifest)),
    tag = "daemon"
)]
pub async fn down(
    State(state): State<DaemonState>,
    Json(request): Json<DownRequest>,
) -> Result<Json<RunManifest>, AppError> {
    Ok(Json(commands::runs::down(&state.app, &request.run_id, request.purge).await?))
}

#[utoipa::path(
    post,
    path = "/v1/runs/kill",
    request_body = KillRequest,
    responses((status = 200, description = "Run killed", body = RunManifest)),
    tag = "daemon"
)]
pub async fn kill(
    State(state): State<DaemonState>,
    Json(request): Json<KillRequest>,
) -> Result<Json<RunManifest>, AppError> {
    Ok(Json(commands::runs::kill(&state.app, &request.run_id).await?))
}

#[utoipa::path(
    post,
    path = "/v1/runs/{run_id}/restart-service",
    request_body = RestartServiceRequest,
    params(("run_id" = String, Path, description = "Run id")),
    responses((status = 200, description = "Service restarted", body = RunManifest)),
    tag = "daemon"
)]
pub async fn restart_service(
    State(state): State<DaemonState>,
    AxumPath(run_id): AxumPath<String>,
    Json(request): Json<RestartServiceRequest>,
) -> Result<Json<RunManifest>, AppError> {
    Ok(Json(commands::restart::restart_service(&state.app, &run_id, &request.service, request.no_wait).await?))
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/status",
    params(("run_id" = String, Path, description = "Run id")),
    responses((status = 200, description = "Run status", body = RunStatusResponse)),
    tag = "daemon"
)]
pub async fn status(
    State(state): State<DaemonState>,
    AxumPath(run_id): AxumPath<String>,
) -> Result<Json<RunStatusResponse>, AppError> {
    Ok(Json(queries::status::build_status(&state.app, &run_id).await?))
}

#[utoipa::path(
    get,
    path = "/v1/runs",
    responses((status = 200, description = "List runs", body = RunListResponse)),
    tag = "daemon"
)]
pub async fn list_runs(
    State(state): State<DaemonState>,
) -> Result<Json<RunListResponse>, AppError> {
    Ok(Json(queries::runs::list_runs(&state.app).await))
}

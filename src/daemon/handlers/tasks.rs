use axum::{
    Json,
    extract::{Path as AxumPath, State},
};

use crate::api::{StartTaskRequest, StartTaskResponse, TaskStatusResponse, TasksResponse};
use crate::app::commands;
use crate::app::error::AppError;
use crate::app::queries;
use crate::daemon::router::DaemonState;

#[utoipa::path(
    post,
    path = "/v1/tasks/run",
    request_body = StartTaskRequest,
    responses((status = 200, description = "Detached task accepted", body = StartTaskResponse)),
    tag = "daemon"
)]
pub async fn start_task(
    State(state): State<DaemonState>,
    Json(request): Json<StartTaskRequest>,
) -> Result<Json<StartTaskResponse>, AppError> {
    Ok(Json(
        commands::tasks::start_task(&state.app, request).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/tasks/{execution_id}",
    params(("execution_id" = String, Path, description = "Task execution id")),
    responses((status = 200, description = "Task execution status", body = TaskStatusResponse)),
    tag = "daemon"
)]
pub async fn task_status(
    State(state): State<DaemonState>,
    AxumPath(execution_id): AxumPath<String>,
) -> Result<Json<TaskStatusResponse>, AppError> {
    Ok(Json(
        queries::tasks::task_status(&state.app, &execution_id).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/tasks",
    params(("run_id" = String, Path, description = "Run id")),
    responses((status = 200, description = "Latest task executions for the run", body = TasksResponse)),
    tag = "daemon"
)]
pub async fn run_tasks(
    State(state): State<DaemonState>,
    AxumPath(run_id): AxumPath<String>,
) -> Result<Json<TasksResponse>, AppError> {
    Ok(Json(queries::tasks::run_tasks(&state.app, &run_id).await?))
}

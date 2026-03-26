use std::path::PathBuf;

use anyhow::anyhow;
use axum::{Json, extract::Path as AxumPath};

use crate::api::{ProjectsResponse, RegisterProjectRequest, RegisterProjectResponse};
use crate::daemon::error::AppError;
use crate::projects::ProjectsLedger;

#[utoipa::path(
    get,
    path = "/v1/projects",
    responses((status = 200, description = "List of registered projects", body = ProjectsResponse)),
    tag = "daemon"
)]
pub async fn list_projects() -> Result<Json<ProjectsResponse>, AppError> {
    let ledger = ProjectsLedger::load().map_err(AppError::from)?;
    Ok(Json(ProjectsResponse {
        projects: ledger.to_summaries(),
    }))
}

#[utoipa::path(
    post,
    path = "/v1/projects/register",
    request_body = RegisterProjectRequest,
    responses((status = 200, description = "Project registered", body = RegisterProjectResponse)),
    tag = "daemon"
)]
pub async fn register_project(
    Json(request): Json<RegisterProjectRequest>,
) -> Result<Json<RegisterProjectResponse>, AppError> {
    let path = PathBuf::from(&request.path);
    if !path.exists() {
        return Err(AppError::bad_request(format!(
            "path does not exist: {}",
            request.path
        )));
    }

    let mut ledger = ProjectsLedger::load().map_err(AppError::from)?;
    let id = ledger.register(&path).map_err(AppError::from)?;
    let project = ledger
        .to_summaries()
        .into_iter()
        .find(|project| project.id == id)
        .ok_or_else(|| AppError::Internal(anyhow!("failed to find registered project")))?;

    Ok(Json(RegisterProjectResponse { project }))
}

#[utoipa::path(
    delete,
    path = "/v1/projects/{project_id}",
    params(("project_id" = String, Path, description = "Project ID")),
    responses((status = 200, description = "Project removed")),
    tag = "daemon"
)]
pub async fn remove_project(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut ledger = ProjectsLedger::load().map_err(AppError::from)?;
    let removed = ledger.remove(&project_id).map_err(AppError::from)?;
    if !removed {
        return Err(AppError::not_found(format!("project {} not found", project_id)));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

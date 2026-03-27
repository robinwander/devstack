use axum::{Json, extract::State};

use crate::api::{GcRequest, GcResponse};
use crate::app::commands;
use crate::app::error::AppError;
use crate::daemon::router::DaemonState;

#[utoipa::path(
    post,
    path = "/v1/gc",
    request_body = GcRequest,
    responses((status = 200, description = "Garbage collection result", body = GcResponse)),
    tag = "daemon"
)]
pub async fn gc(
    State(state): State<DaemonState>,
    Json(request): Json<GcRequest>,
) -> Result<Json<GcResponse>, AppError> {
    Ok(Json(commands::gc::run_gc(&state.app, request).await?))
}

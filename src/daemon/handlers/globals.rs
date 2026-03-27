use axum::{Json, extract::State};

use crate::api::GlobalsResponse;
use crate::app::error::AppError;
use crate::app::queries;
use crate::daemon::router::DaemonState;

#[utoipa::path(
    get,
    path = "/v1/globals",
    responses((status = 200, description = "List global services", body = GlobalsResponse)),
    tag = "daemon"
)]
pub async fn list_globals(
    State(state): State<DaemonState>,
) -> Result<Json<GlobalsResponse>, AppError> {
    Ok(Json(queries::globals::list_globals(&state.app).await?))
}

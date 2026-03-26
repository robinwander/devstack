use axum::Json;

use crate::api::PingResponse;

#[utoipa::path(
    get,
    path = "/v1/ping",
    responses(
        (status = 200, description = "Daemon is healthy", body = PingResponse),
        (status = 500, description = "Internal error", body = crate::api::ErrorResponse)
    ),
    tag = "daemon"
)]
pub async fn ping() -> Json<PingResponse> {
    Json(PingResponse { ok: true })
}

use axum::{Json, extract::State};

use crate::api::{NavigationIntentResponse, SetNavigationIntentRequest};
use crate::app::commands;
use crate::app::error::AppError;
use crate::app::queries;
use crate::daemon::router::DaemonState;

#[utoipa::path(
    post,
    path = "/v1/navigation/intent",
    request_body = SetNavigationIntentRequest,
    responses((status = 200, description = "Navigation intent stored", body = NavigationIntentResponse)),
    tag = "daemon"
)]
pub async fn set_navigation_intent(
    State(state): State<DaemonState>,
    Json(request): Json<SetNavigationIntentRequest>,
) -> Result<Json<NavigationIntentResponse>, AppError> {
    let intent = commands::navigation::set_navigation_intent(&state.app, request).await;
    Ok(Json(NavigationIntentResponse {
        intent: Some(intent),
    }))
}

#[utoipa::path(
    get,
    path = "/v1/navigation/intent",
    responses((status = 200, description = "Current navigation intent", body = NavigationIntentResponse)),
    tag = "daemon"
)]
pub async fn get_navigation_intent(
    State(state): State<DaemonState>,
) -> Result<Json<NavigationIntentResponse>, AppError> {
    Ok(Json(
        queries::navigation::get_navigation_intent(&state.app).await,
    ))
}

#[utoipa::path(
    delete,
    path = "/v1/navigation/intent",
    responses((status = 200, description = "Navigation intent cleared")),
    tag = "daemon"
)]
pub async fn clear_navigation_intent(
    State(state): State<DaemonState>,
) -> Result<Json<serde_json::Value>, AppError> {
    commands::navigation::clear_navigation_intent(&state.app).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

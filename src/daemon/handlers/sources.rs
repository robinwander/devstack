use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
};

use crate::api::{
    AddSourceRequest, AddSourceResponse, LogViewQuery, LogViewResponse, SourceSummary,
    SourcesResponse,
};
use crate::app::error::AppError;
use crate::daemon::router::DaemonState;
use crate::daemon::source_ingest::{
    replace_source_index, should_ingest_source_synchronously, source_log_sources,
    spawn_source_backfill, warmup_sources_for_query,
};
use crate::sources::{SourcesLedger, source_run_id};

#[utoipa::path(
    get,
    path = "/v1/sources",
    responses((status = 200, description = "List registered log sources", body = SourcesResponse)),
    tag = "daemon"
)]
pub async fn list_sources() -> Result<Json<SourcesResponse>, AppError> {
    let ledger = SourcesLedger::load().map_err(AppError::from)?;
    Ok(Json(SourcesResponse {
        sources: ledger.list().iter().map(source_summary).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/v1/sources",
    request_body = AddSourceRequest,
    responses((status = 200, description = "Source added", body = AddSourceResponse)),
    tag = "daemon"
)]
pub async fn add_source(
    State(state): State<DaemonState>,
    Json(request): Json<AddSourceRequest>,
) -> Result<Json<AddSourceResponse>, AppError> {
    let mut ledger = SourcesLedger::load().map_err(AppError::from)?;
    ledger
        .add(&request.name, request.paths)
        .map_err(|err: anyhow::Error| AppError::bad_request(err.to_string()))?;

    let source = ledger
        .get(&request.name)
        .cloned()
        .ok_or_else(|| AppError::Internal(anyhow!("source {} was not persisted", request.name)))?;

    let sources = source_log_sources(&ledger, &request.name).map_err(AppError::from)?;
    let index = state.app.log_index.clone();
    let name = request.name.clone();
    if should_ingest_source_synchronously(&sources) {
        tokio::task::spawn_blocking(move || replace_source_index(&index, &name, &sources))
            .await
            .map_err(|err| AppError::Internal(anyhow!("source ingest task failed: {err}")))?
            .map_err(AppError::from)?;
    } else {
        let run_id = source_run_id(&name);
        let delete_index = index.clone();
        tokio::task::spawn_blocking(move || delete_index.delete_run(&run_id))
            .await
            .map_err(|err| AppError::Internal(anyhow!("source cleanup task failed: {err}")))?
            .map_err(AppError::from)?;
        spawn_source_backfill(index, name, sources);
    }

    Ok(Json(AddSourceResponse {
        source: source_summary(&source),
    }))
}

#[utoipa::path(
    delete,
    path = "/v1/sources/{name}",
    params(("name" = String, Path, description = "Source name")),
    responses((status = 200, description = "Source removed")),
    tag = "daemon"
)]
pub async fn remove_source(
    State(state): State<DaemonState>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut ledger = SourcesLedger::load().map_err(AppError::from)?;
    let removed = ledger.remove(&name).map_err(AppError::from)?;
    if !removed {
        return Err(AppError::not_found(format!("source {} not found", name)));
    }

    let index = state.app.log_index.clone();
    let run_id = source_run_id(&name);
    let _: () = tokio::task::spawn_blocking(move || index.delete_run(&run_id))
        .await
        .map_err(|err| AppError::Internal(anyhow!("source cleanup task failed: {err}")))?
        .map_err(AppError::from)?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get,
    path = "/v1/sources/{name}/logs",
    params(("name" = String, Path, description = "Source name")),
    responses((status = 200, description = "Combined source log view", body = LogViewResponse)),
    tag = "daemon"
)]
pub async fn source_logs_view(
    State(state): State<DaemonState>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<LogViewQuery>,
) -> Result<Json<LogViewResponse>, AppError> {
    let ledger = SourcesLedger::load().map_err(AppError::from)?;
    if ledger.get(&name).is_none() {
        return Err(AppError::not_found(format!("source {} not found", name)));
    }

    let sources = source_log_sources(&ledger, &name).map_err(AppError::from)?;
    let run_id = source_run_id(&name);
    let index = state.app.log_index.clone();

    if should_ingest_source_synchronously(&sources) {
        let response: LogViewResponse = tokio::task::spawn_blocking(move || {
            index.ingest_sources(&sources)?;
            index.query_view(&run_id, query)
        })
        .await
        .map_err(|err| AppError::Internal(anyhow!("source log view task failed: {err}")))?
        .map_err(map_log_index_error)?;
        return Ok(Json(response));
    }

    let warmup_sources = warmup_sources_for_query(&sources, &query).map_err(AppError::from)?;
    let query_index = index.clone();
    let response: LogViewResponse = tokio::task::spawn_blocking(move || {
        if !warmup_sources.is_empty() {
            query_index.ingest_sources(&warmup_sources)?;
        }
        query_index.query_view(&run_id, query)
    })
    .await
    .map_err(|err| AppError::Internal(anyhow!("source log view task failed: {err}")))?
    .map_err(map_log_index_error)?;

    spawn_source_backfill(index, name, sources);

    Ok(Json(response))
}

fn source_summary(entry: &crate::sources::SourceEntry) -> SourceSummary {
    SourceSummary {
        name: entry.name.clone(),
        paths: entry.paths.clone(),
        created_at: entry.created_at.clone(),
    }
}

fn map_log_index_error(err: anyhow::Error) -> AppError {
    let message = err.to_string();
    if let Some(rest) = message.strip_prefix("bad_query:") {
        return AppError::bad_request(rest.trim().to_string());
    }
    AppError::Internal(err)
}

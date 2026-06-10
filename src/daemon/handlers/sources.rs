use std::time::Duration;

use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
};

use crate::api::{
    AddSourceRequest, AddSourceResponse, LogViewQuery, LogViewResponse, SourceStatusResponse,
    SourceSummary, SourcesResponse,
};
use crate::app::error::AppError;
use crate::daemon::bootstrap::log_index_max_age;
use crate::daemon::router::DaemonState;
use crate::daemon::source_ingest::{
    replace_source_index, retention_cutoff_nanos, should_ingest_source_synchronously,
    source_log_sources, spawn_source_backfill, warmup_sources_for_query,
};
use crate::sources::{SourceEntry, source_retention_duration, source_run_id};

#[utoipa::path(
    get,
    path = "/v1/sources",
    responses((status = 200, description = "List registered log sources", body = SourcesResponse)),
    tag = "daemon"
)]
pub async fn list_sources(
    State(state): State<DaemonState>,
) -> Result<Json<SourcesResponse>, AppError> {
    let default_retention = log_index_max_age();
    let sources = state
        .app
        .sources
        .list()
        .map_err(AppError::from)?
        .iter()
        .map(|entry| source_summary(entry, default_retention))
        .collect();
    Ok(Json(SourcesResponse { sources }))
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
    let retention_seconds = parse_retention_seconds(request.retention.as_deref())?;
    let source = state
        .app
        .sources
        .add(&request.name, request.paths, retention_seconds)
        .map_err(|err: anyhow::Error| AppError::bad_request(err.to_string()))?;

    let sources = source_log_sources(&state.app.sources, &request.name).map_err(AppError::from)?;
    let retention = source_retention_duration(&source, log_index_max_age());
    let min_ts_nanos = Some(retention_cutoff_nanos(retention));
    let index = state.app.log_index.clone();
    let registry = state.app.sources.clone();
    let name = request.name.clone();
    if should_ingest_source_synchronously(&sources) {
        tokio::task::spawn_blocking(move || {
            replace_source_index(&index, &registry, &name, &sources, retention, min_ts_nanos)
        })
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
        spawn_source_backfill(index, registry, name, sources, retention, min_ts_nanos);
    }

    Ok(Json(AddSourceResponse {
        source: source_summary(&source, log_index_max_age()),
    }))
}

#[utoipa::path(
    get,
    path = "/v1/sources/{name}",
    params(("name" = String, Path, description = "Source name")),
    responses((status = 200, description = "Source indexing status", body = SourceStatusResponse)),
    tag = "daemon"
)]
pub async fn source_status(
    State(state): State<DaemonState>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<SourceStatusResponse>, AppError> {
    let source = state
        .app
        .sources
        .get(&name)
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::not_found(format!("source {} not found", name)))?;
    let index = state
        .app
        .sources
        .index_state(&name)
        .map_err(AppError::from)?;
    Ok(Json(SourceStatusResponse {
        source: source_summary(&source, log_index_max_age()),
        index,
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
    let removed = state.app.sources.remove(&name).map_err(AppError::from)?;
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
    let source = state
        .app
        .sources
        .get(&name)
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::not_found(format!("source {} not found", name)))?;

    let sources = source_log_sources(&state.app.sources, &name).map_err(AppError::from)?;
    let retention = source_retention_duration(&source, log_index_max_age());
    let min_ts_nanos = Some(retention_cutoff_nanos(retention));
    let run_id = source_run_id(&name);
    let index = state.app.log_index.clone();
    let registry = state.app.sources.clone();

    if should_ingest_source_synchronously(&sources) {
        let response: LogViewResponse = tokio::task::spawn_blocking(move || {
            index.ingest_sources_after(&sources, min_ts_nanos)?;
            index.query_view(&run_id, query)
        })
        .await
        .map_err(|err| AppError::Internal(anyhow!("source log view task failed: {err}")))?
        .map_err(map_log_index_error)?;
        return Ok(Json(response));
    }

    let warmup_sources =
        warmup_sources_for_query(&sources, &query, min_ts_nanos).map_err(AppError::from)?;
    let query_index = index.clone();
    let response: LogViewResponse = tokio::task::spawn_blocking(move || {
        if !warmup_sources.is_empty() && !query_index.sources_are_current(&warmup_sources) {
            query_index.ingest_sources_after(&warmup_sources, min_ts_nanos)?;
        }
        query_index.query_view(&run_id, query)
    })
    .await
    .map_err(|err| AppError::Internal(anyhow!("source log view task failed: {err}")))?
    .map_err(map_log_index_error)?;

    if !index.sources_are_current(&sources) {
        spawn_source_backfill(index, registry, name, sources, retention, min_ts_nanos);
    }

    Ok(Json(response))
}

fn source_summary(entry: &SourceEntry, default_retention: Duration) -> SourceSummary {
    SourceSummary {
        name: entry.name.clone(),
        paths: entry.paths.clone(),
        created_at: entry.created_at.clone(),
        retention_seconds: entry.retention_seconds,
        effective_retention_seconds: source_retention_duration(entry, default_retention).as_secs(),
    }
}

fn parse_retention_seconds(retention: Option<&str>) -> Result<Option<u64>, AppError> {
    let Some(retention) = retention else {
        return Ok(None);
    };
    let retention = retention.trim();
    if retention.is_empty() || retention.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    let duration = humantime::parse_duration(retention).map_err(|err| {
        AppError::bad_request(format!("invalid source retention {retention:?}: {err}"))
    })?;
    Ok(Some(duration.as_secs()))
}

fn map_log_index_error(err: anyhow::Error) -> AppError {
    let message = err.to_string();
    if let Some(rest) = message.strip_prefix("bad_query:") {
        return AppError::bad_request(rest.trim().to_string());
    }
    AppError::Internal(err)
}

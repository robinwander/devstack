use anyhow::anyhow;
use axum::{Json, extract::{Path as AxumPath, Query, State}};

use crate::api::{AddSourceRequest, AddSourceResponse, LogViewQuery, LogViewResponse, SourceSummary, SourcesResponse};
use crate::daemon::error::AppError;
use crate::daemon::router::DaemonState;
use crate::infra::logs::index::LogSource;
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

    let index = state.app.log_index.clone();
    let name = request.name.clone();
    tokio::task::spawn_blocking(move || {
        let run_id = source_run_id(&name);
        index.delete_run(&run_id)?;
        let ledger = SourcesLedger::load()?;
        let sources = source_log_sources(&ledger, &name)?;
        if !sources.is_empty() {
            index.ingest_sources(&sources)?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|err| AppError::Internal(anyhow!("source ingest task failed: {err}")))?
    .map_err(AppError::from)?;

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

    let run_id = source_run_id(&name);
    let index = state.app.log_index.clone();
    let response: LogViewResponse = tokio::task::spawn_blocking(move || index.query_view(&run_id, query))
        .await
        .map_err(|err| AppError::Internal(anyhow!("source log view task failed: {err}")))?
        .map_err(map_log_index_error)?;

    Ok(Json(response))
}

fn source_summary(entry: &crate::sources::SourceEntry) -> SourceSummary {
    SourceSummary {
        name: entry.name.clone(),
        paths: entry.paths.clone(),
        created_at: entry.created_at.clone(),
    }
}

fn source_log_sources(ledger: &SourcesLedger, name: &str) -> anyhow::Result<Vec<LogSource>> {
    let run_id = source_run_id(name);
    Ok(ledger
        .resolve_log_sources(name)?
        .into_iter()
        .map(|item| LogSource {
            run_id: run_id.clone(),
            service: item.service,
            path: item.path,
        })
        .collect())
}

fn map_log_index_error(err: anyhow::Error) -> AppError {
    let message = err.to_string();
    if let Some(rest) = message.strip_prefix("bad_query:") {
        return AppError::bad_request(rest.trim().to_string());
    }
    AppError::Internal(err)
}

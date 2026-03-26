use anyhow::{Context, anyhow};
use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
};

use crate::api::{LogViewQuery, LogViewResponse, LogsQuery, LogsResponse};
use crate::app::queries;
use crate::daemon::error::AppError;
use crate::daemon::router::DaemonState;
use crate::ids::RunId;
use crate::infra::logs::index::LogSource;
use crate::paths;

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/logs/{service}",
    params(("run_id" = String, Path, description = "Run id"), ("service" = String, Path, description = "Service name")),
    responses((status = 200, description = "Log lines", body = LogsResponse)),
    tag = "daemon"
)]
pub async fn logs(
    State(state): State<DaemonState>,
    AxumPath((run_id, service)): AxumPath<(String, String)>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, AppError> {
    Ok(Json(
        queries::logs::read_service_logs(&state.app, &run_id, &service, query).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/runs/{run_id}/logs",
    params(("run_id" = String, Path, description = "Run id")),
    responses((status = 200, description = "Combined log view", body = LogViewResponse)),
    tag = "daemon"
)]
pub async fn logs_view(
    State(state): State<DaemonState>,
    AxumPath(run_id): AxumPath<String>,
    Query(query): Query<LogViewQuery>,
) -> Result<Json<LogViewResponse>, AppError> {
    let mut sources = state
        .app
        .runs
        .with_run(&run_id, |run| {
            run.services
                .iter()
                .map(|(service, record)| LogSource {
                    run_id: run_id.clone(),
                    service: service.clone(),
                    path: record.launch.log_path.clone(),
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))?;
    sources.extend(discover_task_log_sources(&run_id).map_err(AppError::from)?);

    let index = state.app.log_index.clone();
    let response = tokio::task::spawn_blocking(move || {
        index.ingest_sources(&sources)?;
        index.query_view(&run_id, query)
    })
    .await
    .map_err(|err| AppError::Internal(anyhow!("log view task failed: {err}")))?
    .map_err(map_log_index_error)?;

    Ok(Json(response))
}

fn discover_task_log_sources(run_id: &str) -> anyhow::Result<Vec<LogSource>> {
    let task_logs_dir = paths::run_task_logs_dir(&RunId::new(run_id))?;
    if !task_logs_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&task_logs_dir)
        .with_context(|| format!("read task log dir {}", task_logs_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        sources.push(LogSource {
            run_id: run_id.to_string(),
            service: format!("task:{name}"),
            path,
        });
    }
    sources.sort_by(|left, right| left.service.cmp(&right.service));
    Ok(sources)
}

fn map_log_index_error(err: anyhow::Error) -> AppError {
    let message = err.to_string();
    if let Some(rest) = message.strip_prefix("bad_query:") {
        return AppError::bad_request(rest.trim().to_string());
    }
    AppError::Internal(err)
}

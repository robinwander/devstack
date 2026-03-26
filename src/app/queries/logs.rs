use anyhow::anyhow;

use crate::api::{LogsQuery, LogsResponse};
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;

pub async fn read_service_logs(
    app: &AppContext,
    run_id: &str,
    service: &str,
    query: LogsQuery,
) -> AppResult<LogsResponse> {
    let log_path = app
        .runs
        .with_run(run_id, |run| {
            run.services
                .get(service)
                .map(|record| record.launch.log_path.clone())
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))?
        .ok_or_else(|| AppError::not_found(format!("service {service} not found")))?;

    if !log_path.exists() {
        return Ok(LogsResponse {
            lines: vec![],
            truncated: false,
            total: 0,
            error_count: 0,
            warn_count: 0,
            next_after: None,
            matched_total: 0,
        });
    }

    let index = app.log_index.clone();
    let run_id = run_id.to_string();
    let service = service.to_string();
    let response = tokio::task::spawn_blocking(move || {
        index.search_service(&run_id, &service, log_path.as_path(), query)
    })
    .await
    .map_err(|err| AppError::Internal(anyhow!("log search task failed: {err}")))?
    .map_err(map_log_index_error)?;

    Ok(response)
}

fn map_log_index_error(err: anyhow::Error) -> AppError {
    let message = err.to_string();
    if let Some(rest) = message.strip_prefix("bad_query:") {
        return AppError::bad_request(rest.trim().to_string());
    }
    AppError::Internal(err)
}

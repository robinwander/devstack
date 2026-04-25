use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
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
use crate::infra::logs::index::{LogIndex, LogSource};
use crate::logfmt::{contains_ansi, parse_log_line, parse_timestamp_nanos, strip_ansi};
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
        tokio::task::spawn_blocking(move || ingest_source(&index, &name, &sources))
            .await
            .map_err(|err| AppError::Internal(anyhow!("source ingest task failed: {err}")))?
            .map_err(AppError::from)?;
    } else {
        tokio::task::spawn_blocking(move || {
            if let Err(err) = ingest_source(&index, &name, &sources) {
                eprintln!("devstack: background source ingest failed for {name}: {err}");
            }
        });
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

    let run_id = source_run_id(&name);
    let index = state.app.log_index.clone();

    if can_tail_source_directly(&query) {
        let sources = source_log_sources(&ledger, &name).map_err(AppError::from)?;
        if should_ingest_source_synchronously(&sources) {
            let response: LogViewResponse =
                tokio::task::spawn_blocking(move || index.query_view(&run_id, query))
                    .await
                    .map_err(|err| {
                        AppError::Internal(anyhow!("source log view task failed: {err}"))
                    })?
                    .map_err(map_log_index_error)?;
            return Ok(Json(response));
        }
        let tail_index = index.clone();
        let warm_index = index.clone();
        let warm_run_id = run_id.clone();
        let last = query.filter.last.unwrap_or(500);
        let response = tokio::task::spawn_blocking(move || {
            read_source_tail(&sources, last, &tail_index, &run_id)
        })
        .await
        .map_err(|err| AppError::Internal(anyhow!("source tail task failed: {err}")))?
        .map_err(AppError::from)?;
        tokio::task::spawn_blocking(move || warm_index.warm_facets(&warm_run_id));
        return Ok(Json(response));
    }

    let response: LogViewResponse =
        tokio::task::spawn_blocking(move || index.query_view(&run_id, query))
            .await
            .map_err(|err| AppError::Internal(anyhow!("source log view task failed: {err}")))?
            .map_err(map_log_index_error)?;

    Ok(Json(response))
}

const SYNC_SOURCE_INGEST_MAX_BYTES: u64 = 32 * 1024 * 1024;
const SYNC_SOURCE_INGEST_MAX_FILES: usize = 32;

fn ingest_source(index: &LogIndex, name: &str, sources: &[LogSource]) -> anyhow::Result<()> {
    let run_id = source_run_id(name);
    index.delete_run(&run_id)?;
    if !sources.is_empty() {
        index.ingest_sources(sources)?;
    }
    Ok(())
}

fn should_ingest_source_synchronously(sources: &[LogSource]) -> bool {
    if sources.len() > SYNC_SOURCE_INGEST_MAX_FILES {
        return false;
    }

    let total_bytes = sources
        .iter()
        .filter_map(|source| std::fs::metadata(&source.path).ok())
        .try_fold(0u64, |total, metadata| {
            total
                .checked_add(metadata.len())
                .filter(|bytes| *bytes <= SYNC_SOURCE_INGEST_MAX_BYTES)
        });
    total_bytes.is_some()
}

fn can_tail_source_directly(query: &LogViewQuery) -> bool {
    query.include_entries
        && !query.include_facets
        && query.filter.search.as_deref().is_none_or(str::is_empty)
        && query.filter.level.as_deref().is_none_or(str::is_empty)
        && query.filter.stream.as_deref().is_none_or(str::is_empty)
        && query.filter.since.as_deref().is_none_or(str::is_empty)
}

struct SourceTailFile {
    source_ord: usize,
    modified_nanos: i64,
    source: LogSource,
}

fn read_source_tail(
    sources: &[LogSource],
    limit: usize,
    index: &LogIndex,
    run_id: &str,
) -> anyhow::Result<LogViewResponse> {
    if limit == 0 || sources.is_empty() {
        return Ok(LogViewResponse {
            entries: Vec::new(),
            truncated: false,
            total: 0,
            filters: Vec::new(),
        });
    }

    let mut files = sources
        .iter()
        .cloned()
        .enumerate()
        .filter_map(|(source_ord, source)| {
            let metadata = std::fs::metadata(&source.path).ok()?;
            metadata.is_file().then(|| SourceTailFile {
                source_ord,
                modified_nanos: metadata
                    .modified()
                    .ok()
                    .and_then(system_time_nanos)
                    .unwrap_or(0),
                source,
            })
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        right
            .modified_nanos
            .cmp(&left.modified_nanos)
            .then(left.source.path.cmp(&right.source.path))
    });

    let mut candidates = Vec::new();
    for (file_index, file) in files.iter().enumerate() {
        for (seq, raw) in read_last_complete_lines(&file.source.path, limit)?
            .into_iter()
            .enumerate()
        {
            let line = if contains_ansi(&raw) {
                strip_ansi(&raw)
            } else {
                raw
            };
            if line.is_empty() {
                continue;
            }
            let parsed = parse_log_line(&line);
            let ts = parsed.timestamp.unwrap_or_default();
            let ts_nanos = parse_timestamp_nanos(&ts).unwrap_or(0);
            let attributes = parsed
                .json
                .as_ref()
                .map(LogIndex::extract_dynamic_json_fields_from_map)
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            candidates.push((
                ts_nanos,
                file.source_ord,
                seq as u64,
                crate::api::LogEntry {
                    ts,
                    service: file.source.service.clone(),
                    stream: parsed.stream,
                    level: parsed.level,
                    message: parsed.message,
                    raw: line,
                    attributes,
                },
            ));
        }

        sort_tail_candidates_desc(&mut candidates);
        if candidates.len() > limit {
            candidates.truncate(limit);
        }

        let Some(next_file) = files.get(file_index + 1) else {
            break;
        };
        if candidates.len() >= limit
            && let Some(oldest_selected_ts) = candidates.last().map(|candidate| candidate.0)
            && oldest_selected_ts > 0
            && next_file.modified_nanos > 0
            && next_file.modified_nanos < oldest_selected_ts
        {
            break;
        }
    }

    let indexed_total = index
        .query_view(
            run_id,
            LogViewQuery {
                filter: Default::default(),
                service: None,
                include_entries: false,
                include_facets: false,
            },
        )?
        .total;

    let truncated = candidates.len() >= limit;
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.2.cmp(&right.2))
            .then(left.1.cmp(&right.1))
    });

    let total = indexed_total.max(candidates.len());
    Ok(LogViewResponse {
        entries: candidates
            .into_iter()
            .map(|(_, _, _, entry)| entry)
            .collect(),
        truncated,
        total,
        filters: Vec::new(),
    })
}

fn sort_tail_candidates_desc(candidates: &mut [(i64, usize, u64, crate::api::LogEntry)]) {
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(right.2.cmp(&left.2))
            .then(right.1.cmp(&left.1))
    });
}

fn system_time_nanos(time: SystemTime) -> Option<i64> {
    i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}

fn read_last_complete_lines(path: &Path, limit: usize) -> anyhow::Result<Vec<String>> {
    if limit == 0 || !path.exists() {
        return Ok(Vec::new());
    }

    const CHUNK_SIZE: usize = 64 * 1024;
    const MAX_TAIL_BYTES_PER_SOURCE: usize = 2 * 1024 * 1024;

    let mut file =
        File::open(path).with_context(|| format!("open source log {}", path.display()))?;
    let mut offset = file.metadata()?.len();
    let mut bytes = Vec::new();

    while offset > 0
        && bytes.iter().filter(|byte| **byte == b'\n').count() <= limit
        && bytes.len() < MAX_TAIL_BYTES_PER_SOURCE
    {
        let remaining_budget = MAX_TAIL_BYTES_PER_SOURCE - bytes.len();
        let read_len = (offset as usize).min(CHUNK_SIZE).min(remaining_budget);
        offset -= read_len as u64;

        let mut chunk = vec![0; read_len];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&bytes);
        bytes = chunk;
    }

    let Some(end) = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
    else {
        return Ok(Vec::new());
    };
    bytes.truncate(end);

    let start = if offset > 0 {
        match bytes.iter().position(|byte| *byte == b'\n') {
            Some(index) => index + 1,
            None => return Ok(Vec::new()),
        }
    } else {
        0
    };

    let text = String::from_utf8_lossy(&bytes[start..]);
    let mut lines = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.len() > limit {
        lines.drain(..lines.len() - limit);
    }
    Ok(lines)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_source_tail_returns_latest_entries_for_large_source_sets() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let mut sources = Vec::new();

        for i in 0..=SYNC_SOURCE_INGEST_MAX_FILES {
            let path = dir.path().join(format!("source-{i:02}.jsonl"));
            std::fs::write(
                &path,
                format!(
                    "{{\"time\":\"2026-01-01T00:00:{i:02}Z\",\"level\":\"info\",\"msg\":\"line-{i}\"}}\n"
                ),
            )
            .unwrap();
            sources.push(LogSource {
                run_id: "source:test".to_string(),
                service: format!("source-{i:02}"),
                path,
            });
        }

        assert!(!should_ingest_source_synchronously(&sources));

        let response = read_source_tail(&sources, 2, &index, "source:test").unwrap();

        assert_eq!(response.entries.len(), 2);
        assert_eq!(response.entries[0].message, "line-31");
        assert_eq!(response.entries[1].message, "line-32");
        assert!(response.truncated);
    }

    #[test]
    fn tail_reader_ignores_incomplete_final_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tail.jsonl");
        std::fs::write(&path, "one\ntwo\npartial").unwrap();

        let lines = read_last_complete_lines(&path, 5).unwrap();

        assert_eq!(lines, vec!["one".to_string(), "two".to_string()]);
    }
}

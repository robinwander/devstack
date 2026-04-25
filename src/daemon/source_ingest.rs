use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::api::LogViewQuery;
use crate::infra::logs::index::{LogIndex, LogSource};
use crate::logfmt::parse_timestamp_nanos;
use crate::sources::{SourcesLedger, source_run_id};

pub(crate) const SYNC_SOURCE_INGEST_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const SYNC_SOURCE_INGEST_MAX_FILES: usize = 32;

const SOURCE_BACKFILL_BATCH_MAX_BYTES: u64 = 16 * 1024 * 1024;
const SOURCE_BACKFILL_BATCH_MAX_FILES: usize = 8;
const SOURCE_QUERY_WARMUP_MAX_BYTES: u64 = 16 * 1024 * 1024;
const SOURCE_QUERY_WARMUP_MAX_FILES: usize = 8;

#[derive(Clone)]
struct SourceFile {
    source: LogSource,
    modified_nanos: i64,
    len: u64,
}

pub(crate) fn source_log_sources(ledger: &SourcesLedger, name: &str) -> Result<Vec<LogSource>> {
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

pub(crate) fn should_ingest_source_synchronously(sources: &[LogSource]) -> bool {
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

pub(crate) fn retention_cutoff_nanos(max_age: Duration) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    now.saturating_sub(max_age).as_nanos() as i64
}

pub(crate) fn replace_source_index(
    index: &LogIndex,
    name: &str,
    sources: &[LogSource],
    min_ts_nanos: Option<i64>,
) -> Result<()> {
    let run_id = source_run_id(name);
    index.delete_run(&run_id)?;
    if !sources.is_empty() {
        index.ingest_sources_after(sources, min_ts_nanos)?;
    }
    Ok(())
}

pub(crate) fn spawn_source_backfill(
    index: Arc<LogIndex>,
    name: String,
    sources: Vec<LogSource>,
    min_ts_nanos: Option<i64>,
) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let run_id = source_run_id(&name);
        match tokio::task::spawn_blocking(move || {
            if !index.sources_are_current(&sources) {
                ingest_sources_in_priority_batches(&index, &sources, min_ts_nanos)?;
                index.warm_facets(&run_id);
            }
            Ok::<(), anyhow::Error>(())
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                eprintln!("devstack: background source ingest failed for {name}: {err}")
            }
            Err(err) => {
                eprintln!("devstack: background source ingest task failed for {name}: {err}")
            }
        }
    });
}

pub(crate) fn spawn_registered_source_backfill(index: Arc<LogIndex>, min_ts_nanos: Option<i64>) {
    tokio::spawn(async move {
        match tokio::task::spawn_blocking(move || {
            let ledger = SourcesLedger::load()?;
            for source in ledger.list() {
                let sources = source_log_sources(&ledger, &source.name)?;
                let run_id = source_run_id(&source.name);
                ingest_sources_in_priority_batches(&index, &sources, min_ts_nanos)?;
                index.warm_facets(&run_id);
            }
            Ok::<(), anyhow::Error>(())
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => eprintln!("devstack: registered source ingest failed: {err}"),
            Err(err) => eprintln!("devstack: registered source ingest task failed: {err}"),
        }
    });
}

pub(crate) fn ingest_sources_in_priority_batches(
    index: &LogIndex,
    sources: &[LogSource],
    min_ts_nanos: Option<i64>,
) -> Result<()> {
    let batches = priority_source_batches(sources);
    let last_batch_index = batches.len().saturating_sub(1);
    for (batch_index, batch) in batches.into_iter().enumerate() {
        index.ingest_sources_after(&batch, min_ts_nanos)?;
        if batch_index < last_batch_index {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}

pub(crate) fn warmup_sources_for_query(
    sources: &[LogSource],
    query: &LogViewQuery,
) -> Result<Vec<LogSource>> {
    if !query.include_entries && !query.include_facets {
        return Ok(Vec::new());
    }

    let files = source_files_newest_first(sources);
    let since_nanos = query
        .filter
        .since
        .as_deref()
        .and_then(parse_timestamp_nanos);
    let selective = query.include_facets
        || query
            .filter
            .search
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || query
            .filter
            .level
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty() && value != "all")
        || query
            .filter
            .stream
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty() && value != "all");
    let target_lines = query.filter.last.unwrap_or(500).max(1);

    let mut selected = Vec::new();
    let mut selected_bytes = 0u64;
    let mut selected_lines = 0usize;

    for file in files {
        if selected.len() >= SOURCE_QUERY_WARMUP_MAX_FILES
            || selected_bytes >= SOURCE_QUERY_WARMUP_MAX_BYTES
        {
            break;
        }
        if let Some(since) = since_nanos
            && file.modified_nanos > 0
            && file.modified_nanos < since
            && selected.is_empty()
        {
            break;
        }

        selected_bytes = selected_bytes.saturating_add(file.len);
        if !selective && selected_lines < target_lines {
            selected_lines = selected_lines.saturating_add(count_complete_lines_up_to(
                &file.source.path,
                target_lines - selected_lines,
            )?);
        }
        selected.push(file.source);

        if !selective && selected_lines >= target_lines {
            break;
        }
        if since_nanos.is_some() && selected_lines >= target_lines {
            break;
        }
    }

    Ok(selected)
}

fn priority_source_batches(sources: &[LogSource]) -> Vec<Vec<LogSource>> {
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_bytes = 0u64;

    for file in source_files_newest_first(sources) {
        let would_exceed_bytes = current_bytes > 0
            && current_bytes.saturating_add(file.len) > SOURCE_BACKFILL_BATCH_MAX_BYTES;
        let would_exceed_files = current.len() >= SOURCE_BACKFILL_BATCH_MAX_FILES;
        if !current.is_empty() && (would_exceed_bytes || would_exceed_files) {
            batches.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(file.len);
        current.push(file.source);
    }

    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn source_files_newest_first(sources: &[LogSource]) -> Vec<SourceFile> {
    let mut files = sources
        .iter()
        .cloned()
        .filter_map(|source| {
            let metadata = std::fs::metadata(&source.path).ok()?;
            metadata.is_file().then(|| SourceFile {
                modified_nanos: metadata
                    .modified()
                    .ok()
                    .and_then(system_time_nanos)
                    .unwrap_or(0),
                len: metadata.len(),
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
    files
}

fn system_time_nanos(time: SystemTime) -> Option<i64> {
    i64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}

fn count_complete_lines_up_to(path: &Path, limit: usize) -> Result<usize> {
    if limit == 0 {
        return Ok(0);
    }

    let mut file = std::fs::File::open(path)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut lines = 0usize;
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        lines += bytecount_newlines(&buffer[..read]);
        if lines >= limit {
            return Ok(limit);
        }
    }
    Ok(lines)
}

fn bytecount_newlines(bytes: &[u8]) -> usize {
    memchr::memchr_iter(b'\n', bytes).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::LogFilterQuery;

    fn query(last: Option<usize>, since: Option<String>) -> LogViewQuery {
        LogViewQuery {
            filter: LogFilterQuery {
                last,
                since,
                search: None,
                level: None,
                stream: None,
            },
            service: None,
            include_entries: true,
            include_facets: false,
        }
    }

    #[test]
    fn query_warmup_selects_enough_newest_files_for_tail() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.jsonl");
        let new = dir.path().join("new.jsonl");
        std::fs::write(&old, "old-1\nold-2\n").unwrap();
        std::fs::write(&new, "new-1\nnew-2\nnew-3\n").unwrap();

        let sources = vec![
            LogSource {
                run_id: "source:test".to_string(),
                service: "old".to_string(),
                path: old.clone(),
            },
            LogSource {
                run_id: "source:test".to_string(),
                service: "new".to_string(),
                path: new.clone(),
            },
        ];
        let selected = warmup_sources_for_query(&sources, &query(Some(2), None)).unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].path, new);
    }

    #[test]
    fn source_backfill_respects_retention_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let source_path = dir.path().join("source.jsonl");
        std::fs::write(
            &source_path,
            concat!(
                "{\"time\":\"2000-01-01T00:00:00Z\",\"msg\":\"old source\"}\n",
                "{\"time\":\"2200-01-01T00:00:00Z\",\"msg\":\"retained source\"}\n",
            ),
        )
        .unwrap();
        let sources = vec![LogSource {
            run_id: "source:test".to_string(),
            service: "source".to_string(),
            path: source_path,
        }];

        ingest_sources_in_priority_batches(
            &index,
            &sources,
            parse_timestamp_nanos("2100-01-01T00:00:00Z"),
        )
        .unwrap();

        let response = index
            .query_view("source:test", query(Some(10), None))
            .unwrap();
        assert_eq!(response.total, 1);
        assert_eq!(response.entries[0].message, "retained source");
    }

    #[test]
    fn query_warmup_uses_since_to_skip_older_files() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.jsonl");
        std::fs::write(&source_path, "line\n").unwrap();
        let future_since = "2200-01-01T00:00:00Z".to_string();
        let sources = vec![LogSource {
            run_id: "source:test".to_string(),
            service: "source".to_string(),
            path: source_path,
        }];

        let selected =
            warmup_sources_for_query(&sources, &query(Some(10), Some(future_since))).unwrap();

        assert!(selected.is_empty());
    }
}

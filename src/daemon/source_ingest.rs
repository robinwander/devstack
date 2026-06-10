use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::api::LogViewQuery;
use crate::infra::logs::index::{LogIndex, LogSource};
use crate::logfmt::{extract_timestamp_str, parse_timestamp_nanos};
use crate::sources::{
    SourceFileIndexState, SourceIndexState, SourceIndexStatus, SourceRegistry,
    source_retention_duration, source_run_id,
};
use crate::util::now_rfc3339;

pub(crate) const SYNC_SOURCE_INGEST_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const SYNC_SOURCE_INGEST_MAX_FILES: usize = 32;

const SOURCE_BACKFILL_BATCH_MAX_BYTES: u64 = 16 * 1024 * 1024;
const SOURCE_BACKFILL_BATCH_MAX_FILES: usize = 8;
const SOURCE_QUERY_WARMUP_MAX_BYTES: u64 = 16 * 1024 * 1024;
const SOURCE_QUERY_WARMUP_MAX_FILES: usize = 8;
const SOURCE_STARTUP_BACKFILL_DELAY: Duration = Duration::from_secs(2);
const SOURCE_BACKFILL_BETWEEN_SOURCES_DELAY: Duration = Duration::from_millis(250);
const SOURCE_TIMESTAMP_SAMPLE_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
struct SourceFile {
    source: LogSource,
    modified_nanos: i64,
    len: u64,
    first_ts_nanos: Option<i64>,
    last_ts_nanos: Option<i64>,
}

pub(crate) fn source_log_sources(registry: &SourceRegistry, name: &str) -> Result<Vec<LogSource>> {
    let run_id = source_run_id(name);
    Ok(registry
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
    registry: &SourceRegistry,
    name: &str,
    sources: &[LogSource],
    retention: Duration,
    min_ts_nanos: Option<i64>,
) -> Result<()> {
    let run_id = source_run_id(name);
    record_source_state(
        index,
        registry,
        name,
        sources,
        retention,
        min_ts_nanos,
        SourceIndexStatus::Indexing,
        None,
    )?;
    index.delete_run(&run_id)?;
    if !sources.is_empty() {
        ingest_sources_in_priority_batches(index, sources, min_ts_nanos)?;
    }
    record_source_state(
        index,
        registry,
        name,
        sources,
        retention,
        min_ts_nanos,
        SourceIndexStatus::Current,
        None,
    )?;
    Ok(())
}

pub(crate) fn spawn_source_backfill(
    index: Arc<LogIndex>,
    registry: Arc<SourceRegistry>,
    name: String,
    sources: Vec<LogSource>,
    retention: Duration,
    min_ts_nanos: Option<i64>,
) {
    let _ = record_source_state(
        &index,
        &registry,
        &name,
        &sources,
        retention,
        min_ts_nanos,
        SourceIndexStatus::Queued,
        None,
    );

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let error_index = index.clone();
        let error_registry = registry.clone();
        let error_name = name.clone();
        let error_sources = sources.clone();
        let run_id = source_run_id(&name);
        match tokio::task::spawn_blocking(move || {
            record_source_state(
                &index,
                &registry,
                &name,
                &sources,
                retention,
                min_ts_nanos,
                SourceIndexStatus::Indexing,
                None,
            )?;
            if !index.sources_are_current(&sources) {
                ingest_sources_in_priority_batches(&index, &sources, min_ts_nanos)?;
                index.warm_facets(&run_id);
            }
            record_source_state(
                &index,
                &registry,
                &name,
                &sources,
                retention,
                min_ts_nanos,
                SourceIndexStatus::Current,
                None,
            )?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = error_registry.set_index_state(error_source_state(
                    &error_index,
                    &error_name,
                    &error_sources,
                    retention,
                    min_ts_nanos,
                    err.to_string(),
                ));
                eprintln!("devstack: background source ingest failed for {error_name}: {err}")
            }
            Err(err) => {
                let _ = error_registry.set_index_state(error_source_state(
                    &error_index,
                    &error_name,
                    &error_sources,
                    retention,
                    min_ts_nanos,
                    err.to_string(),
                ));
                eprintln!("devstack: background source ingest task failed for {error_name}: {err}")
            }
        }
    });
}

pub(crate) fn spawn_registered_source_backfill(
    index: Arc<LogIndex>,
    registry: Arc<SourceRegistry>,
    default_retention: Duration,
) {
    tokio::spawn(async move {
        tokio::time::sleep(SOURCE_STARTUP_BACKFILL_DELAY).await;
        let registry_for_task = registry.clone();
        match tokio::task::spawn_blocking(move || {
            for source in registry_for_task.list()? {
                let retention = source_retention_duration(&source, default_retention);
                let min_ts_nanos = Some(retention_cutoff_nanos(retention));
                let sources = source_log_sources(&registry_for_task, &source.name)?;
                let run_id = source_run_id(&source.name);
                record_source_state(
                    &index,
                    &registry_for_task,
                    &source.name,
                    &sources,
                    retention,
                    min_ts_nanos,
                    SourceIndexStatus::Queued,
                    None,
                )?;
                if !index.sources_are_current(&sources) {
                    record_source_state(
                        &index,
                        &registry_for_task,
                        &source.name,
                        &sources,
                        retention,
                        min_ts_nanos,
                        SourceIndexStatus::Indexing,
                        None,
                    )?;
                    ingest_sources_in_priority_batches(&index, &sources, min_ts_nanos)?;
                    index.warm_facets(&run_id);
                }
                record_source_state(
                    &index,
                    &registry_for_task,
                    &source.name,
                    &sources,
                    retention,
                    min_ts_nanos,
                    SourceIndexStatus::Current,
                    None,
                )?;
                std::thread::sleep(SOURCE_BACKFILL_BETWEEN_SOURCES_DELAY);
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
    let (batches, skipped) = priority_source_batches(sources, min_ts_nanos);
    if !skipped.is_empty() {
        index.mark_sources_current(&skipped)?;
    }

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
    min_ts_nanos: Option<i64>,
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
        if min_ts_nanos.is_some_and(|cutoff| file.is_before_retention(cutoff)) {
            continue;
        }
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

fn priority_source_batches(
    sources: &[LogSource],
    min_ts_nanos: Option<i64>,
) -> (Vec<Vec<LogSource>>, Vec<LogSource>) {
    let mut batches = Vec::new();
    let mut skipped = Vec::new();
    let mut current = Vec::new();
    let mut current_bytes = 0u64;

    for file in source_files_newest_first(sources) {
        if min_ts_nanos.is_some_and(|cutoff| file.is_before_retention(cutoff)) {
            skipped.push(file.source);
            continue;
        }

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
    (batches, skipped)
}

fn source_files_newest_first(sources: &[LogSource]) -> Vec<SourceFile> {
    let mut files = sources
        .iter()
        .cloned()
        .filter_map(|source| source_file(&source))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        right
            .modified_nanos
            .cmp(&left.modified_nanos)
            .then(left.source.path.cmp(&right.source.path))
    });
    files
}

fn source_file(source: &LogSource) -> Option<SourceFile> {
    let metadata = std::fs::metadata(&source.path).ok()?;
    metadata.is_file().then(|| SourceFile {
        modified_nanos: metadata
            .modified()
            .ok()
            .and_then(system_time_nanos)
            .unwrap_or(0),
        len: metadata.len(),
        first_ts_nanos: first_timestamp_nanos(&source.path).ok().flatten(),
        last_ts_nanos: last_timestamp_nanos(&source.path).ok().flatten(),
        source: source.clone(),
    })
}

impl SourceFile {
    fn is_before_retention(&self, cutoff_nanos: i64) -> bool {
        if self.len == 0 {
            return true;
        }
        match self.last_ts_nanos {
            Some(last_ts_nanos) => last_ts_nanos < cutoff_nanos,
            None => self.modified_nanos > 0 && self.modified_nanos < cutoff_nanos,
        }
    }
}

fn record_source_state(
    index: &LogIndex,
    registry: &SourceRegistry,
    name: &str,
    sources: &[LogSource],
    retention: Duration,
    retention_cutoff_nanos: Option<i64>,
    status: SourceIndexStatus,
    error: Option<String>,
) -> Result<()> {
    registry.set_index_state(source_state(
        index,
        name,
        sources,
        retention,
        retention_cutoff_nanos,
        status,
        error,
    ))
}

fn error_source_state(
    index: &LogIndex,
    name: &str,
    sources: &[LogSource],
    retention: Duration,
    retention_cutoff_nanos: Option<i64>,
    error: String,
) -> SourceIndexState {
    source_state(
        index,
        name,
        sources,
        retention,
        retention_cutoff_nanos,
        SourceIndexStatus::Error,
        Some(error),
    )
}

fn source_state(
    index: &LogIndex,
    name: &str,
    sources: &[LogSource],
    retention: Duration,
    retention_cutoff_nanos: Option<i64>,
    status: SourceIndexStatus,
    error: Option<String>,
) -> SourceIndexState {
    let run_id = source_run_id(name);
    let retained_docs = index.count_run(&run_id).unwrap_or(0);
    let files = sources
        .iter()
        .filter_map(|source| {
            let file = source_file(source)?;
            let cursor = index
                .ingest_cursor(&source.run_id, &source.service)
                .unwrap_or_default();
            let skipped_by_retention =
                retention_cutoff_nanos.is_some_and(|cutoff| file.is_before_retention(cutoff));
            Some(SourceFileIndexState {
                service: source.service.clone(),
                path: source.path.to_string_lossy().to_string(),
                len: file.len,
                modified_nanos: (file.modified_nanos > 0).then_some(file.modified_nanos),
                indexed_offset: cursor.offset,
                next_seq: cursor.next_seq,
                first_ts_nanos: file.first_ts_nanos,
                last_ts_nanos: file.last_ts_nanos,
                skipped_by_retention,
            })
        })
        .collect::<Vec<_>>();
    let retained_through_nanos = files
        .iter()
        .filter(|file| !file.skipped_by_retention)
        .filter_map(|file| file.last_ts_nanos)
        .max();

    SourceIndexState {
        name: name.to_string(),
        run_id,
        status,
        retention_seconds: retention.as_secs(),
        retention_cutoff_nanos,
        retained_docs,
        retained_through_nanos,
        files,
        last_indexed_at: Some(now_rfc3339()),
        error,
    }
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

fn first_timestamp_nanos(path: &Path) -> Result<Option<i64>> {
    let bytes = read_prefix(path, SOURCE_TIMESTAMP_SAMPLE_BYTES)?;
    Ok(timestamp_from_lines(bytes.split(|byte| *byte == b'\n')))
}

fn last_timestamp_nanos(path: &Path) -> Result<Option<i64>> {
    let bytes = read_suffix(path, SOURCE_TIMESTAMP_SAMPLE_BYTES)?;
    Ok(timestamp_from_lines(
        bytes.split(|byte| *byte == b'\n').rev(),
    ))
}

fn timestamp_from_lines<'a, I>(lines: I) -> Option<i64>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(line);
        let Some(timestamp) = extract_timestamp_str(&line) else {
            continue;
        };
        if let Some(nanos) = parse_timestamp_nanos(&timestamp) {
            return Some(nanos);
        }
    }
    None
}

fn read_prefix(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.take(max_bytes).read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn read_suffix(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn bytecount_newlines(bytes: &[u8]) -> usize {
    memchr::memchr_iter(b'\n', bytes).count()
}

#[cfg(test)]
fn empty_cursor() -> crate::infra::logs::index::IngestCursor {
    crate::infra::logs::index::IngestCursor::default()
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
            include_total: true,
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
        let selected = warmup_sources_for_query(&sources, &query(Some(2), None), None).unwrap();

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
    fn source_backfill_marks_fully_expired_files_current_without_indexing() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let source_path = dir.path().join("expired.jsonl");
        std::fs::write(
            &source_path,
            "{\"time\":\"2000-01-01T00:00:00Z\",\"msg\":\"expired\"}\n",
        )
        .unwrap();
        let sources = vec![LogSource {
            run_id: "source:test".to_string(),
            service: "expired".to_string(),
            path: source_path.clone(),
        }];

        ingest_sources_in_priority_batches(
            &index,
            &sources,
            parse_timestamp_nanos("2100-01-01T00:00:00Z"),
        )
        .unwrap();

        assert!(index.sources_are_current(&sources));
        assert_eq!(
            index
                .query_view("source:test", query(Some(10), None))
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            index
                .ingest_cursor("source:test", "expired")
                .unwrap_or_else(empty_cursor)
                .offset,
            std::fs::metadata(source_path).unwrap().len()
        );
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
            warmup_sources_for_query(&sources, &query(Some(10), Some(future_since)), None).unwrap();

        assert!(selected.is_empty());
    }
}

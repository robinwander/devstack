use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::{FAST, Field, FieldType, INDEXED, STORED, STRING, TEXT, Value};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Term};

use crate::api::{
    FacetFilter, FacetValueCount, LogEntry, LogFacetsQuery, LogFacetsResponse, LogSearchQuery,
    LogSearchResponse, LogsQuery, LogsResponse,
};
use crate::logfmt::{
    classify_line_level, extract_log_content, extract_timestamp_str, parse_timestamp_nanos,
};
use crate::paths;
use crate::util::{atomic_write, contains_ansi, strip_ansi};

#[derive(Clone, Debug)]
pub(crate) struct LogSource {
    pub(crate) run_id: String,
    pub(crate) service: String,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct IngestStateFile {
    version: u32,
    sources: HashMap<String, IngestCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct IngestCursor {
    offset: u64,
    next_seq: u64,
}

#[derive(Clone)]
struct LogIndexFields {
    run_id: Field,
    service: Field,
    stream: Field,
    level: Field,
    ts_nanos: Field,
    ts: Field,
    seq: Field,
    message: Field,
    raw: Field,
}

struct LogIndexWriterState {
    writer: Option<IndexWriter>,
    dynamic_fields: HashMap<String, Field>,
}

pub(crate) struct LogIndex {
    index_dir: PathBuf,
    index: RwLock<Index>,
    reader: RwLock<IndexReader>,
    fields: LogIndexFields,
    writer_state: Mutex<LogIndexWriterState>,
    ingest_state_path: PathBuf,
    // Serialize ingestion + cursor persistence to avoid duplicate indexing when multiple
    // clients (UI + CLI) poll concurrently.
    ingest_gate: Mutex<()>,
    ingest: Mutex<IngestStateFile>,
}

impl LogIndex {
    pub(crate) fn open_or_create() -> Result<Self> {
        paths::ensure_base_layout()?;
        let index_dir = paths::logs_index_dir()?;
        let ingest_state_path = paths::logs_ingest_state_path()?;
        Self::open_or_create_at(&index_dir, &ingest_state_path)
    }

    #[cfg(test)]
    pub(crate) fn open_or_create_in(base_dir: &Path) -> Result<Self> {
        let index_dir = base_dir.join("logs_index");
        let ingest_state_path = index_dir.join("ingest_state.json");
        Self::open_or_create_at(&index_dir, &ingest_state_path)
    }

    fn open_or_create_at(index_dir: &Path, ingest_state_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_dir)?;

        let index = match Index::open_in_dir(index_dir) {
            Ok(index) => index,
            Err(_) => {
                // Derived data; if opening fails, rebuild.
                if index_dir.join("meta.json").exists() {
                    let backup = index_dir.with_extension(format!("broken.{}", std::process::id()));
                    let _ = std::fs::rename(index_dir, &backup);
                    std::fs::create_dir_all(index_dir)?;
                }
                let schema = Self::build_schema();
                Index::create_in_dir(index_dir, schema)?
            }
        };

        let schema = index.schema();
        let fields = Self::resolve_fields(&schema)?;

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let writer = index.writer(32 * 1024 * 1024)?;

        let ingest = if ingest_state_path.exists() {
            let bytes = std::fs::read(ingest_state_path).unwrap_or_default();
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            IngestStateFile::default()
        };

        Ok(Self {
            index_dir: index_dir.to_path_buf(),
            index: RwLock::new(index),
            reader: RwLock::new(reader),
            fields,
            writer_state: Mutex::new(LogIndexWriterState {
                writer: Some(writer),
                dynamic_fields: HashMap::new(),
            }),
            ingest_state_path: ingest_state_path.to_path_buf(),
            ingest_gate: Mutex::new(()),
            ingest: Mutex::new(ingest),
        })
    }

    fn build_schema() -> tantivy::schema::Schema {
        let mut schema = tantivy::schema::Schema::builder();
        schema.add_text_field("run_id", STRING | STORED);
        schema.add_text_field("service", STRING | STORED);
        schema.add_text_field("stream", STRING | STORED);
        schema.add_text_field("level", STRING | STORED);
        schema.add_i64_field("ts_nanos", INDEXED | FAST | STORED);
        schema.add_text_field("ts", STRING | STORED);
        schema.add_u64_field("seq", INDEXED | FAST | STORED);
        schema.add_text_field("message", TEXT | STORED);
        schema.add_text_field("raw", STRING | STORED);
        schema.build()
    }

    fn resolve_fields(schema: &tantivy::schema::Schema) -> Result<LogIndexFields> {
        let run_id = schema
            .get_field("run_id")
            .context("tantivy schema missing field run_id")?;
        let service = schema
            .get_field("service")
            .context("tantivy schema missing field service")?;
        let stream = schema
            .get_field("stream")
            .context("tantivy schema missing field stream")?;
        let level = schema
            .get_field("level")
            .context("tantivy schema missing field level")?;
        let ts_nanos = schema
            .get_field("ts_nanos")
            .context("tantivy schema missing field ts_nanos")?;
        let ts = schema
            .get_field("ts")
            .context("tantivy schema missing field ts")?;
        let seq = schema
            .get_field("seq")
            .context("tantivy schema missing field seq")?;
        let message = schema
            .get_field("message")
            .context("tantivy schema missing field message")?;
        let raw = schema
            .get_field("raw")
            .context("tantivy schema missing field raw")?;
        Ok(LogIndexFields {
            run_id,
            service,
            stream,
            level,
            ts_nanos,
            ts,
            seq,
            message,
            raw,
        })
    }

    fn ensure_dynamic_fields(
        &self,
        state: &mut LogIndexWriterState,
        field_names: &BTreeSet<String>,
    ) -> Result<()> {
        let schema = self.index.read().unwrap().schema();
        let mut missing = Vec::new();
        for field_name in field_names {
            if state.dynamic_fields.contains_key(field_name) {
                continue;
            }
            if let Ok(field) = schema.get_field(field_name) {
                state.dynamic_fields.insert(field_name.clone(), field);
                continue;
            }
            missing.push(field_name.clone());
        }

        if missing.is_empty() {
            return Ok(());
        }

        let mut schema_builder = tantivy::schema::Schema::builder();
        for (_, field_entry) in schema.fields() {
            schema_builder.add_field(field_entry.clone());
        }
        for field_name in &missing {
            schema_builder.add_text_field(field_name, STRING | STORED);
        }

        let mut metas = self.index.read().unwrap().load_metas()?;
        metas.schema = schema_builder.build();
        let bytes = serde_json::to_vec_pretty(&metas)?;
        atomic_write(&self.index_dir.join("meta.json"), &bytes)?;

        drop(state.writer.take());

        let index = Index::open_in_dir(&self.index_dir)?;
        let schema = index.schema();
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let writer = index.writer(32 * 1024 * 1024)?;
        Self::resolve_fields(&schema)?;
        let cached_names: Vec<String> = state.dynamic_fields.keys().cloned().collect();

        *self.index.write().unwrap() = index;
        *self.reader.write().unwrap() = reader;
        state.writer = Some(writer);
        state.dynamic_fields.clear();
        for field_name in cached_names.into_iter().chain(missing.into_iter()) {
            if let Ok(field) = schema.get_field(&field_name) {
                state.dynamic_fields.insert(field_name, field);
            }
        }

        Ok(())
    }

    fn extract_dynamic_json_fields(line: &str) -> Vec<(String, String)> {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            return Vec::new();
        }

        let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(trimmed) else {
            return Vec::new();
        };

        let mut fields = HashMap::new();
        for (field_name, value) in map {
            let Some(field_name) = Self::normalize_dynamic_field_name(&field_name) else {
                continue;
            };
            if Self::is_reserved_dynamic_field(&field_name) {
                continue;
            }
            let Some(value) = Self::dynamic_value_to_string(&value) else {
                continue;
            };
            fields.entry(field_name).or_insert(value);
        }

        fields.into_iter().collect()
    }

    fn normalize_dynamic_field_name(field_name: &str) -> Option<String> {
        let mut normalized = String::with_capacity(field_name.len());
        let mut last_was_underscore = false;

        for ch in field_name.chars() {
            if ch.is_ascii_alphanumeric() {
                normalized.push(ch.to_ascii_lowercase());
                last_was_underscore = false;
            } else if !last_was_underscore {
                normalized.push('_');
                last_was_underscore = true;
            }
        }

        let normalized = normalized.trim_matches('_');
        if normalized.is_empty() {
            None
        } else {
            Some(normalized.to_string())
        }
    }

    fn is_reserved_dynamic_field(field_name: &str) -> bool {
        matches!(
            field_name,
            "time"
                | "ts"
                | "timestamp"
                | "msg"
                | "message"
                | "level"
                | "severity"
                | "stream"
                | "run_id"
                | "service"
                | "ts_nanos"
                | "seq"
                | "raw"
        )
    }

    fn dynamic_value_to_string(value: &JsonValue) -> Option<String> {
        let value = match value {
            JsonValue::String(value) => value.clone(),
            JsonValue::Number(value) => value.to_string(),
            JsonValue::Bool(value) => value.to_string(),
            JsonValue::Array(_) | JsonValue::Object(_) | JsonValue::Null => return None,
        };

        if value.is_empty() || value.chars().count() > 256 {
            None
        } else {
            Some(value)
        }
    }

    fn source_key(run_id: &str, service: &str) -> String {
        format!("{run_id}/{service}")
    }

    pub(crate) fn delete_run(&self, run_id: &str) -> Result<()> {
        let _gate = self.ingest_gate.lock().unwrap();
        {
            let mut writer_state = self.writer_state.lock().unwrap();
            let term = Term::from_field_text(self.fields.run_id, run_id);
            let writer = writer_state
                .writer
                .as_mut()
                .context("tantivy writer missing")?;
            writer.delete_term(term);
            writer.commit()?;
        }
        self.reader.read().unwrap().reload().ok();
        {
            let mut ingest = self.ingest.lock().unwrap();
            let prefix = format!("{run_id}/");
            ingest.sources.retain(|k, _| !k.starts_with(&prefix));
            let bytes = serde_json::to_vec_pretty(&*ingest)?;
            atomic_write(&self.ingest_state_path, &bytes)?;
        }
        Ok(())
    }

    pub(crate) fn ingest_sources(&self, sources: &[LogSource]) -> Result<()> {
        let _gate = self.ingest_gate.lock().unwrap();
        if sources.is_empty() {
            return Ok(());
        }

        struct PendingUpdate {
            key: String,
            cursor: IngestCursor,
            run_id: String,
            service: String,
            delete_from_seq: u64,
        }

        struct PendingDoc {
            run_id: String,
            service: String,
            stream: String,
            level: String,
            ts_nanos: i64,
            ts: String,
            seq: u64,
            message: String,
            raw: String,
            dynamic_fields: Vec<(String, String)>,
        }

        // Snapshot cursors first (avoid holding the lock during IO).
        let cursors: HashMap<String, IngestCursor> = {
            let ingest = self.ingest.lock().unwrap();
            sources
                .iter()
                .map(|source| {
                    let key = Self::source_key(&source.run_id, &source.service);
                    let cursor = ingest.sources.get(&key).cloned().unwrap_or_default();
                    (key, cursor)
                })
                .collect()
        };

        // Read + parse new log lines (no tantivy locks held).
        let mut pending_updates = Vec::new();
        let mut pending_docs = Vec::new();
        let mut dynamic_field_names = BTreeSet::new();

        for source in sources {
            if !source.path.exists() {
                continue;
            }
            let key = Self::source_key(&source.run_id, &source.service);
            let mut cursor = cursors.get(&key).cloned().unwrap_or_default();
            let delete_from_seq = cursor.next_seq;

            let file_len = std::fs::metadata(&source.path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if file_len < cursor.offset {
                // Log file truncated or replaced; restart from beginning but keep seq monotonic.
                cursor.offset = 0;
            }

            let mut file =
                File::open(&source.path).with_context(|| format!("open log {:?}", source.path))?;
            file.seek(SeekFrom::Start(cursor.offset))?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            if buf.is_empty() {
                continue;
            }

            // Only ingest complete lines (up to last newline).
            let Some(last_nl) = buf.iter().rposition(|&byte| byte == b'\n') else {
                continue;
            };
            let complete_len = last_nl + 1;
            if complete_len == 0 {
                continue;
            }
            let complete = &buf[..complete_len];
            let text = String::from_utf8_lossy(complete);

            let mut any_lines = false;
            for raw_line in text.lines() {
                let line = if contains_ansi(raw_line) {
                    strip_ansi(raw_line)
                } else {
                    raw_line.to_string()
                };
                if line.is_empty() {
                    continue;
                }
                any_lines = true;

                let ts = extract_timestamp_str(&line).unwrap_or_default();
                let ts_nanos = parse_timestamp_nanos(&ts).unwrap_or(0);
                let (stream, message) = extract_log_content(&line);
                let level = classify_line_level(&line);
                let dynamic_fields = Self::extract_dynamic_json_fields(&line);
                let seq = cursor.next_seq;
                cursor.next_seq = cursor.next_seq.saturating_add(1);

                for (field_name, _) in &dynamic_fields {
                    dynamic_field_names.insert(field_name.clone());
                }

                pending_docs.push(PendingDoc {
                    run_id: source.run_id.clone(),
                    service: source.service.clone(),
                    stream,
                    level,
                    ts_nanos,
                    ts,
                    seq,
                    message,
                    raw: line,
                    dynamic_fields,
                });
            }

            if any_lines {
                cursor.offset = cursor.offset.saturating_add(complete_len as u64);
                pending_updates.push(PendingUpdate {
                    key,
                    cursor,
                    run_id: source.run_id.clone(),
                    service: source.service.clone(),
                    delete_from_seq,
                });
            }
        }

        if pending_docs.is_empty() {
            return Ok(());
        }

        // Write + commit once.
        {
            let mut writer_state = self.writer_state.lock().unwrap();
            self.ensure_dynamic_fields(&mut writer_state, &dynamic_field_names)?;
            let fields = self.fields.clone();

            let docs: Vec<tantivy::TantivyDocument> = pending_docs
                .into_iter()
                .map(|pending| {
                    let mut doc = tantivy::TantivyDocument::default();
                    doc.add_text(fields.run_id, &pending.run_id);
                    doc.add_text(fields.service, &pending.service);
                    doc.add_text(fields.stream, &pending.stream);
                    doc.add_text(fields.level, &pending.level);
                    doc.add_i64(fields.ts_nanos, pending.ts_nanos);
                    doc.add_text(fields.ts, &pending.ts);
                    doc.add_u64(fields.seq, pending.seq);
                    doc.add_text(fields.message, &pending.message);
                    doc.add_text(fields.raw, &pending.raw);
                    for (field_name, value) in pending.dynamic_fields {
                        if let Some(field) = writer_state.dynamic_fields.get(&field_name).copied() {
                            doc.add_text(field, &value);
                        }
                    }
                    doc
                })
                .collect();

            let writer = writer_state
                .writer
                .as_mut()
                .context("tantivy writer missing")?;
            // Crash-safe idempotency: if we previously committed docs but failed to persist
            // `ingest_state`, we may re-ingest overlapping seq ranges. Delete any docs in this
            // run/service with seq >= the starting seq for this ingest, then re-add.
            for update in &pending_updates {
                let run_term = Term::from_field_text(fields.run_id, &update.run_id);
                let svc_term = Term::from_field_text(fields.service, &update.service);
                let q = BooleanQuery::new(vec![
                    (
                        Occur::Must,
                        Box::new(TermQuery::new(
                            run_term,
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                    (
                        Occur::Must,
                        Box::new(TermQuery::new(
                            svc_term,
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                    (
                        Occur::Must,
                        Box::new(RangeQuery::new(
                            Bound::Included(Term::from_field_u64(
                                fields.seq,
                                update.delete_from_seq,
                            )),
                            Bound::Unbounded,
                        )),
                    ),
                ]);
                writer.delete_query(Box::new(q))?;
            }
            for doc in docs {
                writer.add_document(doc)?;
            }
            writer.commit()?;
        }
        self.reader.read().unwrap().reload()?;

        // Persist cursors.
        {
            let mut ingest = self.ingest.lock().unwrap();
            ingest.version = 1;
            for update in pending_updates {
                ingest.sources.insert(update.key, update.cursor);
            }
            let bytes = serde_json::to_vec_pretty(&*ingest)?;
            atomic_write(&self.ingest_state_path, &bytes)?;
        }

        Ok(())
    }

    pub(crate) fn search_service(
        &self,
        run_id: &str,
        service: &str,
        log_path: &Path,
        query: LogsQuery,
    ) -> Result<LogsResponse> {
        self.ingest_sources(&[LogSource {
            run_id: run_id.to_string(),
            service: service.to_string(),
            path: log_path.to_path_buf(),
        }])?;

        let tail = query.last.unwrap_or(500);
        let level_filter = query.level.as_deref().unwrap_or("all");
        let stream_filter = query.stream.as_deref();
        let since_nanos = query.since.as_deref().and_then(parse_timestamp_nanos);
        let after = query.after;
        let fields = self.fields.clone();

        // Scope query: run + service (+ since/+ stream). Counts ignore search/level/after.
        let scope_query = Self::build_scope_query(
            &fields,
            run_id,
            Some(service),
            since_nanos,
            stream_filter,
            None,
            None,
        )?;

        // Result query: scope + (after) + (level) + (search)
        let mut result_query = Self::build_scope_query(
            &fields,
            run_id,
            Some(service),
            since_nanos,
            stream_filter,
            after,
            None,
        )?;
        result_query = Self::add_level_filter(fields.level, result_query, level_filter)?;
        {
            let index = self.index.read().unwrap();
            result_query = Self::add_text_query(
                &index,
                fields.message,
                result_query,
                query.search.as_deref(),
            )?;
        }

        let searcher = self.reader.read().unwrap().searcher();
        let total: usize = searcher.search(&scope_query, &Count)?;

        let error_count: usize = {
            let term = Term::from_field_text(fields.level, "error");
            let q = BooleanQuery::new(vec![
                (Occur::Must, scope_query.box_clone()),
                (
                    Occur::Must,
                    Box::new(TermQuery::new(
                        term,
                        tantivy::schema::IndexRecordOption::Basic,
                    )),
                ),
            ]);
            searcher.search(&q, &Count)?
        };
        let warn_count: usize = {
            let term = Term::from_field_text(fields.level, "warn");
            let q = BooleanQuery::new(vec![
                (Occur::Must, scope_query.box_clone()),
                (
                    Occur::Must,
                    Box::new(TermQuery::new(
                        term,
                        tantivy::schema::IndexRecordOption::Basic,
                    )),
                ),
            ]);
            searcher.search(&q, &Count)?
        };

        let matched_total: usize = searcher.search(&result_query, &Count)?;

        let mut lines: Vec<(i64, u64, String)> = Vec::new();
        let mut next_after: Option<u64> = None;

        if tail > 0 {
            if after.is_some() {
                // Follow mode: order by seq ascending.
                let top_docs: Vec<(u64, DocAddress)> = searcher.search(
                    &result_query,
                    &TopDocs::with_limit(tail).order_by_fast_field("seq", tantivy::Order::Asc),
                )?;
                for (_sort, addr) in top_docs {
                    let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
                    let raw = doc
                        .get_first(fields.raw)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let ts = doc
                        .get_first(fields.ts_nanos)
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let seq = doc
                        .get_first(fields.seq)
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    next_after = Some(next_after.map(|value| value.max(seq)).unwrap_or(seq));
                    lines.push((ts, seq, raw));
                }
                // Already sorted by seq asc.
            } else {
                // Tail mode: order by timestamp descending, then reverse to chrono.
                let top_docs: Vec<(i64, DocAddress)> = searcher.search(
                    &result_query,
                    &TopDocs::with_limit(tail)
                        .order_by_fast_field("ts_nanos", tantivy::Order::Desc),
                )?;
                for (_sort, addr) in top_docs {
                    let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
                    let raw = doc
                        .get_first(fields.raw)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let ts = doc
                        .get_first(fields.ts_nanos)
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let seq = doc
                        .get_first(fields.seq)
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    next_after = Some(next_after.map(|value| value.max(seq)).unwrap_or(seq));
                    lines.push((ts, seq, raw));
                }
                lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            }
        }

        let out_lines: Vec<String> = lines.into_iter().map(|(_, _, line)| line).collect();
        Ok(LogsResponse {
            lines: out_lines,
            truncated: matched_total > tail && tail > 0,
            total,
            error_count,
            warn_count,
            next_after,
            matched_total,
        })
    }

    pub(crate) fn search_run(
        &self,
        run_id: &str,
        _sources: &[LogSource],
        query: LogSearchQuery,
    ) -> Result<LogSearchResponse> {
        let tail = query.last.unwrap_or(500);
        let level_filter = query.level.as_deref().unwrap_or("all");
        let stream_filter = query.stream.as_deref();
        let service_filter = query.service.as_deref();
        let since_nanos = query.since.as_deref().and_then(parse_timestamp_nanos);
        let fields = self.fields.clone();

        let scope_query = Self::build_scope_query(
            &fields,
            run_id,
            service_filter,
            since_nanos,
            stream_filter,
            None,
            None,
        )?;

        let mut result_query = Self::build_scope_query(
            &fields,
            run_id,
            service_filter,
            since_nanos,
            stream_filter,
            None,
            None,
        )?;
        result_query = Self::add_level_filter(fields.level, result_query, level_filter)?;

        let attribute_fields = {
            let index = self.index.read().unwrap();
            result_query = Self::add_text_query(
                &index,
                fields.message,
                result_query,
                query.search.as_deref(),
            )?;
            Self::dynamic_attribute_fields(&index.schema())
        };

        let searcher = self.reader.read().unwrap().searcher();
        let total: usize = searcher.search(&scope_query, &Count)?;
        let error_count: usize = {
            let term = Term::from_field_text(fields.level, "error");
            let q = BooleanQuery::new(vec![
                (Occur::Must, scope_query.box_clone()),
                (
                    Occur::Must,
                    Box::new(TermQuery::new(
                        term,
                        tantivy::schema::IndexRecordOption::Basic,
                    )),
                ),
            ]);
            searcher.search(&q, &Count)?
        };
        let warn_count: usize = {
            let term = Term::from_field_text(fields.level, "warn");
            let q = BooleanQuery::new(vec![
                (Occur::Must, scope_query.box_clone()),
                (
                    Occur::Must,
                    Box::new(TermQuery::new(
                        term,
                        tantivy::schema::IndexRecordOption::Basic,
                    )),
                ),
            ]);
            searcher.search(&q, &Count)?
        };

        let matched_total: usize = searcher.search(&result_query, &Count)?;

        let mut entries: Vec<(i64, u64, LogEntry)> = Vec::new();
        if tail > 0 {
            let top_docs: Vec<(i64, DocAddress)> = searcher.search(
                &result_query,
                &TopDocs::with_limit(tail).order_by_fast_field("ts_nanos", tantivy::Order::Desc),
            )?;
            for (_sort, addr) in top_docs {
                let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
                let ts = doc
                    .get_first(fields.ts)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let service = doc
                    .get_first(fields.service)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let stream = doc
                    .get_first(fields.stream)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let level = doc
                    .get_first(fields.level)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let message = doc
                    .get_first(fields.message)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let raw = doc
                    .get_first(fields.raw)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let ts_nanos = doc
                    .get_first(fields.ts_nanos)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let seq = doc
                    .get_first(fields.seq)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                let mut attributes = BTreeMap::new();
                for (field_name, field_handle) in &attribute_fields {
                    if let Some(value) = doc.get_first(*field_handle).and_then(|v| v.as_str())
                        && !value.is_empty()
                    {
                        attributes.insert(field_name.clone(), value.to_string());
                    }
                }

                entries.push((
                    ts_nanos,
                    seq,
                    LogEntry {
                        ts,
                        service,
                        stream,
                        level,
                        message,
                        raw,
                        attributes,
                    },
                ));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        }

        Ok(LogSearchResponse {
            entries: entries.into_iter().map(|(_, _, entry)| entry).collect(),
            truncated: matched_total > tail && tail > 0,
            total,
            error_count,
            warn_count,
            matched_total,
        })
    }

    pub(crate) fn facets_run(
        &self,
        run_id: &str,
        _sources: &[LogSource],
        query: LogFacetsQuery,
    ) -> Result<LogFacetsResponse> {
        let since_nanos = query.since.as_deref().and_then(parse_timestamp_nanos);
        let service_filter = query.service.as_deref();
        let level_filter = query.level.as_deref();
        let stream_filter = query.stream.as_deref();
        let fields = self.fields.clone();
        let facet_fields = {
            let index = self.index.read().unwrap();
            Self::facet_fields(&index.schema())
        };
        let searcher = self.reader.read().unwrap().searcher();

        let total_query =
            Self::build_scope_query(&fields, run_id, None, since_nanos, None, None, None)?;
        let total: usize = searcher.search(&total_query, &Count)?;

        let mut filters = Vec::new();
        for (field_name, field_handle) in facet_fields {
            let include_service = if field_name == "service" {
                None
            } else {
                service_filter
            };
            let include_stream = if field_name == "stream" {
                None
            } else {
                stream_filter
            };

            let mut scope = Self::build_scope_query(
                &fields,
                run_id,
                include_service,
                since_nanos,
                include_stream,
                None,
                None,
            )?;
            if field_name != "level"
                && let Some(level) = level_filter
            {
                scope = Self::add_level_filter(fields.level, scope, level)?;
            }

            let mut values = Vec::new();
            for value in Self::collect_field_terms(&searcher, field_handle)? {
                let term = Term::from_field_text(field_handle, &value);
                let scoped = scope.box_clone();
                let count_query = BooleanQuery::new(vec![
                    (Occur::Must, scoped),
                    (
                        Occur::Must,
                        Box::new(TermQuery::new(
                            term,
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                ]);
                let count: usize = searcher.search(&count_query, &Count)?;
                if count > 0 {
                    values.push(FacetValueCount { value, count });
                }
            }
            values.sort_by(|a, b| b.count.cmp(&a.count).then(a.value.cmp(&b.value)));

            if !values.is_empty() {
                filters.push(FacetFilter {
                    field: field_name.clone(),
                    kind: Self::facet_kind_for(&field_name).to_string(),
                    values,
                });
            }
        }

        filters.sort_by(|a, b| {
            Self::facet_sort_rank(&a.field)
                .cmp(&Self::facet_sort_rank(&b.field))
                .then(a.field.cmp(&b.field))
        });

        Ok(LogFacetsResponse { total, filters })
    }

    fn facet_fields(schema: &tantivy::schema::Schema) -> Vec<(String, Field)> {
        schema
            .fields()
            .filter_map(|(field, entry)| {
                if !entry.is_indexed() || !entry.is_stored() {
                    return None;
                }
                if !matches!(entry.field_type(), FieldType::Str(_)) {
                    return None;
                }
                let name = entry.name();
                if matches!(name, "run_id" | "ts" | "raw" | "message") {
                    return None;
                }
                Some((name.to_string(), field))
            })
            .collect()
    }

    fn dynamic_attribute_fields(schema: &tantivy::schema::Schema) -> Vec<(String, Field)> {
        schema
            .fields()
            .filter_map(|(field, entry)| {
                if !entry.is_stored() {
                    return None;
                }
                if !matches!(entry.field_type(), FieldType::Str(_)) {
                    return None;
                }
                let name = entry.name();
                if matches!(
                    name,
                    "run_id" | "service" | "stream" | "level" | "ts" | "message" | "raw"
                ) {
                    return None;
                }
                Some((name.to_string(), field))
            })
            .collect()
    }

    fn collect_field_terms(searcher: &tantivy::Searcher, field: Field) -> Result<Vec<String>> {
        let mut terms = BTreeSet::new();
        for segment in searcher.segment_readers() {
            let inverted_index = segment.inverted_index(field)?;
            let mut stream = inverted_index.terms().stream()?;
            while stream.advance() {
                let Ok(value) = std::str::from_utf8(stream.key()) else {
                    continue;
                };
                if value.is_empty() {
                    continue;
                }
                terms.insert(value.to_string());
            }
        }
        Ok(terms.into_iter().collect())
    }

    fn facet_kind_for(field: &str) -> &'static str {
        if matches!(field, "level" | "stream") {
            "toggle"
        } else {
            "select"
        }
    }

    fn facet_sort_rank(field: &str) -> usize {
        match field {
            "service" => 0,
            "level" => 1,
            "stream" => 2,
            _ => 3,
        }
    }

    fn build_scope_query(
        fields: &LogIndexFields,
        run_id: &str,
        service: Option<&str>,
        since_nanos: Option<i64>,
        stream: Option<&str>,
        after_seq: Option<u64>,
        extra: Option<Box<dyn Query>>,
    ) -> Result<Box<dyn Query>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        let run_term = Term::from_field_text(fields.run_id, run_id);
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                run_term,
                tantivy::schema::IndexRecordOption::Basic,
            )),
        ));

        if let Some(service) = service {
            let term = Term::from_field_text(fields.service, service);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    term,
                    tantivy::schema::IndexRecordOption::Basic,
                )),
            ));
        }

        if let Some(stream) = stream
            && !stream.is_empty()
            && stream != "all"
        {
            let term = Term::from_field_text(fields.stream, stream);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    term,
                    tantivy::schema::IndexRecordOption::Basic,
                )),
            ));
        }

        if let Some(since) = since_nanos {
            clauses.push((
                Occur::Must,
                Box::new(RangeQuery::new(
                    Bound::Included(Term::from_field_i64(fields.ts_nanos, since)),
                    Bound::Unbounded,
                )),
            ));
        }

        if let Some(after) = after_seq {
            clauses.push((
                Occur::Must,
                Box::new(RangeQuery::new(
                    Bound::Excluded(Term::from_field_u64(fields.seq, after)),
                    Bound::Unbounded,
                )),
            ));
        }

        if let Some(extra) = extra {
            clauses.push((Occur::Must, extra));
        }

        if clauses.is_empty() {
            return Ok(Box::new(AllQuery));
        }
        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    fn add_level_filter(
        level_field: Field,
        base: Box<dyn Query>,
        level: &str,
    ) -> Result<Box<dyn Query>> {
        let level = level.trim();
        if level.is_empty() || level == "all" {
            return Ok(base);
        }
        let mut clauses = vec![(Occur::Must, base)];
        match level {
            "error" => {
                let term = Term::from_field_text(level_field, "error");
                clauses.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        term,
                        tantivy::schema::IndexRecordOption::Basic,
                    )),
                ));
            }
            "warn" => {
                let warn = Term::from_field_text(level_field, "warn");
                clauses.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        warn,
                        tantivy::schema::IndexRecordOption::Basic,
                    )),
                ));
            }
            _ => {}
        }
        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    fn add_text_query(
        index: &Index,
        message_field: Field,
        base: Box<dyn Query>,
        q: Option<&str>,
    ) -> Result<Box<dyn Query>> {
        let Some(q) = q else {
            return Ok(base);
        };
        let q = q.trim();
        if q.is_empty() {
            return Ok(base);
        }

        let qp = QueryParser::for_index(index, vec![message_field]);
        let parsed = match qp.parse_query(q) {
            Ok(query) => query,
            Err(err) => return Err(anyhow::anyhow!("bad_query: {err}")),
        };
        Ok(Box::new(BooleanQuery::new(vec![
            (Occur::Must, base),
            (Occur::Must, parsed),
        ])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn logs_query(last: usize, after: Option<u64>, search: Option<&str>) -> LogsQuery {
        LogsQuery {
            last: Some(last),
            since: None,
            search: search.map(|s| s.to_string()),
            level: None,
            stream: None,
            after,
        }
    }

    fn facet_values(response: &LogFacetsResponse, field: &str) -> Vec<(String, usize)> {
        response
            .filters
            .iter()
            .find(|filter| filter.field == field)
            .unwrap()
            .values
            .iter()
            .map(|value| (value.value.clone(), value.count))
            .collect()
    }

    fn ingest(index: &LogIndex, sources: &[LogSource]) {
        index.ingest_sources(sources).unwrap();
    }

    #[test]
    fn service_search_ingests_incrementally_and_supports_after() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            "[2025-01-01T00:00:00Z] [stdout] hello world\n[2025-01-01T00:00:01Z] [stderr] Warning: oh no\n",
        )
        .unwrap();

        let resp1 = index
            .search_service("run-1", "api", &log_path, logs_query(10, None, None))
            .unwrap();
        assert_eq!(resp1.lines.len(), 2);
        let after = resp1.next_after.unwrap();

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        writeln!(f, "[2025-01-01T00:00:02Z] [stdout] later message").unwrap();

        let resp2 = index
            .search_service("run-1", "api", &log_path, logs_query(10, Some(after), None))
            .unwrap();
        assert_eq!(resp2.lines.len(), 1);
        assert!(resp2.lines[0].contains("later message"));
    }

    #[test]
    fn run_search_combines_services() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let api_log = dir.path().join("api.log");
        let web_log = dir.path().join("web.log");

        std::fs::write(
            &api_log,
            "[2025-01-01T00:00:00Z] [stdout] api started\n[2025-01-01T00:00:02Z] [stderr] Error: api failed\n",
        )
        .unwrap();
        std::fs::write(&web_log, "[2025-01-01T00:00:01Z] [stdout] web started\n").unwrap();

        let sources = vec![
            LogSource {
                run_id: "run-1".to_string(),
                service: "api".to_string(),
                path: api_log,
            },
            LogSource {
                run_id: "run-1".to_string(),
                service: "web".to_string(),
                path: web_log,
            },
        ];
        ingest(&index, &sources);

        let resp = index
            .search_run(
                "run-1",
                &sources,
                LogSearchQuery {
                    last: Some(10),
                    since: None,
                    search: Some("error".to_string()),
                    level: None,
                    stream: None,
                    service: None,
                },
            )
            .unwrap();

        assert_eq!(resp.entries.len(), 1);
        assert_eq!(resp.entries[0].service, "api");
        assert!(resp.entries[0].raw.contains("Error"));
    }

    #[test]
    fn run_queries_do_not_auto_ingest_sources() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let log_path = dir.path().join("api.log");
        std::fs::write(&log_path, "[2025-01-01T00:00:00Z] [stdout] hello\n").unwrap();

        let sources = vec![LogSource {
            run_id: "run-no-auto-ingest".to_string(),
            service: "api".to_string(),
            path: log_path,
        }];

        let search = index
            .search_run(
                "run-no-auto-ingest",
                &sources,
                LogSearchQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                    service: None,
                },
            )
            .unwrap();
        assert_eq!(search.total, 0);
        assert!(search.entries.is_empty());

        let facets = index
            .facets_run(
                "run-no-auto-ingest",
                &sources,
                LogFacetsQuery {
                    since: None,
                    service: None,
                    level: None,
                    stream: None,
                },
            )
            .unwrap();
        assert_eq!(facets.total, 0);
        assert!(facets.filters.is_empty());
    }

    #[test]
    fn ingest_json_lines_returns_structured_fields() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","level":"info","msg":"started"}
{"time":"2025-01-01T00:00:01Z","stream":"stderr","level":"error","msg":"failed"}
"#,
        )
        .unwrap();

        let sources = vec![LogSource {
            run_id: "run-json".to_string(),
            service: "api".to_string(),
            path: log_path,
        }];
        ingest(&index, &sources);

        let resp = index
            .search_run(
                "run-json",
                &sources,
                LogSearchQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                    service: None,
                },
            )
            .unwrap();

        assert_eq!(resp.entries.len(), 2);
        assert_eq!(resp.entries[0].message, "started");
        assert_eq!(resp.entries[0].level, "info");
        assert_eq!(resp.entries[1].message, "failed");
        assert_eq!(resp.entries[1].level, "error");
    }

    #[test]
    fn ingest_mixed_json_and_bracket_lines() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","msg":"json line"}
[2025-01-01T00:00:01Z] [stderr] Warning: bracket line
"#,
        )
        .unwrap();

        let resp = index
            .search_service("run-mixed", "api", &log_path, logs_query(10, None, None))
            .unwrap();

        assert_eq!(resp.total, 2);
        assert!(resp.lines.iter().any(|line| line.contains("json line")));
        assert!(resp.lines.iter().any(|line| line.contains("bracket line")));
    }

    #[test]
    fn json_level_is_used_instead_of_keyword_heuristics() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","level":"info","msg":"Error text but info level"}
"#,
        )
        .unwrap();

        let sources = vec![LogSource {
            run_id: "run-level".to_string(),
            service: "api".to_string(),
            path: log_path,
        }];
        ingest(&index, &sources);

        let errors = index
            .search_run(
                "run-level",
                &sources,
                LogSearchQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: Some("error".to_string()),
                    stream: None,
                    service: None,
                },
            )
            .unwrap();

        assert_eq!(errors.entries.len(), 0);
        assert_eq!(errors.error_count, 0);
    }

    #[test]
    fn json_timestamp_controls_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:02Z","stream":"stdout","msg":"later"}
{"time":"2025-01-01T00:00:01Z","stream":"stdout","msg":"earlier"}
"#,
        )
        .unwrap();

        let sources = vec![LogSource {
            run_id: "run-order".to_string(),
            service: "api".to_string(),
            path: log_path,
        }];
        ingest(&index, &sources);

        let resp = index
            .search_run(
                "run-order",
                &sources,
                LogSearchQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                    service: None,
                },
            )
            .unwrap();

        assert_eq!(resp.entries.len(), 2);
        assert_eq!(resp.entries[0].message, "earlier");
        assert_eq!(resp.entries[1].message, "later");
    }

    #[test]
    fn facets_include_filter_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let api_log = dir.path().join("api.log");
        let worker_log = dir.path().join("worker.log");

        std::fs::write(
            &api_log,
            "[2025-01-01T00:00:00Z] [stdout] hello\n[2025-01-01T00:00:01Z] [stderr] Error: failed\n",
        )
        .unwrap();
        std::fs::write(
            &worker_log,
            "[2025-01-01T00:00:00Z] [stdout] worker ready\n",
        )
        .unwrap();

        let sources = vec![
            LogSource {
                run_id: "run-facets".to_string(),
                service: "api".to_string(),
                path: api_log,
            },
            LogSource {
                run_id: "run-facets".to_string(),
                service: "worker".to_string(),
                path: worker_log,
            },
        ];
        ingest(&index, &sources);

        let response = index
            .facets_run(
                "run-facets",
                &sources,
                LogFacetsQuery {
                    since: None,
                    service: None,
                    level: None,
                    stream: None,
                },
            )
            .unwrap();

        assert!(
            response
                .filters
                .iter()
                .any(|filter| filter.field == "service")
        );
        assert!(
            response
                .filters
                .iter()
                .any(|filter| filter.field == "level")
        );
        assert!(
            response
                .filters
                .iter()
                .any(|filter| filter.field == "stream")
        );

        let level_filter = response
            .filters
            .iter()
            .find(|filter| filter.field == "level")
            .unwrap();
        assert_eq!(level_filter.kind, "toggle");
        assert!(
            level_filter
                .values
                .iter()
                .any(|value| value.value == "error")
        );
    }

    #[test]
    fn facets_include_dynamic_json_fields() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let api_log = dir.path().join("api.log");
        let worker_log = dir.path().join("worker.log");
        let long_value = "x".repeat(257);

        let api_contents = format!(
            concat!(
                "{{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"level\":\"info\",\"msg\":\"GET /users\",\"method\":\"GET\",\"path\":\"/users\",\"status\":200,\"details\":{{\"skip\":true}},\"trace\":\"{}\"}}\n",
                "{{\"time\":\"2025-01-01T00:00:01Z\",\"stream\":\"stdout\",\"level\":\"info\",\"msg\":\"GET /users\",\"method\":\"GET\",\"path\":\"/users\",\"status\":200}}\n"
            ),
            long_value,
        );
        std::fs::write(&api_log, api_contents).unwrap();
        std::fs::write(
            &worker_log,
            "{\"time\":\"2025-01-01T00:00:02Z\",\"stream\":\"stderr\",\"level\":\"error\",\"msg\":\"POST /jobs failed\",\"method\":\"POST\",\"path\":\"/jobs\",\"status\":500}\n",
        )
        .unwrap();

        let sources = vec![
            LogSource {
                run_id: "run-dynamic-facets".to_string(),
                service: "api".to_string(),
                path: api_log,
            },
            LogSource {
                run_id: "run-dynamic-facets".to_string(),
                service: "worker".to_string(),
                path: worker_log,
            },
        ];
        ingest(&index, &sources);

        let response = index
            .facets_run(
                "run-dynamic-facets",
                &sources,
                LogFacetsQuery {
                    since: None,
                    service: None,
                    level: None,
                    stream: None,
                },
            )
            .unwrap();

        assert_eq!(
            facet_values(&response, "method"),
            vec![("GET".to_string(), 2), ("POST".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "path"),
            vec![("/users".to_string(), 2), ("/jobs".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "status"),
            vec![("200".to_string(), 2), ("500".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "service"),
            vec![("api".to_string(), 2), ("worker".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "level"),
            vec![("info".to_string(), 2), ("error".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "stream"),
            vec![("stdout".to_string(), 2), ("stderr".to_string(), 1)]
        );
        assert!(response.filters.iter().all(|filter| filter.field != "time"));
        assert!(
            response
                .filters
                .iter()
                .all(|filter| filter.field != "details")
        );
        assert!(
            response
                .filters
                .iter()
                .all(|filter| filter.field != "trace")
        );
    }

    #[test]
    fn ingest_is_idempotent_if_cursor_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let log_path = dir.path().join("api.log");
        std::fs::write(
            &log_path,
            "[2025-01-01T00:00:00Z] [stdout] hello world\n[2025-01-01T00:00:01Z] [stderr] Warning: oh no\n",
        )
        .unwrap();

        let resp1 = index
            .search_service("run-1", "api", &log_path, logs_query(50, None, None))
            .unwrap();
        assert_eq!(resp1.total, 2);

        // Simulate a crash between commit and persisting ingest_state: cursor rewinds.
        {
            let mut ingest = index.ingest.lock().unwrap();
            ingest.sources.insert(
                LogIndex::source_key("run-1", "api"),
                IngestCursor {
                    offset: 0,
                    next_seq: 0,
                },
            );
        }

        let resp2 = index
            .search_service("run-1", "api", &log_path, logs_query(50, None, None))
            .unwrap();
        assert_eq!(resp2.total, 2);
        assert_eq!(resp2.lines.len(), 2);
    }
}

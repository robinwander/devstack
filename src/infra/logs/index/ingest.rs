use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Bound;

use anyhow::{Context, Result};
use tantivy::Term;
use tantivy::query::{BooleanQuery, Occur, RangeQuery, TermQuery};
use tantivy::schema::OwnedValue;

use crate::logfmt::{contains_ansi, parse_log_line, parse_timestamp_nanos, strip_ansi};
use crate::util::atomic_write;

use super::{IngestCursor, LogIndex, LogSource};

const INGEST_READ_CHUNK_BYTES: usize = 512 * 1024;
const INGEST_COMMIT_DOC_LIMIT: usize = 10_000;
const INGEST_COMMIT_BYTE_LIMIT: usize = 8 * 1024 * 1024;

impl LogIndex {
    pub(crate) fn sources_are_current(&self, sources: &[LogSource]) -> bool {
        let ingest = self.ingest.lock().unwrap();
        sources.iter().all(|source| {
            let Ok(metadata) = std::fs::metadata(&source.path) else {
                return true;
            };
            if !metadata.is_file() {
                return true;
            }
            let key = Self::source_key(&source.run_id, &source.service);
            ingest
                .sources
                .get(&key)
                .is_some_and(|cursor| cursor.offset == metadata.len())
        })
    }

    pub(crate) fn ingest_cursor(&self, run_id: &str, service: &str) -> Option<IngestCursor> {
        let key = Self::source_key(run_id, service);
        self.ingest.lock().unwrap().sources.get(&key).cloned()
    }

    pub(crate) fn mark_sources_current(&self, sources: &[LogSource]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }

        let mut ingest = self.ingest.lock().unwrap();
        ingest.version = 1;
        for source in sources {
            let Ok(metadata) = std::fs::metadata(&source.path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let key = Self::source_key(&source.run_id, &source.service);
            let cursor = ingest.sources.entry(key).or_default();
            if metadata.len() < cursor.offset {
                cursor.next_seq = 0;
            }
            cursor.offset = metadata.len();
        }
        let bytes = serde_json::to_vec_pretty(&*ingest)?;
        atomic_write(&self.ingest_state_path, &bytes)?;
        Ok(())
    }

    pub(crate) fn ingest_sources(&self, sources: &[LogSource]) -> Result<()> {
        self.ingest_sources_after(sources, None)
    }

    pub(crate) fn ingest_sources_after(
        &self,
        sources: &[LogSource],
        min_ts_nanos: Option<i64>,
    ) -> Result<()> {
        let _gate = self.ingest_gate.lock().unwrap();
        if sources.is_empty() {
            return Ok(());
        }

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

        let mut batch = IngestBatch::default();

        for source in sources {
            let Ok(metadata) = std::fs::metadata(&source.path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }

            let key = Self::source_key(&source.run_id, &source.service);
            let mut cursor = cursors.get(&key).cloned().unwrap_or_default();
            let file_len = metadata.len();
            if file_len < cursor.offset {
                cursor.offset = 0;
                cursor.next_seq = 0;
            } else if file_len == cursor.offset {
                continue;
            }

            let mut delete_from_seq = Some(cursor.next_seq);
            let mut file =
                File::open(&source.path).with_context(|| format!("open log {:?}", source.path))?;
            file.seek(SeekFrom::Start(cursor.offset))?;

            let mut carry = Vec::new();
            let mut read_buffer = vec![0u8; INGEST_READ_CHUNK_BYTES];
            loop {
                let read = file.read(&mut read_buffer)?;
                if read == 0 {
                    break;
                }
                carry.extend_from_slice(&read_buffer[..read]);

                let Some(last_nl) = carry.iter().rposition(|&byte| byte == b'\n') else {
                    continue;
                };
                let complete_len = last_nl + 1;
                let complete = carry[..complete_len].to_vec();
                carry.drain(..complete_len);

                let previous_offset = cursor.offset;
                self.collect_complete_lines(
                    source,
                    &key,
                    &mut cursor,
                    delete_from_seq.take(),
                    &complete,
                    min_ts_nanos,
                    &mut batch,
                )?;

                if cursor.offset == previous_offset {
                    cursor.offset = cursor.offset.saturating_add(complete_len as u64);
                    batch.record_update(PendingUpdate {
                        key: key.clone(),
                        cursor: cursor.clone(),
                        run_id: source.run_id.clone(),
                        service: source.service.clone(),
                        delete_from_seq: None,
                    });
                }

                if batch.should_flush() {
                    self.commit_ingest_batch(&mut batch)?;
                }
            }
        }

        self.commit_ingest_batch(&mut batch)?;
        Ok(())
    }

    fn collect_complete_lines(
        &self,
        source: &LogSource,
        key: &str,
        cursor: &mut IngestCursor,
        delete_from_seq: Option<u64>,
        complete: &[u8],
        min_ts_nanos: Option<i64>,
        batch: &mut IngestBatch,
    ) -> Result<()> {
        let text = String::from_utf8_lossy(complete);
        let starting_offset = cursor.offset;
        let mut consumed_bytes = 0usize;
        let mut saw_complete_line = false;

        for raw_line in text.split_inclusive('\n') {
            let line_bytes = raw_line.as_bytes().len();
            consumed_bytes = consumed_bytes.saturating_add(line_bytes);
            let raw_line = raw_line.trim_end_matches(['\r', '\n']);
            if raw_line.is_empty() {
                saw_complete_line = true;
                continue;
            }
            saw_complete_line = true;

            let line = if contains_ansi(raw_line) {
                strip_ansi(raw_line)
            } else {
                raw_line.to_string()
            };

            let parsed = parse_log_line(&line);
            let ts = parsed.timestamp.unwrap_or_default();
            let ts_nanos = parse_timestamp_nanos(&ts).unwrap_or(0);
            let seq = cursor.next_seq;
            cursor.next_seq = cursor.next_seq.saturating_add(1);

            if min_ts_nanos.is_some_and(|min_ts_nanos| ts_nanos < min_ts_nanos) {
                continue;
            }

            let dynamic_fields = parsed
                .json
                .as_ref()
                .map(Self::extract_dynamic_json_fields_from_map)
                .unwrap_or_default();

            batch.add_doc(PendingDoc {
                run_id: source.run_id.clone(),
                service: source.service.clone(),
                stream: parsed.stream,
                level: parsed.level,
                ts_nanos,
                ts,
                seq,
                message: parsed.message,
                raw: line,
                dynamic_fields,
            });
        }

        if saw_complete_line {
            cursor.offset = starting_offset.saturating_add(consumed_bytes as u64);
            batch.record_update(PendingUpdate {
                key: key.to_string(),
                cursor: cursor.clone(),
                run_id: source.run_id.clone(),
                service: source.service.clone(),
                delete_from_seq,
            });
        }

        Ok(())
    }

    fn commit_ingest_batch(&self, batch: &mut IngestBatch) -> Result<()> {
        if batch.pending_updates.is_empty() {
            return Ok(());
        }

        let pending_updates = std::mem::take(&mut batch.pending_updates);
        let pending_docs = std::mem::take(&mut batch.pending_docs);
        let dynamic_field_names_by_source =
            std::mem::take(&mut batch.dynamic_field_names_by_source);
        batch.pending_doc_bytes = 0;

        {
            let mut writer_state = self.writer_state.lock().unwrap();
            let fields = self.fields.clone();
            let writer = writer_state
                .writer
                .as_mut()
                .context("tantivy writer missing")?;
            for update in &pending_updates {
                let Some(delete_from_seq) = update.delete_from_seq else {
                    continue;
                };
                let run_term = Term::from_field_text(fields.run_id, &update.run_id);
                let service_term = Term::from_field_text(fields.service, &update.service);
                let query = BooleanQuery::new(vec![
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
                            service_term,
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                    (
                        Occur::Must,
                        Box::new(RangeQuery::new(
                            Bound::Included(Term::from_field_u64(fields.seq, delete_from_seq)),
                            Bound::Unbounded,
                        )),
                    ),
                ]);
                writer.delete_query(Box::new(query))?;
            }
            for pending in pending_docs {
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
                if !pending.dynamic_fields.is_empty() {
                    doc.add_object(fields.attrs, dynamic_fields_object(pending.dynamic_fields));
                }
                writer.add_document(doc)?;
            }
            writer.commit()?;
        }
        self.reader.read().unwrap().reload()?;
        self.clear_facet_cache();

        {
            let mut ingest = self.ingest.lock().unwrap();
            ingest.version = 1;
            for update in pending_updates {
                ingest.sources.insert(update.key, update.cursor);
            }
            for (key, field_names) in dynamic_field_names_by_source {
                ingest
                    .facet_fields
                    .entry(key)
                    .or_default()
                    .extend(field_names);
            }
            let bytes = serde_json::to_vec_pretty(&*ingest)?;
            atomic_write(&self.ingest_state_path, &bytes)?;
        }

        Ok(())
    }
}

#[derive(Default)]
struct IngestBatch {
    pending_updates: Vec<PendingUpdate>,
    pending_docs: Vec<PendingDoc>,
    pending_doc_bytes: usize,
    dynamic_field_names_by_source: HashMap<String, BTreeSet<String>>,
}

struct PendingUpdate {
    key: String,
    cursor: IngestCursor,
    run_id: String,
    service: String,
    delete_from_seq: Option<u64>,
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

impl IngestBatch {
    fn add_doc(&mut self, doc: PendingDoc) {
        self.pending_doc_bytes = self
            .pending_doc_bytes
            .saturating_add(doc.raw.len())
            .saturating_add(doc.message.len());
        for (field_name, _) in &doc.dynamic_fields {
            self.dynamic_field_names_by_source
                .entry(LogIndex::source_key(&doc.run_id, &doc.service))
                .or_default()
                .insert(field_name.clone());
        }
        self.pending_docs.push(doc);
    }

    fn record_update(&mut self, update: PendingUpdate) {
        if let Some(existing) = self
            .pending_updates
            .iter_mut()
            .find(|existing| existing.key == update.key)
        {
            existing.cursor = update.cursor;
            if existing.delete_from_seq.is_none() {
                existing.delete_from_seq = update.delete_from_seq;
            }
            return;
        }
        self.pending_updates.push(update);
    }

    fn should_flush(&self) -> bool {
        self.pending_docs.len() >= INGEST_COMMIT_DOC_LIMIT
            || self.pending_doc_bytes >= INGEST_COMMIT_BYTE_LIMIT
    }
}

fn dynamic_fields_object(fields: Vec<(String, String)>) -> BTreeMap<String, OwnedValue> {
    fields
        .into_iter()
        .map(|(field, value)| (field, OwnedValue::Str(value)))
        .collect()
}

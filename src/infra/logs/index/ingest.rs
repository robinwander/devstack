use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Bound;

use anyhow::{Context, Result};
use tantivy::Term;
use tantivy::query::{BooleanQuery, Occur, RangeQuery, TermQuery};

use crate::logfmt::{
    classify_line_level, extract_log_content, extract_timestamp_str, parse_timestamp_nanos,
};
use crate::logfmt::{contains_ansi, strip_ansi};
use crate::util::atomic_write;

use super::{IngestCursor, LogIndex, LogSource};

impl LogIndex {
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

        let mut pending_updates = Vec::new();
        let mut pending_docs = Vec::new();
        let mut dynamic_field_names = BTreeSet::new();
        let mut dynamic_field_names_by_source: HashMap<String, BTreeSet<String>> = HashMap::new();

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
                    dynamic_field_names_by_source
                        .entry(key.clone())
                        .or_default()
                        .insert(field_name.clone());
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
            for update in &pending_updates {
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
                            Bound::Included(Term::from_field_u64(
                                fields.seq,
                                update.delete_from_seq,
                            )),
                            Bound::Unbounded,
                        )),
                    ),
                ]);
                writer.delete_query(Box::new(query))?;
            }
            for doc in docs {
                writer.add_document(doc)?;
            }
            writer.commit()?;
        }
        self.reader.read().unwrap().reload()?;

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

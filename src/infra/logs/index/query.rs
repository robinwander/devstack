use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::path::Path;

use anyhow::{Result, anyhow};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::{Field, Value};
use tantivy::{DocAddress, Index, Term};

use crate::api::{FacetFilter, LogEntry, LogViewQuery, LogViewResponse, LogsQuery, LogsResponse};
use crate::logfmt::parse_timestamp_nanos;

use super::facets::{FacetCountCollector, ScopeStatsCollector};
use super::{LogIndex, LogIndexFields, LogSource};

impl LogIndex {
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

        let scope_query = Self::build_scope_query(
            &fields,
            run_id,
            Some(service),
            since_nanos,
            stream_filter,
            None,
            None,
        )?;

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
        let scope_stats = searcher.search(&scope_query, &ScopeStatsCollector::new(fields.level))?;
        let total = scope_stats.total;
        let error_count = scope_stats.error_count;
        let warn_count = scope_stats.warn_count;

        let mut lines: Vec<(i64, u64, String)> = Vec::new();
        let mut next_after: Option<u64> = None;

        let matched_total = if tail > 0 {
            if after.is_some() {
                let (matched_total, top_docs): (usize, Vec<(u64, DocAddress)>) = searcher.search(
                    &result_query,
                    &(
                        Count,
                        TopDocs::with_limit(tail)
                            .order_by_fast_field::<u64>("seq", tantivy::Order::Asc),
                    ),
                )?;
                for (_sort, addr) in top_docs {
                    let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
                    let raw = doc
                        .get_first(fields.raw)
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let ts = doc
                        .get_first(fields.ts_nanos)
                        .and_then(|value| value.as_i64())
                        .unwrap_or(0);
                    let seq = doc
                        .get_first(fields.seq)
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0);
                    next_after = Some(next_after.map(|value| value.max(seq)).unwrap_or(seq));
                    lines.push((ts, seq, raw));
                }
                matched_total
            } else {
                let (matched_total, top_docs): (usize, Vec<(i64, DocAddress)>) = searcher.search(
                    &result_query,
                    &(
                        Count,
                        TopDocs::with_limit(tail)
                            .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Desc),
                    ),
                )?;
                for (_sort, addr) in top_docs {
                    let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
                    let raw = doc
                        .get_first(fields.raw)
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let ts = doc
                        .get_first(fields.ts_nanos)
                        .and_then(|value| value.as_i64())
                        .unwrap_or(0);
                    let seq = doc
                        .get_first(fields.seq)
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0);
                    next_after = Some(next_after.map(|value| value.max(seq)).unwrap_or(seq));
                    lines.push((ts, seq, raw));
                }
                lines.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
                matched_total
            }
        } else {
            searcher.search(&result_query, &Count)?
        };

        Ok(LogsResponse {
            lines: lines.into_iter().map(|(_, _, line)| line).collect(),
            truncated: matched_total > tail && tail > 0,
            total,
            error_count,
            warn_count,
            next_after,
            matched_total,
        })
    }

    pub(crate) fn query_view(&self, run_id: &str, query: LogViewQuery) -> Result<LogViewResponse> {
        let tail = query.last.unwrap_or(500);
        let level_filter = query.level.as_deref().unwrap_or("all");
        let stream_filter = query.stream.as_deref();
        let service_filter = query.service.as_deref();
        let since_nanos = query.since.as_deref().and_then(parse_timestamp_nanos);
        let fields = self.fields.clone();

        let mut view_query = Self::build_scope_query(
            &fields,
            run_id,
            service_filter,
            since_nanos,
            stream_filter,
            None,
            None,
        )?;
        view_query = Self::add_level_filter(fields.level, view_query, level_filter)?;

        let (all_dynamic_fields, facet_fields) = {
            let index = self.index.read().unwrap();
            view_query =
                Self::add_text_query(&index, fields.message, view_query, query.search.as_deref())?;
            let all_dynamic_fields = if query.include_entries || query.include_facets {
                Self::dynamic_attribute_fields(&index.schema())
            } else {
                Vec::new()
            };
            let facet_fields = if query.include_facets {
                self.facet_fields_for_scope(run_id, service_filter)
            } else {
                Vec::new()
            };
            (all_dynamic_fields, facet_fields)
        };
        let attribute_fields = if query.include_entries {
            all_dynamic_fields.clone()
        } else {
            Vec::new()
        };

        let searcher = self.reader.read().unwrap().searcher();

        let (total, top_docs, facet_counts) = match (
            query.include_entries && tail > 0,
            query.include_facets && !facet_fields.is_empty(),
        ) {
            (true, true) => {
                let (total, top_docs, facet_counts) = searcher.search(
                    view_query.as_ref(),
                    &(
                        Count,
                        TopDocs::with_limit(tail)
                            .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Desc),
                        FacetCountCollector::new(&facet_fields),
                    ),
                )?;
                (total, top_docs, facet_counts)
            }
            (true, false) => {
                let (total, top_docs) = searcher.search(
                    view_query.as_ref(),
                    &(
                        Count,
                        TopDocs::with_limit(tail)
                            .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Desc),
                    ),
                )?;
                (total, top_docs, HashMap::new())
            }
            (false, true) => {
                let (total, facet_counts) = searcher.search(
                    view_query.as_ref(),
                    &(Count, FacetCountCollector::new(&facet_fields)),
                )?;
                (total, Vec::new(), facet_counts)
            }
            (false, false) => (
                searcher.search(view_query.as_ref(), &Count)?,
                Vec::new(),
                HashMap::new(),
            ),
        };

        let mut entries: Vec<(i64, u64, LogEntry)> = Vec::new();
        if query.include_entries && tail > 0 {
            for (_sort, addr) in top_docs {
                let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
                let ts = doc
                    .get_first(fields.ts)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let service = doc
                    .get_first(fields.service)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let stream = doc
                    .get_first(fields.stream)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let level = doc
                    .get_first(fields.level)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let message = doc
                    .get_first(fields.message)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let raw = doc
                    .get_first(fields.raw)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let ts_nanos = doc
                    .get_first(fields.ts_nanos)
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0);
                let seq = doc
                    .get_first(fields.seq)
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);

                let mut attributes = BTreeMap::new();
                for (field_name, field_handle) in &attribute_fields {
                    if let Some(value) = doc
                        .get_first(*field_handle)
                        .and_then(|value| value.as_str())
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
            entries.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        }

        let mut filters = Vec::new();
        if query.include_facets {
            for field_name in facet_fields {
                let values = Self::facet_values_from_counts(facet_counts.get(&field_name));
                if values.is_empty() {
                    continue;
                }
                filters.push(FacetFilter {
                    field: field_name.clone(),
                    kind: Self::facet_kind_for(&field_name).to_string(),
                    values,
                });
            }
            filters.sort_by(|left, right| {
                Self::facet_sort_rank(&left.field)
                    .cmp(&Self::facet_sort_rank(&right.field))
                    .then(left.field.cmp(&right.field))
            });
        }

        Ok(LogViewResponse {
            entries: entries.into_iter().map(|(_, _, entry)| entry).collect(),
            truncated: query.include_entries && total > tail && tail > 0,
            total,
            filters,
        })
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
                let term = Term::from_field_text(level_field, "warn");
                clauses.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        term,
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
        query: Option<&str>,
    ) -> Result<Box<dyn Query>> {
        let Some(query) = query else {
            return Ok(base);
        };
        let query = query.trim();
        if query.is_empty() {
            return Ok(base);
        }

        let query_parser = QueryParser::for_index(index, vec![message_field]);
        let parsed = match query_parser.parse_query(query) {
            Ok(query) => query,
            Err(err) => return Err(anyhow!("bad_query: {err}")),
        };
        Ok(Box::new(BooleanQuery::new(vec![
            (Occur::Must, base),
            (Occur::Must, parsed),
        ])))
    }
}

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
use super::{FacetCacheKey, LogIndex, LogIndexFields, LogSource};

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

        let tail = query.filter.last.unwrap_or(500);
        let level_filter = query.filter.level.as_deref().unwrap_or("all");
        let stream_filter = query.filter.stream.as_deref();
        let since_nanos = query
            .filter
            .since
            .as_deref()
            .and_then(parse_timestamp_nanos);
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
                query.filter.search.as_deref(),
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
        let facet_cache_key = Self::facet_cache_key(run_id, &query);
        if let Some(cache_key) = facet_cache_key.as_ref()
            && let Some(response) = self.facet_cache.lock().unwrap().get(cache_key).cloned()
        {
            return Ok(response);
        }

        let tail = query.filter.last.unwrap_or(500);
        let level_filter = query.filter.level.as_deref().unwrap_or("all");
        let stream_filter = query.filter.stream.as_deref();
        let service_filter = query.service.as_deref();
        let since_nanos = query
            .filter
            .since
            .as_deref()
            .and_then(parse_timestamp_nanos);
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

        let facet_fields = {
            let index = self.index.read().unwrap();
            view_query = Self::add_text_query(
                &index,
                fields.message,
                view_query,
                query.filter.search.as_deref(),
            )?;
            if query.include_facets {
                self.facet_fields_for_scope(run_id, service_filter)
            } else {
                Vec::new()
            }
        };

        let searcher = self.reader.read().unwrap().searcher();
        let include_entries = query.include_entries && tail > 0;
        let include_facets = query.include_facets && !facet_fields.is_empty();

        let (total, total_exact, top_docs, facet_counts, truncated) =
            match (include_entries, include_facets, query.include_total) {
                (true, true, _) => {
                    let (total, top_docs, facet_counts) = searcher.search(
                        view_query.as_ref(),
                        &(
                            Count,
                            TopDocs::with_limit(tail)
                                .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Desc),
                            FacetCountCollector::new(&facet_fields),
                        ),
                    )?;
                    (total, true, top_docs, facet_counts, total > tail)
                }
                (true, false, true) => {
                    let (total, top_docs) = searcher.search(
                        view_query.as_ref(),
                        &(
                            Count,
                            TopDocs::with_limit(tail)
                                .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Desc),
                        ),
                    )?;
                    (total, true, top_docs, HashMap::new(), total > tail)
                }
                (true, false, false) => {
                    let mut top_docs = searcher.search(
                        view_query.as_ref(),
                        &TopDocs::with_limit(tail.saturating_add(1))
                            .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Desc),
                    )?;
                    let truncated = top_docs.len() > tail;
                    if truncated {
                        top_docs.truncate(tail);
                    }
                    let total = if truncated {
                        tail.saturating_add(1)
                    } else {
                        top_docs.len()
                    };
                    (total, false, top_docs, HashMap::new(), truncated)
                }
                (false, true, _) => {
                    let (total, facet_counts) = searcher.search(
                        view_query.as_ref(),
                        &(Count, FacetCountCollector::new(&facet_fields)),
                    )?;
                    (total, true, Vec::new(), facet_counts, false)
                }
                (false, false, true) => (
                    searcher.search(view_query.as_ref(), &Count)?,
                    true,
                    Vec::new(),
                    HashMap::new(),
                    false,
                ),
                (false, false, false) => (0, false, Vec::new(), HashMap::new(), false),
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
                let json = parse_raw_json_object(&raw);
                let ts_nanos = doc
                    .get_first(fields.ts_nanos)
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0);
                let seq = doc
                    .get_first(fields.seq)
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);

                let attributes = doc
                    .get_first(fields.attrs)
                    .map(Self::attributes_from_value)
                    .unwrap_or_default();

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
                        json,
                        attributes,
                    },
                ));
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        }

        let mut filters = Vec::new();
        if query.include_facets {
            for spec in facet_fields {
                let values = Self::facet_values_from_counts(facet_counts.get(&spec.field));
                if values.is_empty() {
                    continue;
                }
                filters.push(FacetFilter {
                    field: spec.field.clone(),
                    kind: Self::facet_kind_for(&spec.field).to_string(),
                    values,
                });
            }
            filters.sort_by(|left, right| {
                Self::facet_sort_rank(&left.field)
                    .cmp(&Self::facet_sort_rank(&right.field))
                    .then(left.field.cmp(&right.field))
            });
        }

        let response = LogViewResponse {
            entries: entries.into_iter().map(|(_, _, entry)| entry).collect(),
            truncated: query.include_entries && truncated,
            total,
            total_exact,
            filters,
        };
        if let Some(cache_key) = facet_cache_key {
            self.facet_cache
                .lock()
                .unwrap()
                .insert(cache_key, response.clone());
        }
        Ok(response)
    }

    fn facet_cache_key(run_id: &str, query: &LogViewQuery) -> Option<FacetCacheKey> {
        (query.include_facets && !query.include_entries).then(|| FacetCacheKey {
            run_id: run_id.to_string(),
            service: query.service.clone(),
            since: query.filter.since.clone(),
            search: query.filter.search.clone(),
            level: query.filter.level.clone(),
            stream: query.filter.stream.clone(),
        })
    }

    pub(crate) fn warm_facets(&self, run_id: &str) {
        let _ = self.query_view(
            run_id,
            LogViewQuery {
                filter: Default::default(),
                service: None,
                include_entries: false,
                include_facets: true,
                include_total: true,
            },
        );
    }

    pub(crate) fn clear_facet_cache(&self) {
        self.facet_cache.lock().unwrap().clear();
    }

    pub(crate) fn count_run(&self, run_id: &str) -> Result<usize> {
        let query = Self::build_scope_query(&self.fields, run_id, None, None, None, None, None)?;
        let searcher = self.reader.read().unwrap().searcher();
        searcher.search(query.as_ref(), &Count).map_err(Into::into)
    }

    fn attributes_from_value<'a, V>(value: V) -> BTreeMap<String, String>
    where
        V: Value<'a>,
    {
        let mut attributes = BTreeMap::new();
        let Some(fields) = value.as_object() else {
            return attributes;
        };

        for (field, value) in fields {
            if let Some(value) = Self::attribute_value_to_string(value)
                && !value.is_empty()
            {
                attributes.insert(field.to_string(), value);
            }
        }
        attributes
    }

    fn attribute_value_to_string<'a, V>(value: V) -> Option<String>
    where
        V: Value<'a>,
    {
        if let Some(value) = value.as_str() {
            return Some(value.to_string());
        }
        if let Some(value) = value.as_u64() {
            return Some(value.to_string());
        }
        if let Some(value) = value.as_i64() {
            return Some(value.to_string());
        }
        if let Some(value) = value.as_f64() {
            return Some(value.to_string());
        }
        if let Some(value) = value.as_bool() {
            return Some(value.to_string());
        }
        None
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

fn parse_raw_json_object(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    value.is_object().then_some(value)
}

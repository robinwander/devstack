use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};
use tantivy::schema::Field;
use tantivy::{Index, IndexReader, IndexWriter};

mod compaction;
mod facets;
mod ingest;
mod query;
mod schema;

const CURRENT_SCHEMA_VERSION: &str = "4";
const FACET_TERMS_LIMIT: u32 = 50;
const FACET_STORE_CACHE_BLOCKS: usize = 32;
const COMPACTION_SEGMENT_BATCH_SIZE: usize = 32;
const COMPACTION_MAX_BATCHES_PER_PASS: usize = 8;

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
    #[serde(default)]
    facet_fields: HashMap<String, BTreeSet<String>>,
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
    ingest_gate: Mutex<()>,
    ingest: Mutex<IngestStateFile>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{LogViewQuery, LogViewResponse, LogsQuery};
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

    fn log_view_query(
        last: Option<usize>,
        search: Option<&str>,
        level: Option<&str>,
        stream: Option<&str>,
        service: Option<&str>,
        include_entries: bool,
        include_facets: bool,
    ) -> LogViewQuery {
        LogViewQuery {
            last,
            since: None,
            search: search.map(str::to_string),
            level: level.map(str::to_string),
            stream: stream.map(str::to_string),
            service: service.map(str::to_string),
            include_entries,
            include_facets,
        }
    }

    fn facet_values(response: &LogViewResponse, field: &str) -> Vec<(String, usize)> {
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
            .query_view(
                "run-1",
                log_view_query(Some(10), Some("error"), None, None, None, true, false),
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

        let search = index
            .query_view(
                "run-no-auto-ingest",
                log_view_query(Some(10), None, None, None, None, true, false),
            )
            .unwrap();
        assert_eq!(search.total, 0);
        assert!(search.entries.is_empty());

        let facets = index
            .query_view(
                "run-no-auto-ingest",
                log_view_query(None, None, None, None, None, false, true),
            )
            .unwrap();
        assert_eq!(facets.total, 0);
        assert!(facets.filters.is_empty());
    }

    #[test]
    fn delete_run_removes_entries_and_resets_ingest_state() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let log_path = dir.path().join("api.log");
        std::fs::write(&log_path, "[2025-01-01T00:00:00Z] [stdout] hello\n").unwrap();

        let sources = vec![LogSource {
            run_id: "run-delete".to_string(),
            service: "api".to_string(),
            path: log_path.clone(),
        }];
        ingest(&index, &sources);
        assert_eq!(
            index
                .query_view(
                    "run-delete",
                    log_view_query(Some(10), None, None, None, None, true, false),
                )
                .unwrap()
                .total,
            1
        );

        index.delete_run("run-delete").unwrap();
        assert_eq!(
            index
                .query_view(
                    "run-delete",
                    log_view_query(Some(10), None, None, None, None, true, false),
                )
                .unwrap()
                .total,
            0
        );

        ingest(&index, &sources);
        assert_eq!(
            index
                .query_view(
                    "run-delete",
                    log_view_query(Some(10), None, None, None, None, true, false),
                )
                .unwrap()
                .total,
            1
        );
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
            .query_view(
                "run-json",
                log_view_query(Some(10), None, None, None, None, true, false),
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
            .query_view(
                "run-level",
                log_view_query(Some(10), None, Some("error"), None, None, true, false),
            )
            .unwrap();

        assert_eq!(errors.entries.len(), 0);
        assert_eq!(errors.total, 0);
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
            .query_view(
                "run-order",
                log_view_query(Some(10), None, None, None, None, true, false),
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
            .query_view(
                "run-facets",
                log_view_query(None, None, None, None, None, false, true),
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
            .query_view(
                "run-dynamic-facets",
                log_view_query(None, None, None, None, None, false, true),
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
    fn facets_reflect_full_current_scope() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let api_log = dir.path().join("api.log");
        let worker_log = dir.path().join("worker.log");

        std::fs::write(
            &api_log,
            concat!(
                "[2025-01-01T00:00:00Z] [stderr] Error: api failed\n",
                "[2025-01-01T00:00:01Z] [stdout] api recovered\n",
            ),
        )
        .unwrap();
        std::fs::write(
            &worker_log,
            "[2025-01-01T00:00:02Z] [stdout] Error: worker failed\n",
        )
        .unwrap();

        let sources = vec![
            LogSource {
                run_id: "run-filtered-facets".to_string(),
                service: "api".to_string(),
                path: api_log,
            },
            LogSource {
                run_id: "run-filtered-facets".to_string(),
                service: "worker".to_string(),
                path: worker_log,
            },
        ];
        ingest(&index, &sources);

        let response = index
            .query_view(
                "run-filtered-facets",
                log_view_query(None, None, Some("error"), None, Some("api"), false, true),
            )
            .unwrap();

        assert_eq!(
            facet_values(&response, "service"),
            vec![("api".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "level"),
            vec![("error".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "stream"),
            vec![("stderr".to_string(), 1)]
        );
    }

    #[test]
    fn facets_respect_search_query_scope() {
        let dir = tempfile::tempdir().unwrap();
        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        let api_log = dir.path().join("api.log");
        let worker_log = dir.path().join("worker.log");

        std::fs::write(
            &api_log,
            concat!(
                "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"level\":\"info\",\"msg\":\"fetch ok\",\"method\":\"GET\"}\n",
                "{\"time\":\"2025-01-01T00:00:01Z\",\"stream\":\"stderr\",\"level\":\"error\",\"msg\":\"timeout\",\"method\":\"POST\"}\n",
            ),
        )
        .unwrap();
        std::fs::write(
            &worker_log,
            "{\"time\":\"2025-01-01T00:00:02Z\",\"stream\":\"stderr\",\"level\":\"error\",\"msg\":\"timeout\",\"method\":\"POST\"}\n",
        )
        .unwrap();

        let sources = vec![
            LogSource {
                run_id: "run-search-facets".to_string(),
                service: "api".to_string(),
                path: api_log,
            },
            LogSource {
                run_id: "run-search-facets".to_string(),
                service: "worker".to_string(),
                path: worker_log,
            },
        ];
        ingest(&index, &sources);

        let response = index
            .query_view(
                "run-search-facets",
                log_view_query(None, Some("timeout"), None, None, None, false, true),
            )
            .unwrap();

        assert_eq!(
            facet_values(&response, "service"),
            vec![("api".to_string(), 1), ("worker".to_string(), 1)]
        );
        assert_eq!(
            facet_values(&response, "method"),
            vec![("POST".to_string(), 2)]
        );
        assert_eq!(
            facet_values(&response, "level"),
            vec![("error".to_string(), 2)]
        );
    }

    #[test]
    fn schema_version_mismatch_rebuilds_index_state() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("logs_index");
        std::fs::create_dir_all(&index_dir).unwrap();
        std::fs::write(index_dir.join("schema_version"), "2").unwrap();
        std::fs::write(index_dir.join("sentinel"), "stale").unwrap();
        std::fs::write(
            index_dir.join("ingest_state.json"),
            r#"{"version":1,"sources":{"run-1/api":{"offset":123,"next_seq":4}}}"#,
        )
        .unwrap();

        let index = LogIndex::open_or_create_in(dir.path()).unwrap();

        assert!(!index_dir.join("sentinel").exists());
        assert_eq!(
            std::fs::read_to_string(index_dir.join("schema_version")).unwrap(),
            CURRENT_SCHEMA_VERSION
        );
        assert!(index.ingest.lock().unwrap().sources.is_empty());
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

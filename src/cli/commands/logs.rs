use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::api::{LogFilterQuery, LogViewQuery, LogViewResponse, LogsQuery, LogsResponse};
use crate::cli::context::{
    CliContext, DAEMON_LONG_TIMEOUT, resolve_active_run_id, resolve_follow_for,
    resolve_latest_run_id, resolve_project_dir_from_cwd, resolve_run_id,
};
use crate::cli::output::{print_entry, print_json, print_lines, print_log_facets};
use crate::infra::logs::index::{LogIndex, LogSource};
use crate::logs::{LogOutputFormat, stream_logs};
use crate::paths;
use crate::sources::{SourcesLedger, source_run_id};
use crate::util::expand_home;

async fn resolve_log_target(
    context: &CliContext,
    project_dir: &Path,
    target: Option<String>,
    source_flag: Option<String>,
    service_flag: Option<String>,
    run_id: &Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    if source_flag.is_some() || service_flag.is_some() {
        return Ok((source_flag, service_flag));
    }
    let Some(target) = target else {
        return Ok((None, None));
    };

    let run_id = if let Some(rid) = run_id {
        Some(rid.clone())
    } else {
        resolve_latest_run_id(context, project_dir)
            .await
            .ok()
            .flatten()
    };

    if let Some(rid) = &run_id
        && let Ok(status) = context
            .daemon_request_json::<(), crate::api::RunStatusResponse>(
                "GET",
                &format!("/v1/runs/{rid}/status"),
                None,
                Some(std::time::Duration::from_secs(5)),
            )
            .await
        && status.services.contains_key(&target)
    {
        return Ok((None, Some(target)));
    }

    if let Ok(ledger) = SourcesLedger::load()
        && ledger.sources.contains_key(&target)
    {
        return Ok((Some(target), None));
    }

    Ok((None, Some(target)))
}

fn service_log_output_format(context: &CliContext) -> LogOutputFormat {
    if context.interactive {
        LogOutputFormat::Text
    } else {
        LogOutputFormat::Json
    }
}

fn aggregate_log_output_format() -> LogOutputFormat {
    LogOutputFormat::Json
}

pub(crate) fn normalize_since_arg(since: Option<String>) -> Result<Option<String>> {
    let Some(since) = since else {
        return Ok(None);
    };
    let since = since.trim().to_string();
    if since.is_empty() {
        return Ok(None);
    }
    if OffsetDateTime::parse(&since, &Rfc3339).is_ok() {
        return Ok(Some(since));
    }
    if let Ok(duration) = humantime::parse_duration(&since) {
        let timestamp = (OffsetDateTime::now_utc() - duration).format(&Rfc3339)?;
        return Ok(Some(timestamp));
    }
    Err(anyhow!(
        "invalid --since value {since:?}; use RFC3339 (e.g. 2025-01-01T00:00:00Z) or a duration (e.g. 5m, 1h)"
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    context: &CliContext,
    run_id: Option<String>,
    source: Option<String>,
    facets: bool,
    all: bool,
    target: Option<String>,
    service: Option<String>,
    task: Option<String>,
    tail: Option<usize>,
    q: Option<String>,
    level: Option<String>,
    errors: bool,
    stream: Option<String>,
    since: Option<String>,
    no_health: bool,
    follow: bool,
    follow_for: Option<Duration>,
) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;

    let (source, service) =
        resolve_log_target(context, &project_dir, target, source, service, &run_id).await?;
    let follow_for = resolve_follow_for(follow, follow_for, context.interactive);
    let since = normalize_since_arg(since)?;
    let level = if errors {
        Some("error".to_string())
    } else {
        level
    };
    let view_query =
        |tail: Option<usize>, include_entries: bool, include_facets: bool| LogViewQuery {
            filter: LogFilterQuery {
                last: tail,
                since: since.clone(),
                search: q.clone(),
                level: level.clone(),
                stream: stream.clone(),
            },
            service: service.clone(),
            include_entries,
            include_facets,
        };

    if let Some(source_name) = source {
        if follow {
            return Err(anyhow!("--follow is not supported with --source"));
        }

        let response = query_source_log_view(
            context,
            &source_name,
            view_query(tail.or(Some(500)), !facets, facets),
        )
        .await?;
        if context.interactive {
            if facets {
                print_log_facets(&format!("Source: {source_name}"), &response);
            } else {
                for entry in &response.entries {
                    print_entry(entry, aggregate_log_output_format(), no_health);
                }
            }
        } else if facets {
            print_json(&response);
        } else {
            for entry in &response.entries {
                print_entry(entry, aggregate_log_output_format(), no_health);
            }
        }
        return Ok(());
    }

    if facets {
        let run_id = resolve_run_id(context, &project_dir, run_id).await?;
        let response = fetch_run_log_view(context, &run_id, view_query(None, false, true)).await?;
        if context.interactive {
            print_log_facets(&format!("Run: {run_id}"), &response);
        } else {
            print_json(&response);
        }
        return Ok(());
    }

    if let Some(task_name) = task {
        if all
            || service.is_some()
            || q.is_some()
            || level.is_some()
            || stream.is_some()
            || since.is_some()
        {
            return Err(anyhow!(
                "--task cannot be combined with --all, --service, --search, --level, --stream, or --since"
            ));
        }

        let candidates =
            task_log_path_candidates(&project_dir, context, &task_name, run_id.as_deref()).await?;
        let log_path = candidates.iter().find(|path| path.exists()).cloned();

        let Some(log_path) = log_path else {
            let looked_at = candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(anyhow!(
                "task log not found for '{task_name}' (looked at {looked_at})"
            ));
        };

        return stream_logs(
            &log_path,
            &task_name,
            tail,
            follow,
            follow_for,
            service_log_output_format(context),
            no_health,
        )
        .await;
    }

    let run_id = resolve_run_id(context, &project_dir, run_id).await?;

    if all {
        if follow {
            return Err(anyhow!(
                "--follow requires --service (cannot be used with --all)"
            ));
        }
        let tail = tail.unwrap_or(500);
        let response =
            fetch_run_log_view(context, &run_id, view_query(Some(tail), true, false)).await?;
        let output_format = aggregate_log_output_format();
        for entry in &response.entries {
            print_entry(entry, output_format, no_health);
        }
        return Ok(());
    }

    let Some(service) = service else {
        return Err(anyhow!(
            "--service is required unless --all or --task is set"
        ));
    };

    let api_only = q.is_some() || level.is_some() || stream.is_some() || since.is_some();

    let api_result = if follow {
        stream_service_logs_api(
            context,
            &run_id,
            &service,
            tail.unwrap_or(200),
            q.as_deref(),
            level.as_deref(),
            stream.as_deref(),
            since.as_deref(),
            follow_for,
            no_health,
        )
        .await
    } else {
        let tail = tail.unwrap_or(500);
        let response = fetch_service_logs_api(
            context,
            &run_id,
            &service,
            tail,
            None,
            q.as_deref(),
            level.as_deref(),
            stream.as_deref(),
            since.as_deref(),
        )
        .await?;
        print_lines(
            &service,
            &response.lines,
            service_log_output_format(context),
            no_health,
        );
        Ok(())
    };

    match api_result {
        Ok(()) => Ok(()),
        Err(_) if !api_only => {
            let log_path = paths::run_log_path(
                &crate::ids::RunId::new(run_id),
                &crate::ids::ServiceName::new(&service),
            )?;
            stream_logs(
                &log_path,
                &service,
                tail,
                follow,
                follow_for,
                service_log_output_format(context),
                no_health,
            )
            .await
        }
        Err(err) => Err(err),
    }
}

fn build_query_string(params: Vec<(&str, String)>) -> String {
    let mut out = String::new();
    for (key, value) in params {
        if value.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push('?');
        } else {
            out.push('&');
        }
        out.push_str(key);
        out.push('=');
        out.push_str(&urlencoding::encode(&value));
    }
    out
}

fn push_log_filter_query_params(params: &mut Vec<(&str, String)>, filter: &LogFilterQuery) {
    if let Some(last) = filter.last {
        params.push(("last", last.to_string()));
    }
    if let Some(search) = filter.search.as_deref() {
        params.push(("search", search.to_string()));
    }
    if let Some(level) = filter.level.as_deref()
        && level != "all"
    {
        params.push(("level", level.to_string()));
    }
    if let Some(stream) = filter.stream.as_deref() {
        params.push(("stream", stream.to_string()));
    }
    if let Some(since) = filter.since.as_deref() {
        params.push(("since", since.to_string()));
    }
}

fn build_logs_query(
    tail: usize,
    after: Option<u64>,
    query: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
) -> LogsQuery {
    LogsQuery {
        filter: LogFilterQuery {
            last: Some(tail),
            since: since.map(str::to_string),
            search: query.map(str::to_string),
            level: level.map(str::to_string),
            stream: stream.map(str::to_string),
        },
        after,
    }
}

fn build_log_view_query_string(query: &LogViewQuery) -> String {
    let mut params = Vec::new();
    push_log_filter_query_params(&mut params, &query.filter);
    if let Some(service) = query.service.as_deref() {
        params.push(("service", service.to_string()));
    }
    if !query.include_entries {
        params.push(("include_entries", "false".to_string()));
    }
    if query.include_facets {
        params.push(("include_facets", "true".to_string()));
    }
    build_query_string(params)
}

fn build_logs_query_string(query: &LogsQuery) -> String {
    let mut params = Vec::new();
    push_log_filter_query_params(&mut params, &query.filter);
    if let Some(after) = query.after {
        params.push(("after", after.to_string()));
    }
    build_query_string(params)
}

#[allow(clippy::too_many_arguments)]
async fn fetch_service_logs_api(
    context: &CliContext,
    run_id: &str,
    service: &str,
    tail: usize,
    after: Option<u64>,
    q: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
) -> Result<LogsResponse> {
    let query = build_logs_query(tail, after, q, level, stream, since);
    let query = build_logs_query_string(&query);
    let path = format!("/v1/runs/{run_id}/logs/{service}{query}");
    context
        .daemon_request_json::<(), LogsResponse>("GET", &path, None, Some(DAEMON_LONG_TIMEOUT))
        .await
}

async fn fetch_run_log_view(
    context: &CliContext,
    run_id: &str,
    query: LogViewQuery,
) -> Result<LogViewResponse> {
    let query = build_log_view_query_string(&query);
    let path = format!("/v1/runs/{run_id}/logs{query}");
    context
        .daemon_request_json::<(), LogViewResponse>("GET", &path, None, Some(DAEMON_LONG_TIMEOUT))
        .await
}

fn source_log_sources(ledger: &SourcesLedger, source_name: &str) -> Result<Vec<LogSource>> {
    let run_id = source_run_id(source_name);
    let resolved = ledger.resolve_log_sources(source_name)?;
    Ok(resolved
        .into_iter()
        .map(|item| LogSource {
            run_id: run_id.clone(),
            service: item.service,
            path: item.path,
        })
        .collect())
}

fn query_source_log_view_local(
    index: &LogIndex,
    ledger: &SourcesLedger,
    source_name: &str,
    query: LogViewQuery,
) -> Result<LogViewResponse> {
    let sources = source_log_sources(ledger, source_name)?;
    if sources.is_empty() {
        return Ok(LogViewResponse {
            entries: Vec::new(),
            truncated: false,
            total: 0,
            filters: Vec::new(),
        });
    }

    index.ingest_sources(&sources)?;
    let run_id = source_run_id(source_name);
    index.query_view(&run_id, query)
}

async fn query_source_log_view(
    context: &CliContext,
    source_name: &str,
    query: LogViewQuery,
) -> Result<LogViewResponse> {
    if context.daemon_is_running() {
        let query_str = build_log_view_query_string(&query);
        let path = format!("/v1/sources/{source_name}/logs{query_str}");
        return context
            .daemon_request_json::<(), LogViewResponse>(
                "GET",
                &path,
                None,
                Some(DAEMON_LONG_TIMEOUT),
            )
            .await;
    }

    let source_name = source_name.to_string();
    tokio::task::spawn_blocking(move || {
        let ledger = SourcesLedger::load()?;
        let index = LogIndex::open_or_create()?;
        query_source_log_view_local(&index, &ledger, &source_name, query)
    })
    .await
    .map_err(|err| anyhow!("source log view task failed: {err}"))?
}

pub(crate) async fn refresh_source_index(source_name: &str) -> Result<()> {
    let source_name = source_name.to_string();
    tokio::task::spawn_blocking(move || {
        let index = LogIndex::open_or_create()?;
        let run_id = source_run_id(&source_name);
        index.delete_run(&run_id)?;

        let ledger = SourcesLedger::load()?;
        if ledger.get(&source_name).is_some() {
            let sources = source_log_sources(&ledger, &source_name)?;
            if !sources.is_empty() {
                index.ingest_sources(&sources)?;
            }
        }

        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|err| anyhow!("source index refresh task failed: {err}"))??;
    Ok(())
}

pub(crate) fn absolutize_source_patterns(paths: Vec<String>) -> Result<Vec<String>> {
    let cwd = std::env::current_dir()?;
    Ok(paths
        .into_iter()
        .map(|pattern| {
            let expanded = expand_home(Path::new(&pattern));
            if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            }
            .to_string_lossy()
            .to_string()
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
async fn stream_service_logs_api(
    context: &CliContext,
    run_id: &str,
    service: &str,
    initial_tail: usize,
    q: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
    follow_for: Option<Duration>,
    no_health: bool,
) -> Result<()> {
    let start = Instant::now();

    let response = fetch_service_logs_api(
        context,
        run_id,
        service,
        initial_tail,
        None,
        q,
        level,
        stream,
        since,
    )
    .await?;
    let output_format = service_log_output_format(context);
    print_lines(service, &response.lines, output_format, no_health);
    let mut after = response.next_after;

    loop {
        if let Some(limit) = follow_for
            && start.elapsed() >= limit
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let response = fetch_service_logs_api(
            context, run_id, service, 500, after, q, level, stream, since,
        )
        .await?;
        print_lines(service, &response.lines, output_format, no_health);
        if let Some(next) = response.next_after {
            after = Some(after.map(|current| current.max(next)).unwrap_or(next));
        }
    }
}

fn task_log_path_candidates_for(
    project_dir: &Path,
    task_name: &str,
    explicit_run_id: Option<&str>,
    active_run_id: Option<&str>,
    latest_run_id: Option<&str>,
) -> Result<Vec<PathBuf>> {
    fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
        if !paths.iter().any(|candidate| candidate == &path) {
            paths.push(path);
        }
    }

    let mut candidates = Vec::new();
    if let Some(run_id) = explicit_run_id {
        push_unique(
            &mut candidates,
            paths::task_log_path(&crate::ids::RunId::new(run_id), task_name)?,
        );
        return Ok(candidates);
    }

    if let Some(run_id) = active_run_id {
        push_unique(
            &mut candidates,
            paths::task_log_path(&crate::ids::RunId::new(run_id), task_name)?,
        );
    }
    if let Some(run_id) = latest_run_id {
        push_unique(
            &mut candidates,
            paths::task_log_path(&crate::ids::RunId::new(run_id), task_name)?,
        );
    }
    push_unique(
        &mut candidates,
        paths::ad_hoc_task_log_path(project_dir, task_name)?,
    );
    Ok(candidates)
}

async fn task_log_path_candidates(
    project_dir: &Path,
    context: &CliContext,
    task_name: &str,
    explicit_run_id: Option<&str>,
) -> Result<Vec<PathBuf>> {
    if explicit_run_id.is_some() {
        return task_log_path_candidates_for(project_dir, task_name, explicit_run_id, None, None);
    }

    let active_run_id = resolve_active_run_id(context, project_dir).await?;
    let latest_run_id = resolve_latest_run_id(context, project_dir).await?;
    task_log_path_candidates_for(
        project_dir,
        task_name,
        None,
        active_run_id.as_deref(),
        latest_run_id.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_logs_always_use_json_output() {
        assert_eq!(aggregate_log_output_format(), LogOutputFormat::Json);
    }

    #[test]
    fn service_logs_use_text_output_when_interactive() {
        let context = CliContext::new(true);
        assert_eq!(service_log_output_format(&context), LogOutputFormat::Text);
    }

    #[test]
    fn service_logs_use_json_output_when_noninteractive() {
        let context = CliContext::new(false);
        assert_eq!(service_log_output_format(&context), LogOutputFormat::Json);
    }

    #[test]
    fn task_log_path_candidates_prioritize_run_logs() {
        let dir = tempfile::tempdir().unwrap();
        let candidates = task_log_path_candidates_for(
            dir.path(),
            "lint",
            None,
            Some("run-active"),
            Some("run-latest"),
        )
        .unwrap();

        assert_eq!(
            candidates,
            vec![
                paths::task_log_path(&crate::ids::RunId::new("run-active"), "lint").unwrap(),
                paths::task_log_path(&crate::ids::RunId::new("run-latest"), "lint").unwrap(),
                paths::ad_hoc_task_log_path(dir.path(), "lint").unwrap(),
            ]
        );
    }

    #[test]
    fn source_query_ingests_json_and_preserves_structured_fields() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("external.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","level":"info","msg":"ready"}
{"time":"2025-01-01T00:00:01Z","stream":"stderr","level":"error","msg":"boom"}
"#,
        )
        .unwrap();

        let mut ledger = SourcesLedger::default();
        ledger.sources.insert(
            "ext".to_string(),
            crate::sources::SourceEntry {
                name: "ext".to_string(),
                paths: vec![log_path.to_string_lossy().to_string()],
                created_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );

        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let response = query_source_log_view_local(
            &index,
            &ledger,
            "ext",
            LogViewQuery {
                filter: LogFilterQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                },
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .unwrap();

        assert_eq!(ledger.list().len(), 1);
        assert_eq!(response.entries.len(), 2);
        assert_eq!(response.entries[0].ts, "2025-01-01T00:00:00Z");
        assert_eq!(response.entries[0].level, "info");
        assert_eq!(response.entries[1].ts, "2025-01-01T00:00:01Z");
        assert_eq!(response.entries[1].level, "error");
    }

    #[test]
    fn source_remove_cleans_up_index_entries() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("external.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","msg":"ready"}
"#,
        )
        .unwrap();

        let mut ledger = SourcesLedger::default();
        ledger.sources.insert(
            "ext".to_string(),
            crate::sources::SourceEntry {
                name: "ext".to_string(),
                paths: vec![log_path.to_string_lossy().to_string()],
                created_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );

        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let _ = query_source_log_view_local(
            &index,
            &ledger,
            "ext",
            LogViewQuery {
                filter: LogFilterQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                },
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .unwrap();

        let run_id = source_run_id("ext");
        let before = index
            .query_view(
                &run_id,
                LogViewQuery {
                    filter: LogFilterQuery {
                        last: Some(10),
                        since: None,
                        search: None,
                        level: None,
                        stream: None,
                    },
                    service: None,
                    include_entries: true,
                    include_facets: false,
                },
            )
            .unwrap();
        assert!(before.total > 0);

        ledger.sources.remove("ext");
        index.delete_run(&run_id).unwrap();

        let after = index
            .query_view(
                &run_id,
                LogViewQuery {
                    filter: LogFilterQuery {
                        last: Some(10),
                        since: None,
                        search: None,
                        level: None,
                        stream: None,
                    },
                    service: None,
                    include_entries: true,
                    include_facets: false,
                },
            )
            .unwrap();
        assert_eq!(after.total, 0);
    }
}

mod support;

use std::time::Duration;

use anyhow::Result;
use devstack::api::{LogFilterQuery, LogViewQuery, LogsQuery, TaskExecutionState};
use support::fixtures;
use support::workflows::start_fixture_run;
use support::{TaskStartOptions, TestHarness, UpOptions};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[tokio::test]
async fn service_logs_are_queryable_by_service() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::multi_service()).await?;

    run.assert_ready().await?;
    let logs = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                },
                after: None,
            },
        )
        .await?;

    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("service-started name=api"))
    );
    assert!(!logs.lines.iter().any(|line| line.contains("worker-ready")));

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn combined_logs_view_can_filter_by_service_level_stream() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::multi_service()).await?;

    run.assert_ready().await?;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let seed = t
        .api()
        .logs_view(
            run.id(),
            &LogViewQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: Some("worker-stderr".to_string()),
                    level: None,
                    stream: Some("stderr".to_string()),
                },
                service: Some("worker".to_string()),
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;
    let level = seed
        .entries
        .first()
        .expect("worker stderr entry")
        .level
        .clone();

    let view = t
        .api()
        .logs_view(
            run.id(),
            &LogViewQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: Some("worker-stderr".to_string()),
                    level: Some(level.clone()),
                    stream: Some("stderr".to_string()),
                },
                service: Some("worker".to_string()),
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;

    assert!(!view.entries.is_empty());
    assert!(view.entries.iter().all(|entry| {
        entry.service == "worker"
            && entry.stream == "stderr"
            && entry.level == level
            && entry.message.contains("worker-stderr")
    }));

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn logs_since_filters_older_entries() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let cutoff = OffsetDateTime::now_utc().format(&Rfc3339)?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let response = run.service("api").http_get("/").await?;
    assert!(response.contains("200 OK"));
    run.service("api")
        .assert_log_contains("http-access")
        .await?;

    let logs = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: Some(cutoff),
                    search: None,
                    level: None,
                    stream: None,
                },
                after: None,
            },
        )
        .await?;

    assert!(logs.lines.iter().any(|line| line.contains("http-access")));
    assert!(
        !logs
            .lines
            .iter()
            .any(|line| line.contains("service-started name=api"))
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn logs_search_returns_matching_entries() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.service("api").http_get("/").await?;
    let logs = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: Some("http-access".to_string()),
                    level: None,
                    stream: None,
                },
                after: None,
            },
        )
        .await?;

    assert!(logs.matched_total >= 1);
    assert!(logs.lines.iter().all(|line| line.contains("http-access")));

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn logs_follow_returns_incremental_updates() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let initial = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                },
                after: None,
            },
        )
        .await?;
    let cursor = initial.next_after.expect("initial log cursor");

    run.service("api").http_get("/").await?;
    let updates = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                },
                after: Some(cursor),
            },
        )
        .await?;

    assert!(
        updates
            .lines
            .iter()
            .any(|line| line.contains("http-access"))
    );
    assert!(
        !updates
            .lines
            .iter()
            .any(|line| line.contains("service-started name=api"))
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn logs_facets_returns_filter_metadata() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::multi_service()).await?;

    run.assert_ready().await?;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let _ = t
        .api()
        .logs_view(
            run.id(),
            &LogViewQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: Some("worker-stderr".to_string()),
                    level: None,
                    stream: None,
                },
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;
    let view = t
        .api()
        .logs_view(
            run.id(),
            &LogViewQuery {
                filter: LogFilterQuery {
                    last: Some(50),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                },
                service: None,
                include_entries: false,
                include_facets: true,
            },
        )
        .await?;
    let fields: Vec<_> = view
        .filters
        .iter()
        .map(|filter| filter.field.as_str())
        .collect();

    assert!(fields.contains(&"service"));
    assert!(fields.contains(&"level"));
    assert!(fields.contains(&"stream"));

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn sse_emits_run_service_task_and_log_events() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::tasks_fixture())
        .with_file(
            fixtures::TasksFixture::INPUT_FILE,
            b"hello from task\n".to_vec(),
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let events = t.events().subscribe().await?;

    let run = t
        .cli()
        .up_with(
            &project,
            UpOptions {
                no_wait: true,
                ..UpOptions::default()
            },
        )
        .await?;
    run.assert_service_ready("api").await?;
    events.assert_run_created(run.id()).await?;
    events
        .assert_service_state(run.id(), "api", devstack::model::ServiceState::Ready)
        .await?;

    let task = t
        .cli()
        .run_task_detached(&project, "copy-input", TaskStartOptions::default())
        .await?;
    events.assert_task_started(task.id()).await?;
    task.assert_completed().await?;
    events.assert_task_completed(task.id()).await?;

    let run_events = t.events().subscribe_run(run.id()).await?;
    run.service("api").http_get("/").await?;
    run_events
        .assert_log_contains(run.id(), "api", "http-access")
        .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn run_lists_available_tasks() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::tasks_fixture()).create().await?;

    let tasks = t.cli().list_tasks_json(&project).await?;
    let names = tasks["tasks"].as_array().unwrap();
    assert!(names.iter().any(|value| value == "copy-input"));
    assert!(names.iter().any(|value| value == "fail-task"));
    assert!(names.iter().any(|value| value == "chatty-task"));
    assert!(names.iter().any(|value| value == "env-task"));
    Ok(())
}

#[tokio::test]
async fn run_executes_named_task() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::tasks_fixture())
        .with_file(fixtures::TasksFixture::INPUT_FILE, b"copied\n".to_vec())
        .create()
        .await?;

    let result = t.cli().run_task_json(&project, "copy-input", &[]).await?;
    assert_eq!(result["task"], "copy-input");
    assert_eq!(result["exit_code"], 0);
    t.fs(&project)
        .assert_file_contains(fixtures::TasksFixture::OUTPUT_FILE, "copied")?;
    Ok(())
}

#[tokio::test]
async fn run_detach_returns_execution_id_and_task_status_converges() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::tasks_fixture())
        .with_file(
            fixtures::TasksFixture::INPUT_FILE,
            b"hello from task\n".to_vec(),
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;
    let task = t
        .cli()
        .run_task_detached(&project, "copy-input", TaskStartOptions::default())
        .await?;
    task.assert_completed().await?;

    let status = t.cli().task_status_json(&project, task.id()).await?;
    assert_eq!(status.state, TaskExecutionState::Completed);
    t.fs(&project)
        .assert_file_contains(fixtures::TasksFixture::OUTPUT_FILE, "hello from task")?;

    let run_tasks = t.api().run_tasks(run.id()).await?;
    assert!(run_tasks.tasks.iter().any(|entry| {
        entry.task == "copy-input" && entry.state == TaskExecutionState::Completed
    }));

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn run_init_executes_stack_init_tasks_without_starting_services() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::init_post_init()).create().await?;

    let result = t.cli().run_init_json(&project).await?;
    assert_eq!(result["mode"], "init");
    t.fs(&project)
        .assert_file_contains(fixtures::InitPostInitFixture::INIT_LOG, "initial")?;
    t.fs(&project)
        .assert_missing(fixtures::InitPostInitFixture::STARTS_LOG)?;
    t.fs(&project)
        .assert_missing(fixtures::InitPostInitFixture::POST_INIT_LOG)?;
    assert!(t.adhoc_task_history_path(&project).exists());
    Ok(())
}

#[tokio::test]
async fn run_verbose_streams_output() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::tasks_fixture()).create().await?;

    let cmd = t
        .cli()
        .run_task_verbose(&project, "chatty-task")
        .await?
        .success()?;
    cmd.assert_stdout_contains("chatty-stdout")?;
    cmd.assert_stderr_contains("chatty-stderr")?;
    Ok(())
}

mod support;

use anyhow::Result;
use devstack::manifest::RunLifecycle;
use serde_json::Value;
use support::fixtures;
use support::workflows::start_fixture_run;
use support::{TestHarness, UpOptions};

fn old_timestamp() -> String {
    "2000-01-01T00:00:00Z".to_string()
}

#[tokio::test]
async fn up_ensures_globals_and_list_globals_reports_them() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::globals_fixture()).await?;

    run.assert_ready().await?;
    let globals = t.api().list_globals().await?;
    let cache = globals
        .globals
        .iter()
        .find(|global| {
            global.project_dir == project.path().to_string_lossy() && global.name == "cache"
        })
        .expect("cache global listed");
    assert_eq!(cache.state, RunLifecycle::Running);
    assert!(cache.port.is_some());
    assert!(
        cache
            .url
            .as_deref()
            .unwrap_or_default()
            .contains("localhost")
    );
    assert!(t.global_manifest_path(&project, "cache")?.exists());

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn globals_reuse_existing_instance_when_already_active() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::globals_fixture()).await?;

    run.assert_ready().await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::GlobalsFixture::GLOBAL_STARTS_LOG, 1)
        .await?;
    let globals_before = t.api().list_globals().await?;
    let before = globals_before
        .globals
        .iter()
        .find(|global| {
            global.project_dir == project.path().to_string_lossy() && global.name == "cache"
        })
        .expect("cache global before refresh")
        .clone();

    let second = t
        .cli()
        .up_with(
            &project,
            UpOptions {
                new_run: true,
                ..UpOptions::default()
            },
        )
        .await?;
    second.assert_ready().await?;

    t.fs(&project)
        .assert_line_count_stays(
            fixtures::GlobalsFixture::GLOBAL_STARTS_LOG,
            1,
            std::time::Duration::from_secs(1),
        )
        .await?;

    let globals_after = t.api().list_globals().await?;
    let after = globals_after
        .globals
        .iter()
        .find(|global| {
            global.project_dir == project.path().to_string_lossy() && global.name == "cache"
        })
        .expect("cache global after refresh");
    assert_eq!(after.port, before.port);
    assert_eq!(after.url, before.url);

    run.down().await?;
    second.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn globals_run_post_init() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::globals_fixture()).await?;

    run.assert_ready().await?;
    t.fs(&project)
        .wait_for_file_contains(
            fixtures::GlobalsFixture::GLOBAL_POST_INIT_LOG,
            "global-post-init",
        )
        .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn gc_all_removes_stopped_globals() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::globals_fixture()).await?;

    run.assert_ready().await?;
    run.down().await?;
    let global_manifest_path = t.global_manifest_path(&project, "cache")?;
    let global_dir = t.global_dir(&project, "cache")?;
    let global_key = global_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    daemon.stop().await?;

    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&global_manifest_path)?)?;
    manifest["state"] = Value::String("stopped".to_string());
    manifest["stopped_at"] = Value::String(old_timestamp());
    std::fs::write(&global_manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    let daemon = t.daemon().start().await?;
    let response = t.api().gc(Some("1h".to_string()), true).await?;
    assert!(response.removed_globals.contains(&global_key));
    assert!(!global_dir.exists());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_touches_project_in_ledger() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let projects = t.api().list_projects().await?;
    assert!(
        projects
            .projects
            .iter()
            .any(|entry| entry.path == project.path().to_string_lossy())
    );
    assert!(t.projects_ledger_path().exists());

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn projects_add_list_remove_round_trip() -> Result<()> {
    let t = TestHarness::new().await?;
    let controller = t.fixture(fixtures::simple_http()).create().await?;
    let other = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;

    t.cli()
        .projects_add(&controller, other.path())
        .await?
        .success()?;
    let projects = t.cli().projects_list_json(&controller).await?;
    let entry = projects
        .projects
        .iter()
        .find(|project| project.path == other.path().to_string_lossy())
        .expect("project added to ledger")
        .clone();

    t.cli()
        .projects_remove(&controller, &entry.id)
        .await?
        .success()?;
    let projects = t.cli().projects_list_json(&controller).await?;
    assert!(
        !projects
            .projects
            .iter()
            .any(|project| project.path == other.path().to_string_lossy())
    );

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn sources_add_list_remove_round_trip() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;
    let source_path = project.path().join("state/external.jsonl");
    std::fs::create_dir_all(source_path.parent().unwrap())?;
    std::fs::write(
        &source_path,
        "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"msg\":\"ready\"}\n",
    )?;

    let source_arg = source_path.to_string_lossy().to_string();
    t.cli()
        .sources_add(&project, "ext", std::slice::from_ref(&source_arg))
        .await?
        .success()?;
    let sources = t.cli().sources_list_json(&project).await?;
    assert!(
        sources
            .sources
            .iter()
            .any(|source| source.name == "ext" && source.paths == vec![source_arg.clone()])
    );

    t.cli().sources_remove(&project, "ext").await?.success()?;
    let sources = t.cli().sources_list_json(&project).await?;
    assert!(!sources.sources.iter().any(|source| source.name == "ext"));

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn source_logs_can_be_queried() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;
    let source_path = project.path().join("state/external.jsonl");
    std::fs::create_dir_all(source_path.parent().unwrap())?;
    std::fs::write(
        &source_path,
        concat!(
            "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"level\":\"info\",\"msg\":\"source-ready\"}\n",
            "{\"time\":\"2025-01-01T00:00:01Z\",\"stream\":\"stderr\",\"level\":\"error\",\"msg\":\"source-boom\"}\n"
        ),
    )?;

    t.api()
        .add_source("ext", vec![source_path.to_string_lossy().to_string()])
        .await?;
    let logs = t
        .api()
        .source_logs(
            "ext",
            &devstack::api::LogViewQuery {
                last: Some(50),
                since: None,
                search: Some("source-boom".to_string()),
                level: Some("error".to_string()),
                stream: Some("stderr".to_string()),
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;

    assert_eq!(logs.entries.len(), 1);
    assert_eq!(logs.entries[0].message, "source-boom");
    assert_eq!(logs.entries[0].stream, "stderr");
    assert_eq!(logs.entries[0].level, "error");

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn removing_source_makes_it_unqueryable() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;
    let source_path = project.path().join("state/remove-source.jsonl");
    std::fs::create_dir_all(source_path.parent().unwrap())?;
    std::fs::write(
        &source_path,
        "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"msg\":\"source-live\"}\n",
    )?;

    t.api()
        .add_source("ext", vec![source_path.to_string_lossy().to_string()])
        .await?;
    let before = t
        .api()
        .source_logs(
            "ext",
            &devstack::api::LogViewQuery {
                last: Some(50),
                since: None,
                search: Some("source-live".to_string()),
                level: None,
                stream: None,
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;
    assert_eq!(before.entries.len(), 1);

    t.api().remove_source("ext").await?;
    let err = t
        .api()
        .source_logs(
            "ext",
            &devstack::api::LogViewQuery {
                last: Some(50),
                since: None,
                search: None,
                level: None,
                stream: None,
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .await
        .expect_err("removed source should not remain queryable");
    assert!(err.to_string().contains("source ext not found"));

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn readding_source_refreshes_searchable_entries() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;
    let source_path = project.path().join("state/refresh-source.jsonl");
    std::fs::create_dir_all(source_path.parent().unwrap())?;
    std::fs::write(
        &source_path,
        "{\"time\":\"2025-01-01T00:00:00Z\",\"stream\":\"stdout\",\"msg\":\"source-old\"}\n",
    )?;

    let source_arg = source_path.to_string_lossy().to_string();
    t.cli()
        .sources_add(&project, "ext", std::slice::from_ref(&source_arg))
        .await?
        .success()?;

    let old = t
        .api()
        .source_logs(
            "ext",
            &devstack::api::LogViewQuery {
                last: Some(50),
                since: None,
                search: Some("source-old".to_string()),
                level: None,
                stream: None,
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;
    assert_eq!(old.entries.len(), 1);

    std::fs::write(
        &source_path,
        "{\"time\":\"2025-01-01T00:00:01Z\",\"stream\":\"stdout\",\"msg\":\"source-new\"}\n",
    )?;
    t.cli()
        .sources_add(&project, "ext", std::slice::from_ref(&source_arg))
        .await?
        .success()?;

    let new = t
        .api()
        .source_logs(
            "ext",
            &devstack::api::LogViewQuery {
                last: Some(50),
                since: None,
                search: Some("source-new".to_string()),
                level: None,
                stream: None,
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;
    assert_eq!(new.entries.len(), 1);
    assert_eq!(new.entries[0].message, "source-new");

    let stale = t
        .api()
        .source_logs(
            "ext",
            &devstack::api::LogViewQuery {
                last: Some(50),
                since: None,
                search: Some("source-old".to_string()),
                level: None,
                stream: None,
                service: None,
                include_entries: true,
                include_facets: false,
            },
        )
        .await?;
    assert!(stale.entries.is_empty());

    daemon.stop().await?;
    Ok(())
}

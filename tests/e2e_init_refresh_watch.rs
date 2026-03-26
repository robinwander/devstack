mod support;

use std::time::Duration;

use anyhow::Result;
use devstack::manifest::ServiceState;
use support::fixtures;
use support::workflows::{latest_run_for_project, start_fixture_run};
use support::{TestHarness, UpOptions};

#[tokio::test]
async fn init_runs_before_service_start() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::init_post_init();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::InitPostInitFixture::ORDER_LOG, 3)
        .await?;

    let order = t
        .fs(&project)
        .read_text(fixtures::InitPostInitFixture::ORDER_LOG)?;
    let lines: Vec<_> = order.lines().collect();
    assert_eq!(lines[0], "init-task");
    assert_eq!(lines[1], "service-start");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn init_skips_when_watch_hash_unchanged() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::init_post_init();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::InitPostInitFixture::INIT_LOG, 1)
        .await?;

    let refreshed = t.cli().up(&project).await?;
    refreshed.assert_service_ready("api").await?;
    assert_eq!(refreshed.id(), run.id());
    t.fs(&project)
        .assert_line_count_stays(
            fixtures::InitPostInitFixture::INIT_LOG,
            1,
            Duration::from_secs(1),
        )
        .await?;

    refreshed.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn post_init_runs_after_readiness() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::init_post_init();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::InitPostInitFixture::POST_INIT_LOG, 1)
        .await?;

    let order = t
        .fs(&project)
        .read_text(fixtures::InitPostInitFixture::ORDER_LOG)?;
    let lines: Vec<_> = order.lines().collect();
    assert_eq!(lines[1], "service-start");
    assert_eq!(lines[2], "post-init");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn post_init_runs_again_on_refresh() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::init_post_init();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::InitPostInitFixture::POST_INIT_LOG, 1)
        .await?;

    let refreshed = t.cli().up(&project).await?;
    refreshed.assert_service_ready("api").await?;
    assert_eq!(refreshed.id(), run.id());
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::InitPostInitFixture::POST_INIT_LOG, 2)
        .await?;

    refreshed.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn restart_service_runs_post_init_again() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::init_post_init();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::InitPostInitFixture::POST_INIT_LOG, 1)
        .await?;

    run.service("api").restart().await?;
    run.service("api").assert_ready().await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::InitPostInitFixture::POST_INIT_LOG, 2)
        .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn failing_init_marks_service_failed_without_starting_process() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::init_post_init())
        .with_config_patch(|config| {
            config
                .task("init-task")?
                .cmd("echo init-broke >&2; exit 17");
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let run = t.cli().up(&project).await?;
    run.assert_degraded().await?;
    run.service("api").assert_failed().await?;
    t.fs(&project)
        .assert_missing(fixtures::InitPostInitFixture::STARTS_LOG)?;

    let status = run.status().await?;
    assert!(
        status.services["api"]
            .last_failure
            .as_deref()
            .unwrap_or_default()
            .contains("init task failed")
    );

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn failing_post_init_marks_service_failed() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::init_post_init())
        .with_config_patch(|config| {
            config
                .task("post-init")?
                .cmd("echo post-init-broke >&2; exit 19");
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let cmd = t
        .cli()
        .run_in(
            &project,
            &[
                "up",
                "--project",
                project.path().to_string_lossy().as_ref(),
                "--stack",
                "dev",
            ],
        )
        .await?
        .failure()?;
    cmd.assert_stderr_contains("post_init task failed")?;

    let run = latest_run_for_project(&t, &project).await?;
    run.assert_degraded().await?;
    run.service("api").assert_failed().await?;

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn refresh_removes_deleted_services() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::multi_service()).await?;

    run.assert_ready().await?;
    run.service("worker").assert_ready().await?;

    project.patch_config(|config| config.remove_service("dev", "worker"))?;
    let refreshed = t.cli().up(&project).await?;
    refreshed.assert_ready().await?;
    assert_eq!(refreshed.id(), run.id());

    let status = refreshed.status().await?;
    assert!(status.services.contains_key("api"));
    assert!(!status.services.contains_key("worker"));

    refreshed.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn force_refresh_restarts_even_without_hash_change() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    t.fs(&project)
        .wait_for_line_count_at_least("state/api-starts.log", 1)
        .await?;

    let refreshed = t
        .cli()
        .up_with(
            &project,
            UpOptions {
                force: true,
                ..UpOptions::default()
            },
        )
        .await?;
    refreshed.assert_ready().await?;
    assert_eq!(refreshed.id(), run.id());
    t.fs(&project)
        .wait_for_line_count_at_least("state/api-starts.log", 2)
        .await?;

    refreshed.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn restart_service_no_wait_eventually_returns_to_ready() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.readiness_delay_ms(1_000);
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    run.service("api").restart_no_wait().await?;
    let initial = run.service("api").status().await?;
    assert_ne!(initial.state, ServiceState::Ready);
    run.service("api").assert_ready().await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn watched_file_change_triggers_restart() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::watch_restart();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::WatchRestartFixture::STARTS_LOG, 1)
        .await?;

    fixture.touch_watched_file(&t.fs(&project))?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::WatchRestartFixture::STARTS_LOG, 2)
        .await?;
    run.service("api").assert_ready().await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_restart_restores_watch_metadata() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::watch_restart();
    let (daemon, _project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    let initial_watch = run.watch_status().await?;
    assert!(initial_watch.services["api"].auto_restart);

    let daemon = daemon.restart().await?;
    daemon.assert_ping().await?;

    let restored_watch = run.watch_status().await?;
    assert!(restored_watch.services["api"].auto_restart);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn ignored_file_change_does_not_trigger_restart() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::watch_restart();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::WatchRestartFixture::STARTS_LOG, 1)
        .await?;

    fixture.touch_ignored_file(&t.fs(&project))?;
    t.fs(&project)
        .assert_line_count_stays(
            fixtures::WatchRestartFixture::STARTS_LOG,
            1,
            Duration::from_secs(2),
        )
        .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn watch_pause_prevents_restart() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::watch_restart();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    let status = run.service("api").pause_watch().await?;
    assert!(status.services["api"].paused);
    assert!(!status.services["api"].active);

    fixture.touch_watched_file(&t.fs(&project))?;
    t.fs(&project)
        .assert_line_count_stays(
            fixtures::WatchRestartFixture::STARTS_LOG,
            1,
            Duration::from_secs(2),
        )
        .await?;

    let run_watch = run.watch_status().await?;
    assert!(run_watch.services["api"].paused);
    assert!(!run_watch.services["api"].active);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn watch_resume_restores_restart_behavior() -> Result<()> {
    let t = TestHarness::new().await?;
    let fixture = fixtures::watch_restart();
    let (daemon, project, run) = start_fixture_run(&t, fixture).await?;

    run.assert_service_ready("api").await?;
    run.service("api").pause_watch().await?;
    fixture.touch_watched_file(&t.fs(&project))?;
    t.fs(&project)
        .assert_line_count_stays(
            fixtures::WatchRestartFixture::STARTS_LOG,
            1,
            Duration::from_secs(2),
        )
        .await?;

    let status = run.service("api").resume_watch().await?;
    assert!(!status.services["api"].paused);
    assert!(status.services["api"].active);

    fixture.touch_watched_file(&t.fs(&project))?;
    t.fs(&project)
        .wait_for_line_count_at_least(fixtures::WatchRestartFixture::STARTS_LOG, 2)
        .await?;
    run.service("api").assert_ready().await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

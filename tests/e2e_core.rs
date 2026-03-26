mod support;

use anyhow::Result;
use devstack::manifest::{RunLifecycle, ServiceState};
use devstack::persistence::PersistedRun;
use support::fixtures;
use support::workflows::start_fixture_run;
use support::{TestHarness, UpOptions};

#[tokio::test]
async fn up_starts_simple_stack_and_status_reports_ready() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.service("api")
        .assert_log_contains("service-started name=api")
        .await?;
    t.fs(&project).assert_exists("state/api-starts.log")?;
    t.fs(&project)
        .assert_file_contains("state/api-starts.log", "started")?;
    assert!(run.manifest_path().exists());

    let status = t.cli().status_json(&project, run.id()).await?;
    assert_eq!(status.state, RunLifecycle::Running);
    assert_eq!(status.services["api"].state, ServiceState::Ready);

    run.down().await?;
    run.assert_stopped().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_without_new_run_refreshes_existing_run() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let refreshed = t.cli().up(&project).await?;
    refreshed.assert_ready().await?;

    assert_eq!(refreshed.id(), run.id());

    refreshed.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_with_new_run_creates_distinct_run() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let second = t
        .cli()
        .up_with(
            run.project(),
            UpOptions {
                new_run: true,
                ..UpOptions::default()
            },
        )
        .await?;
    second.assert_ready().await?;

    assert_ne!(second.id(), run.id());

    run.down().await?;
    second.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn down_stops_run_and_marks_manifest_stopped() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.down().await?;
    run.assert_stopped().await?;

    let manifest: PersistedRun =
        serde_json::from_str(&std::fs::read_to_string(run.manifest_path())?)?;
    assert_eq!(manifest.state, RunLifecycle::Stopped);
    assert!(manifest.stopped_at.is_some());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn kill_force_stops_run_and_marks_manifest_stopped() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.kill().await?;
    run.assert_stopped().await?;

    let manifest: PersistedRun =
        serde_json::from_str(&std::fs::read_to_string(run.manifest_path())?)?;
    assert_eq!(manifest.state, RunLifecycle::Stopped);
    assert!(manifest.stopped_at.is_some());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn no_wait_returns_early_and_background_readiness_converges() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.readiness_delay_ms(1_500);
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
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

    let initial = run.status().await?;
    assert_ne!(initial.services["api"].state, ServiceState::Ready);

    run.assert_service_ready("api").await?;
    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_restart_preserves_visible_run_state() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;

    let daemon = daemon.restart().await?;
    daemon.assert_ping().await?;
    run.assert_service_ready("api").await?;

    let status = run.status().await?;
    assert_eq!(status.state, RunLifecycle::Running);
    assert_eq!(status.services["api"].state, ServiceState::Ready);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn list_runs_reconciles_exited_service_state_and_persists_manifest() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'echo api-ready; sleep 1; exit 17'")
                .port_none()
                .readiness_log_regex("api-ready");
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;

    t.wait_until(
        std::time::Duration::from_secs(10),
        "run list to reconcile exited service state",
        || {
            let api = t.api();
            let run_id = run.id().to_string();
            async move {
                let runs = api.list_runs().await?;
                let state = runs
                    .runs
                    .iter()
                    .find(|candidate| candidate.run_id == run_id)
                    .map(|candidate| candidate.state.clone());
                if state == Some(RunLifecycle::Degraded) {
                    Ok(Some(()))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await?;

    let manifest: PersistedRun =
        serde_json::from_str(&std::fs::read_to_string(run.manifest_path())?)?;
    assert_eq!(manifest.state, RunLifecycle::Degraded);
    assert_eq!(manifest.services["api"].state, ServiceState::Degraded);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

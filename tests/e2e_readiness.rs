mod support;

use anyhow::Result;
use devstack::manifest::RunLifecycle;
use support::fixtures;
use support::workflows::{latest_run_for_project, start_fixture_run};
use support::{ProjectHandle, RunHandle, TestHarness};

async fn assert_up_failure_marks_service_failed(
    t: &TestHarness,
    project: &ProjectHandle,
    expected_stderr: &str,
    expected_failure: &str,
) -> Result<RunHandle> {
    let cmd = t
        .cli()
        .run_in(
            project,
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
    cmd.assert_stderr_contains(expected_stderr)?;

    let run = latest_run_for_project(t, project).await?;
    run.assert_degraded().await?;
    run.service("api").assert_failed().await?;
    assert!(
        run.status().await?.services["api"]
            .last_failure
            .as_deref()
            .unwrap_or_default()
            .contains(expected_failure)
    );
    Ok(run)
}

#[tokio::test]
async fn http_readiness_transitions_to_ready_successfully() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn tcp_readiness_transitions_to_ready_successfully() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.clear_readiness();
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn log_regex_readiness_transitions_to_ready_successfully() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::multi_service()).await?;

    run.assert_ready().await?;
    run.service("worker").assert_ready().await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn cmd_readiness_transitions_to_ready_successfully() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc '(sleep 1; touch state/cmd-ready) & trap \"exit 0\" TERM INT; while true; do sleep 1; done'")
                .port_none()
                .readiness_cmd("test -f state/cmd-ready");
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;
    t.fs(&project).assert_exists("state/cmd-ready")?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn delay_readiness_transitions_to_ready_successfully() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'trap \"exit 0\" TERM INT; while true; do sleep 1; done'")
                .port_none()
                .readiness_delay_ms(750);
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn exit_readiness_fast_exit_success_works() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'echo exit-ready'")
                .port_none()
                .readiness_exit();
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;
    assert_eq!(run.status().await?.state, RunLifecycle::Running);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn none_readiness_without_port_becomes_ready() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'trap \"exit 0\" TERM INT; while true; do sleep 1; done'")
                .port_none()
                .clear_readiness();
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn tcp_readiness_failure_marks_service_failed() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'echo tcp-pending; trap \"exit 0\" TERM INT; while true; do sleep 1; done'")
                .readiness_tcp()
                .readiness_timeout_ms(500)?;
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let run = assert_up_failure_marks_service_failed(
        &t,
        &project,
        "readiness timed out",
        "readiness timed out",
    )
    .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn http_readiness_failure_marks_service_failed() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'exec python3 -m http.server \"$PORT\"'")
                .readiness_http("/missing-health", [200, 299])
                .readiness_timeout_ms(500)?;
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let run = assert_up_failure_marks_service_failed(
        &t,
        &project,
        "readiness timed out",
        "readiness timed out",
    )
    .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn log_regex_readiness_failure_marks_service_failed() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'echo still-starting; trap \"exit 0\" TERM INT; while true; do sleep 1; done'")
                .port_none()
                .readiness_log_regex("never-ready")
                .readiness_timeout_ms(500)?;
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let run = assert_up_failure_marks_service_failed(
        &t,
        &project,
        "readiness timed out",
        "readiness timed out",
    )
    .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn cmd_readiness_failure_marks_service_failed() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'trap \"exit 0\" TERM INT; while true; do sleep 1; done'")
                .port_none()
                .readiness_cmd("test -f state/never-ready")
                .readiness_timeout_ms(500)?;
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let run = assert_up_failure_marks_service_failed(
        &t,
        &project,
        "readiness timed out",
        "readiness timed out",
    )
    .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn delay_readiness_failure_marks_service_failed() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'trap \"exit 0\" TERM INT; while true; do sleep 1; done'")
                .port_none()
                .readiness_delay_ms(1_000)
                .readiness_timeout_ms(250)?;
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let run = assert_up_failure_marks_service_failed(
        &t,
        &project,
        "readiness timed out",
        "readiness timed out",
    )
    .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn exit_readiness_failure_marks_service_failed() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'echo exit-broke >&2; exit 7'")
                .port_none()
                .readiness_exit();
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let run = assert_up_failure_marks_service_failed(
        &t,
        &project,
        "readiness timed out",
        "readiness timed out",
    )
    .await?;
    run.down().await?;

    daemon.stop().await?;
    Ok(())
}

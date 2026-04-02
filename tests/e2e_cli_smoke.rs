mod support;

use anyhow::Result;
use devstack::api::RunListResponse;
use serde_json::Value;
use support::TestHarness;
use support::fixtures;
use support::workflows::start_fixture_run;

#[tokio::test]
async fn second_daemon_instance_on_same_socket_fails_cleanly() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;

    let cmd = t.cli().run_in(&project, &["daemon"]).await?.failure()?;
    cmd.assert_stderr_contains("daemon already running")?;

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn ls_filters_runs_to_current_project_unless_all_is_used() -> Result<()> {
    let t = TestHarness::new().await?;
    let project_a = t.fixture(fixtures::simple_http()).create().await?;
    let project_b = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;

    let run_a = t.cli().up(&project_a).await?;
    let run_b = t.cli().up(&project_b).await?;
    run_a.assert_ready().await?;
    run_b.assert_ready().await?;

    let local = t.cli().run_in(&project_a, &["ls"]).await?.success()?;
    let local_runs: RunListResponse = local.stdout_json()?;
    assert!(local_runs.runs.iter().any(|run| run.run_id == run_a.id()));
    assert!(!local_runs.runs.iter().any(|run| run.run_id == run_b.id()));

    let all = t
        .cli()
        .run_in(&project_a, &["ls", "--all"])
        .await?
        .success()?;
    let all_runs: RunListResponse = all.stdout_json()?;
    assert!(all_runs.runs.iter().any(|run| run.run_id == run_a.id()));
    assert!(all_runs.runs.iter().any(|run| run.run_id == run_b.id()));

    run_a.down().await?;
    run_b.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn show_sets_navigation_intent_and_prints_dashboard_url() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let cmd = t
        .cli()
        .run_in(
            &project,
            &[
                "show",
                "--run",
                run.id(),
                "--service",
                "api",
                "--search",
                "boom",
                "--level",
                "error",
                "--stream",
                "stderr",
                "--since",
                "5m",
                "--last",
                "25",
            ],
        )
        .await?
        .success()?;
    cmd.assert_stdout_contains("Opening dashboard at http://localhost:47832")?;

    let intent = t
        .api()
        .get_navigation_intent()
        .await?
        .intent
        .expect("stored intent");
    assert_eq!(intent.run_id.as_deref(), Some(run.id()));
    assert_eq!(intent.service.as_deref(), Some("api"));
    assert_eq!(intent.search.as_deref(), Some("boom"));
    assert_eq!(intent.level.as_deref(), Some("error"));
    assert_eq!(intent.stream.as_deref(), Some("stderr"));
    assert_eq!(intent.last, Some(25));
    assert!(intent.since.is_some());

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn lint_succeeds_for_valid_config() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;

    let cmd = t.cli().run_in(&project, &["lint"]).await?.success()?;
    let lint: Value = cmd.stdout_json()?;
    assert_eq!(lint["ok"], true);
    assert_eq!(lint["default_stack"], "dev");
    assert!(
        lint["stacks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "dev")
    );
    Ok(())
}

#[tokio::test]
async fn lint_fails_for_invalid_config() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    project.patch_config(|config| {
        config.service("dev", "api")?.init(&["missing-task"]);
        Ok(())
    })?;

    let cmd = t.cli().run_in(&project, &["lint"]).await?.failure()?;
    cmd.assert_stderr_contains("unknown init task 'missing-task'")?;
    Ok(())
}

#[tokio::test]
async fn doctor_returns_health_report() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;

    let cmd = t.cli().run_in(&project, &["doctor"]).await?.success()?;
    let report: Value = cmd.stdout_json()?;
    let checks = report["checks"].as_array().expect("doctor checks array");
    assert!(
        checks
            .iter()
            .any(|check| check["name"] == "daemon_socket" && check["ok"] == true)
    );
    assert!(checks.iter().any(|check| check["name"] == "filesystem"));

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn openapi_writes_spec_file() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let out = project.path().join("state/openapi.json");
    let out_arg = out.to_string_lossy().to_string();

    let cmd = t
        .cli()
        .run_in(&project, &["openapi", "--out", &out_arg])
        .await?
        .success()?;
    cmd.assert_stdout_contains("Wrote OpenAPI spec")?;

    let spec: Value = serde_json::from_str(&std::fs::read_to_string(&out)?)?;
    assert!(spec["openapi"].is_string());
    assert!(spec["paths"].is_object());
    Ok(())
}

#[tokio::test]
async fn completions_generates_bash_script() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;

    let cmd = t
        .cli()
        .run_in(&project, &["completions", "bash"])
        .await?
        .success()?;
    cmd.assert_stdout_contains("_devstack_complete()")?;
    cmd.assert_stdout_contains(
        "complete -o bashdefault -o default -F _devstack_complete devstack",
    )?;
    Ok(())
}

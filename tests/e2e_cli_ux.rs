mod support;

use anyhow::Result;
use support::fixtures;
use support::workflows::start_fixture_run;
use support::TestHarness;

// --- Dashed template name ---

#[tokio::test]
async fn dashed_service_name_works_in_template() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.my-api]
cmd = "bash -lc 'printf \"%s\" \"$SELF_URL\" > state/url.txt; exec python3 bin/service_http.py'"

[stacks.dev.services.my-api.env]
FIXTURE_SERVICE_NAME = "my-api"
FIXTURE_STARTS_FILE = "state/api-starts.log"
SELF_URL = "{{ services.my_api.url }}"

[stacks.dev.services.my-api.readiness.http]
path = "/"
expect_status = [200, 299]
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("my-api").await?;
    let content = t.fs(&project).read_text("state/url.txt")?;
    assert!(
        content.starts_with("http://"),
        "expected rendered URL, got: {content}"
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

// --- Up with service filter ---

#[tokio::test]
async fn up_with_service_filter_starts_only_named_services() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, _run) = start_fixture_run(&t, fixtures::multi_service()).await?;

    // Stop and start fresh with only "api" (not "worker")
    let runs = t.api().list_runs().await?;
    for run in &runs.runs {
        t.api().down(&run.run_id).await?;
    }

    let result = t
        .cli()
        .run_in(
            &project,
            &[
                "up",
                "--project",
                &project.path_string(),
                "dev",
                "api",
            ],
        )
        .await?;
    let response: devstack::api::RunResponse = result.success()?.stdout_json()?;
    let run = t.run_handle(&project, &response.run_id);

    run.assert_service_ready("api").await?;
    let status = run.status().await?;
    assert!(
        status.services.contains_key("api"),
        "api should be in the run"
    );
    assert!(
        !status.services.contains_key("worker"),
        "worker should NOT be in the run"
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_with_service_filter_includes_transitive_deps() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.db]
cmd = "python3 bin/service_http.py"

[stacks.dev.services.db.env]
FIXTURE_SERVICE_NAME = "db"
FIXTURE_STARTS_FILE = "state/db-starts.log"

[stacks.dev.services.db.readiness.http]
path = "/"
expect_status = [200, 299]

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
deps = ["db"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[stacks.dev.services.worker]
cmd = "bash -lc 'printf \"started\\n\" >> state/worker-starts.log; echo worker-ready; trap \"exit 0\" TERM INT; while true; do sleep 1; done'"
port = "none"
readiness = { log_regex = "worker-ready" }
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    // Start only "api" — should also pull in "db" (its dep), but NOT "worker"
    let result = t
        .cli()
        .run_in(
            &project,
            &[
                "up",
                "--project",
                &project.path_string(),
                "dev",
                "api",
            ],
        )
        .await?;
    let response: devstack::api::RunResponse = result.success()?.stdout_json()?;
    let run = t.run_handle(&project, &response.run_id);

    run.assert_service_ready("api").await?;
    run.assert_service_ready("db").await?;
    let status = run.status().await?;
    assert!(!status.services.contains_key("worker"), "worker should NOT be started");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

// --- Logs positional arg ---

#[tokio::test]
async fn logs_positional_service_name() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;
    run.assert_service_ready("api").await?;

    // Use positional: `devstack logs api` instead of `devstack logs --service api`
    let result = t
        .cli()
        .run_in(
            &project,
            &["logs", "api", "--last", "10"],
        )
        .await?;
    let output = result.success()?;
    output.assert_stdout_contains("service-started")?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn logs_flag_service_still_works() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;
    run.assert_service_ready("api").await?;

    // --service flag should still work exactly as before
    let result = t
        .cli()
        .run_in(
            &project,
            &["logs", "--service", "api", "--last", "10"],
        )
        .await?;
    let output = result.success()?;
    output.assert_stdout_contains("service-started")?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

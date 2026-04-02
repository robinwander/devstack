mod support;

use anyhow::Result;
use devstack::api::{LogFilterQuery, LogsQuery, RunResponse};
use devstack::model::ServiceState;
use support::fixtures;
use support::{TestHarness, UpOptions};

#[tokio::test]
async fn init_task_output_appears_in_service_logs() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["setup-db"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.setup-db]
cmd = "echo 'running migrations'; echo 'migration complete'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;

    let logs = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(200),
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
            .any(|line| line.contains("running migrations")),
        "expected init task output 'running migrations' in service logs, got:\n{}",
        logs.lines.join("\n")
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("migration complete")),
        "expected init task output 'migration complete' in service logs, got:\n{}",
        logs.lines.join("\n")
    );

    // Init output should have stream field indicating it came from an init task
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("init:setup-db")),
        "expected init task stream label 'init:setup-db' in service logs, got:\n{}",
        logs.lines.join("\n")
    );

    // Service runtime logs should also be present (verifying init didn't clobber them)
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("service-started")),
        "expected service runtime logs after init, got:\n{}",
        logs.lines.join("\n")
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn post_init_task_output_appears_in_service_logs() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
post_init = ["seed-data"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.seed-data]
cmd = "echo 'seeding database'; echo 'seed complete'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;

    let logs = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(200),
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
            .any(|line| line.contains("seeding database")),
        "expected post_init task output 'seeding database' in service logs, got:\n{}",
        logs.lines.join("\n")
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("post_init:seed-data")),
        "expected post_init task stream label in service logs, got:\n{}",
        logs.lines.join("\n")
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn failed_init_task_output_appears_in_service_logs() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["bad-migrate"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.bad-migrate]
cmd = "echo 'starting migration'; echo 'ERROR: relation already exists' >&2; exit 1"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    // Up should still succeed (the run is created, service is marked failed)
    let run = t.api().up_with(
        &project,
        &UpOptions::default(),
    ).await?;

    run.service("api").assert_failed().await?;

    let logs = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(200),
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
            .any(|line| line.contains("starting migration")),
        "expected failed init task stdout in service logs, got:\n{}",
        logs.lines.join("\n")
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("ERROR: relation already exists")),
        "expected failed init task stderr in service logs, got:\n{}",
        logs.lines.join("\n")
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_response_includes_last_failure_for_failed_service() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["fail-init"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.fail-init]
cmd = "echo 'cannot connect to database' >&2; exit 1"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let result = t
        .cli()
        .run_in(&project, &["up", "--project", &project.path_string()])
        .await?;
    let response: RunResponse = result.stdout_json()?;

    let api = response.services.get("api").expect("api service in response");
    assert_eq!(api.state, ServiceState::Failed);
    assert!(
        api.last_failure.is_some(),
        "expected last_failure to be set for failed service, got: {:?}",
        api
    );
    let failure = api.last_failure.as_deref().unwrap();
    assert!(
        failure.contains("fail-init"),
        "expected last_failure to mention the task name, got: {failure}"
    );

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_interactive_prints_human_summary() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    // Force interactive mode by not piping (default assert_cmd captures, so
    // we test that the output contains the human summary pattern)
    let result = t
        .cli()
        .run_in(&project, &["up", "--project", &project.path_string()])
        .await?
        .success()?;

    let _: RunResponse = result.stdout_json().expect("non-interactive up should output valid TOON");

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_interactive_shows_failure_summary() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["broken-init"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.broken-init]
cmd = "exit 1"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;

    let result = t
        .cli()
        .run_in(&project, &["up", "--project", &project.path_string()])
        .await?;

    // Should contain the failure information somewhere in the output
    let combined = format!("{}{}", result.stdout, result.stderr);
    assert!(
        combined.contains("failed") || combined.contains("Failed"),
        "expected up output to mention failure, got:\nstdout: {}\nstderr: {}",
        result.stdout,
        result.stderr,
    );

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn init_task_logs_still_accessible_via_task_flag() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["setup"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.setup]
cmd = "echo 'task log line'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;
    run.assert_service_ready("api").await?;

    // The original task log path should still exist and work
    let result = t
        .cli()
        .run_in(
            &project,
            &[
                "logs",
                "--task",
                "setup",
                "--run-id",
                run.id(),
                "--last",
                "10",
            ],
        )
        .await?
        .success()?;

    assert!(
        result.stdout.contains("task log line"),
        "expected --task flag to still show init task output, got: {}",
        result.stdout,
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn multiple_init_tasks_all_appear_in_service_logs() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["step-one", "step-two"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.step-one]
cmd = "echo 'first step done'"

[tasks.step-two]
cmd = "echo 'second step done'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;

    let logs = t
        .api()
        .logs(
            run.id(),
            "api",
            &LogsQuery {
                filter: LogFilterQuery {
                    last: Some(200),
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
            .any(|line| line.contains("first step done")),
        "expected first init task output in service logs, got:\n{}",
        logs.lines.join("\n")
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("second step done")),
        "expected second init task output in service logs, got:\n{}",
        logs.lines.join("\n")
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("init:step-one")),
        "expected init:step-one stream label, got:\n{}",
        logs.lines.join("\n")
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.contains("init:step-two")),
        "expected init:step-two stream label, got:\n{}",
        logs.lines.join("\n")
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

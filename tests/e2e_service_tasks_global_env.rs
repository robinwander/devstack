mod support;

use anyhow::Result;
use support::TestHarness;
use support::fixtures;

#[tokio::test]
async fn global_env_flows_to_service() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[env]
GLOBAL_MARKER = "from-global"

[stacks.dev.services.api]
cmd = "bash -lc 'printf \"%s\" \"$GLOBAL_MARKER\" > state/env-result.txt; exec python3 bin/service_http.py'"

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
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_file_contains("state/env-result.txt", "from-global")
        .await?;
    let content = t.fs(&project).read_text("state/env-result.txt")?;
    assert_eq!(content, "from-global");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn service_env_overrides_global_env() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[env]
SHARED_VAR = "global-value"

[stacks.dev.services.api]
cmd = "bash -lc 'printf \"%s\" \"$SHARED_VAR\" > state/env-result.txt; exec python3 bin/service_http.py'"

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"
SHARED_VAR = "service-value"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_file_contains("state/env-result.txt", "service-value")
        .await?;
    let content = t.fs(&project).read_text("state/env-result.txt")?;
    assert_eq!(content, "service-value");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn global_env_file_is_loaded_for_services() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text("shared.env", "ENV_FILE_VAR=from-env-file\n")
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"
env_file = "shared.env"

[stacks.dev.services.api]
cmd = "bash -lc 'printf \"%s\" \"$ENV_FILE_VAR\" > state/env-result.txt; exec python3 bin/service_http.py'"

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
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_file_contains("state/env-result.txt", "from-env-file")
        .await?;
    let content = t.fs(&project).read_text("state/env-result.txt")?;
    assert_eq!(content, "from-env-file");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn init_task_receives_service_env() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[env]
INIT_MARKER = "global-env-value"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["check-env"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"
SERVICE_MARKER = "service-env-value"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.check-env]
cmd = "bash -lc 'printf \"%s:%s\" \"$INIT_MARKER\" \"$SERVICE_MARKER\" > state/init-env.txt'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    let content = t.fs(&project).read_text("state/init-env.txt")?;
    assert_eq!(content, "global-env-value:service-env-value");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn post_init_task_receives_service_env() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[env]
POST_INIT_MARKER = "from-global"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
post_init = ["check-post-env"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"
SVC_POST_MARKER = "from-service"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.check-post-env]
cmd = "bash -lc 'printf \"%s:%s\" \"$POST_INIT_MARKER\" \"$SVC_POST_MARKER\" > state/post-init-env.txt'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    t.fs(&project)
        .wait_for_file_contains("state/post-init-env.txt", "from-global:from-service")
        .await?;
    let content = t.fs(&project).read_text("state/post-init-env.txt")?;
    assert_eq!(content, "from-global:from-service");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn service_scoped_task_inherits_service_env() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_text(
            "devstack.toml",
            r#"
version = 1
default_stack = "dev"

[env]
GLOBAL_VAR = "global-val"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
init = ["svc-init"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"
SVC_VAR = "svc-val"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[stacks.dev.services.api.tasks.svc-init]
cmd = "bash -lc 'printf \"%s:%s\" \"$GLOBAL_VAR\" \"$SVC_VAR\" > state/svc-task-env.txt'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    let content = t.fs(&project).read_text("state/svc-task-env.txt")?;
    assert_eq!(content, "global-val:svc-val");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn service_task_shadows_global_task() -> Result<()> {
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

[stacks.dev.services.api.tasks.setup]
cmd = "bash -lc 'printf service-version > state/which-task.txt'"

[tasks.setup]
cmd = "bash -lc 'printf global-version > state/which-task.txt'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    let content = t.fs(&project).read_text("state/which-task.txt")?;
    assert_eq!(content, "service-version");

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn init_task_receives_dev_url_vars() -> Result<()> {
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
init = ["dump-urls"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.dump-urls]
cmd = "bash -lc 'printf \"%s\" \"$DEV_URL_API\" > state/dev-url.txt'"
"#,
        )
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;
    let content = t.fs(&project).read_text("state/dev-url.txt")?;
    assert!(
        content.starts_with("http://"),
        "expected DEV_URL_API to start with http://, got: {content}"
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

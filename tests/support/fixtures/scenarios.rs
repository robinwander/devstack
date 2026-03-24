use anyhow::Result;

use crate::support::harness::FsHandle;

use super::{FixtureSpec, RenderedFixture, base_http_fixture_toml};

#[derive(Clone, Copy, Debug, Default)]
pub struct SimpleHttpFixture;

impl FixtureSpec for SimpleHttpFixture {
    fn name(&self) -> &'static str {
        "simple_http"
    }

    fn render(&self) -> Result<RenderedFixture> {
        Ok(RenderedFixture::default()
            .text("devstack.toml", base_http_fixture_toml(false, false))
            .text("src/watched.txt", "initial\n")
            .text("ignored/skip.txt", "initial\n"))
    }
}

pub fn simple_http() -> SimpleHttpFixture {
    SimpleHttpFixture
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InitPostInitFixture;

impl InitPostInitFixture {
    pub const ORDER_LOG: &'static str = "state/order.log";
    pub const INIT_LOG: &'static str = "state/init.log";
    pub const POST_INIT_LOG: &'static str = "state/post-init.log";
    pub const STARTS_LOG: &'static str = "state/api-starts.log";
    pub const WATCH_FILE: &'static str = "src/watched.txt";

    pub fn touch_watched_file(&self, fs: &FsHandle) -> Result<()> {
        fs.append_text(Self::WATCH_FILE, "changed\n")
    }
}

impl FixtureSpec for InitPostInitFixture {
    fn name(&self) -> &'static str {
        "init_post_init"
    }

    fn render(&self) -> Result<RenderedFixture> {
        Ok(RenderedFixture::default()
            .text(
                "devstack.toml",
                format!(
                    r#"version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "bash -lc 'printf \"service-start\\n\" >> {order}; exec python3 bin/service_http.py'"
init = ["init-task"]
post_init = ["post-init"]

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "{starts}"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.init-task]
cmd = "bash -lc 'cat {watch} >> {init_log}; printf \"init-task\\n\" >> {order}'"
watch = ["{watch}"]

[tasks.post-init]
cmd = "bash -lc 'bin/append-marker.sh {post_log} post-init && bin/append-marker.sh {order} post-init'"
"#,
                    order = Self::ORDER_LOG,
                    starts = Self::STARTS_LOG,
                    watch = Self::WATCH_FILE,
                    init_log = Self::INIT_LOG,
                    post_log = Self::POST_INIT_LOG,
                ),
            )
            .text(Self::WATCH_FILE, "initial\n"))
    }
}

pub fn init_post_init() -> InitPostInitFixture {
    InitPostInitFixture
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WatchRestartFixture;

impl WatchRestartFixture {
    pub const STARTS_LOG: &'static str = "state/api-starts.log";
    pub const WATCHED_FILE: &'static str = "src/watched.txt";
    pub const IGNORED_FILE: &'static str = "ignored/skip.txt";

    pub fn touch_watched_file(&self, fs: &FsHandle) -> Result<()> {
        fs.append_text(Self::WATCHED_FILE, "changed\n")
    }

    pub fn touch_ignored_file(&self, fs: &FsHandle) -> Result<()> {
        fs.append_text(Self::IGNORED_FILE, "changed\n")
    }
}

impl FixtureSpec for WatchRestartFixture {
    fn name(&self) -> &'static str {
        "watch_restart"
    }

    fn render(&self) -> Result<RenderedFixture> {
        Ok(RenderedFixture::default()
            .text("devstack.toml", base_http_fixture_toml(true, false))
            .text(Self::WATCHED_FILE, "initial\n")
            .text(Self::IGNORED_FILE, "initial\n"))
    }
}

pub fn watch_restart() -> WatchRestartFixture {
    WatchRestartFixture
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TasksFixture;

impl TasksFixture {
    pub const INPUT_FILE: &'static str = "state/input.txt";
    pub const OUTPUT_FILE: &'static str = "state/task-output.txt";
    pub const ENV_OUTPUT_FILE: &'static str = "state/env-task.txt";
}

impl FixtureSpec for TasksFixture {
    fn name(&self) -> &'static str {
        "tasks"
    }

    fn render(&self) -> Result<RenderedFixture> {
        Ok(RenderedFixture::default()
            .text(
                "devstack.toml",
                format!(
                    r#"version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.copy-input]
cmd = "sleep 1; cp {input} {output}"

[tasks.fail-task]
cmd = "echo task-failed >&2; exit 7"

[tasks.chatty-task]
cmd = "bash -lc 'echo chatty-stdout; echo warn: chatty-stderr >&2'"

[tasks.env-task]
cmd = "bash -lc 'printf %s \"$SPECIAL_VALUE\" > {env_output}'"
"#,
                    input = Self::INPUT_FILE,
                    output = Self::OUTPUT_FILE,
                    env_output = Self::ENV_OUTPUT_FILE,
                ),
            )
            .text("src/watched.txt", "initial\n"))
    }
}

pub fn tasks_fixture() -> TasksFixture {
    TasksFixture
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MultiServiceFixture;

impl MultiServiceFixture {
    pub const API_STARTS_LOG: &'static str = "state/api-starts.log";
    pub const WORKER_STARTS_LOG: &'static str = "state/worker-starts.log";
}

impl FixtureSpec for MultiServiceFixture {
    fn name(&self) -> &'static str {
        "multi_service"
    }

    fn render(&self) -> Result<RenderedFixture> {
        Ok(RenderedFixture::default().text(
            "devstack.toml",
            format!(
                r#"version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "{api_starts}"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[stacks.dev.services.worker]
cmd = "bash -lc 'printf \"started\\n\" >> {worker_starts}; echo worker-ready; echo warn: worker-stderr >&2; trap \"exit 0\" TERM INT; while true; do sleep 1; done'"
port = "none"
readiness = {{ log_regex = "worker-ready" }}
"#,
                api_starts = Self::API_STARTS_LOG,
                worker_starts = Self::WORKER_STARTS_LOG,
            ),
        ))
    }
}

pub fn multi_service() -> MultiServiceFixture {
    MultiServiceFixture
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GlobalsFixture;

impl GlobalsFixture {
    pub const GLOBAL_POST_INIT_LOG: &'static str = "state/global-post-init.log";
    pub const GLOBAL_STARTS_LOG: &'static str = "state/global-starts.log";
}

impl FixtureSpec for GlobalsFixture {
    fn name(&self) -> &'static str {
        "globals"
    }

    fn render(&self) -> Result<RenderedFixture> {
        Ok(RenderedFixture::default().text(
            "devstack.toml",
            format!(
                r#"version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]

[globals.cache]
cmd = "python3 bin/service_http.py"
post_init = ["seed-global"]

[globals.cache.env]
FIXTURE_SERVICE_NAME = "cache"
FIXTURE_STARTS_FILE = "{starts}"

[globals.cache.readiness.http]
path = "/"
expect_status = [200, 299]

[tasks.seed-global]
cmd = "bin/append-marker.sh {post_log} global-post-init"
"#,
                starts = Self::GLOBAL_STARTS_LOG,
                post_log = Self::GLOBAL_POST_INIT_LOG,
            ),
        ))
    }
}

pub fn globals_fixture() -> GlobalsFixture {
    GlobalsFixture
}

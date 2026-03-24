use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use assert_cmd::Command;
use devstack::api::LogsQuery;
use tokio::time::{Instant, sleep};

use super::{
    ApiHandle, CliHandle, DaemonController, EventsHandle, FsHandle, HarnessInner, NEXT_ID,
    POLL_INTERVAL, copy_fixture_bin_scripts, tail_lines, write_rendered_fixture,
};
use crate::support::fixtures::{FixtureConfig, FixtureSpec, RenderedFixture};

#[derive(Clone, Debug)]
pub struct UpOptions {
    pub stack: String,
    pub run_id: Option<String>,
    pub no_wait: bool,
    pub new_run: bool,
    pub force: bool,
}

impl Default for UpOptions {
    fn default() -> Self {
        Self {
            stack: "dev".to_string(),
            run_id: None,
            no_wait: false,
            new_run: false,
            force: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TaskStartOptions {
    pub args: Vec<String>,
}

#[derive(Clone)]
pub struct TestHarness {
    pub(super) inner: Arc<HarnessInner>,
}

impl TestHarness {
    pub async fn new() -> Result<Self> {
        let root = tempfile::tempdir().context("create temp test root")?;
        let home = root.path().join("home");
        let xdg_data_home = root.path().join("xdg-data");
        let xdg_config_home = root.path().join("xdg-config");
        let xdg_runtime_dir = root.path().join("xdg-runtime");
        let workspace = root.path().join("workspace");

        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(&xdg_data_home)?;
        std::fs::create_dir_all(&xdg_config_home)?;
        std::fs::create_dir_all(&xdg_runtime_dir)?;
        std::fs::create_dir_all(&workspace)?;

        let bin = assert_cmd::cargo::cargo_bin("devstack");

        Ok(Self {
            inner: Arc::new(HarnessInner {
                _root: root,
                home,
                xdg_data_home,
                xdg_config_home,
                xdg_runtime_dir,
                workspace,
                bin,
                daemon_log_path: std::sync::Mutex::new(None),
            }),
        })
    }

    pub fn fixture<F: FixtureSpec>(&self, fixture: F) -> FixtureBuilder {
        FixtureBuilder {
            harness: self.clone(),
            name: fixture.name().to_string(),
            rendered: fixture.render().expect("render fixture"),
        }
    }

    pub fn daemon(&self) -> DaemonController {
        DaemonController {
            harness: self.clone(),
        }
    }

    pub fn cli(&self) -> CliHandle {
        CliHandle {
            harness: self.clone(),
        }
    }

    pub fn api(&self) -> ApiHandle {
        ApiHandle {
            harness: self.clone(),
        }
    }

    pub fn events(&self) -> EventsHandle {
        EventsHandle {
            harness: self.clone(),
        }
    }

    pub fn fs(&self, project: &ProjectHandle) -> FsHandle {
        FsHandle {
            harness: self.clone(),
            root: project.root.clone(),
        }
    }

    pub fn run_handle(
        &self,
        project: &ProjectHandle,
        run_id: impl Into<String>,
    ) -> super::RunHandle {
        super::RunHandle::new(self.clone(), project.clone(), run_id.into())
    }

    fn child_envs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("HOME", self.inner.home.to_string_lossy().to_string()),
            (
                "XDG_DATA_HOME",
                self.inner.xdg_data_home.to_string_lossy().to_string(),
            ),
            (
                "XDG_CONFIG_HOME",
                self.inner.xdg_config_home.to_string_lossy().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR",
                self.inner.xdg_runtime_dir.to_string_lossy().to_string(),
            ),
            ("DEVSTACK_PROCESS_MANAGER", "local".to_string()),
            ("DEVSTACK_DISABLE_DASHBOARD", "1".to_string()),
            ("NO_COLOR", "1".to_string()),
        ]
    }

    pub(crate) fn apply_child_env_assert(&self, cmd: &mut Command) {
        for (key, value) in self.child_envs() {
            cmd.env(key, value);
        }
    }

    pub(crate) fn apply_child_env_tokio(&self, cmd: &mut tokio::process::Command) {
        for (key, value) in self.child_envs() {
            cmd.env(key, value);
        }
    }

    pub fn base_dir(&self) -> PathBuf {
        self.inner.xdg_data_home.join("devstack")
    }

    pub fn daemon_socket_path(&self) -> PathBuf {
        self.base_dir().join("daemon").join("devstackd.sock")
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.base_dir().join("runs").join(run_id)
    }

    pub fn run_manifest_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("manifest.json")
    }

    pub fn daemon_log_path(&self) -> Option<PathBuf> {
        self.inner
            .daemon_log_path
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    pub fn projects_ledger_path(&self) -> PathBuf {
        self.base_dir().join("projects.json")
    }

    pub fn sources_ledger_path(&self) -> PathBuf {
        self.base_dir().join("sources.json")
    }

    pub fn task_logs_dir_for_run(&self, run_id: &str) -> PathBuf {
        self.base_dir().join("task-logs").join(run_id)
    }

    pub fn task_history_path_for_run(&self, run_id: &str) -> PathBuf {
        self.task_logs_dir_for_run(run_id).join("history.json")
    }

    pub fn adhoc_task_logs_dir(&self, project: &ProjectHandle) -> PathBuf {
        self.base_dir().join("task-logs").join(format!(
            "adhoc-{}",
            devstack::util::project_hash(project.path())
        ))
    }

    pub fn adhoc_task_history_path(&self, project: &ProjectHandle) -> PathBuf {
        self.adhoc_task_logs_dir(project).join("history.json")
    }

    pub fn global_dir(&self, project: &ProjectHandle, name: &str) -> Result<PathBuf> {
        Ok(self
            .base_dir()
            .join("globals")
            .join(devstack::paths::global_key(project.path(), name)?))
    }

    pub fn global_manifest_path(&self, project: &ProjectHandle, name: &str) -> Result<PathBuf> {
        Ok(self.global_dir(project, name)?.join("manifest.json"))
    }

    pub(crate) async fn wait_until<T, F, Fut>(
        &self,
        timeout: Duration,
        description: impl Into<String>,
        mut check: F,
    ) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Option<T>>>,
    {
        let deadline = Instant::now() + timeout;
        let description = description.into();
        let mut last_error: Option<anyhow::Error> = None;

        loop {
            match check().await {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => {}
                Err(err) => last_error = Some(err),
            }

            if Instant::now() >= deadline {
                let mut message = format!("timed out waiting for {description} after {timeout:?}");
                if let Some(err) = last_error {
                    let _ = write!(message, "\n\nlast error: {err:#}");
                }
                return Err(anyhow!(message));
            }

            sleep(POLL_INTERVAL).await;
        }
    }

    pub(crate) async fn diagnostics(&self, run_id: Option<&str>, service: Option<&str>) -> String {
        let mut out = String::new();

        if let Some(run_id) = run_id {
            let _ = writeln!(out, "run_id: {run_id}");
            let manifest_path = self.run_manifest_path(run_id);
            let _ = writeln!(out, "manifest_path: {}", manifest_path.display());
            if let Ok(manifest) = std::fs::read_to_string(&manifest_path) {
                let _ = writeln!(out, "manifest_contents:\n{manifest}");
            }

            if let Ok(status) = self.api().status(run_id).await
                && let Ok(json) = serde_json::to_string_pretty(&status)
            {
                let _ = writeln!(out, "status:\n{json}");
            }

            if let Some(service) = service {
                let query = LogsQuery {
                    last: Some(20),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                    after: None,
                };
                if let Ok(logs) = self.api().logs(run_id, service, &query).await {
                    let _ = writeln!(out, "recent_logs:");
                    for line in logs.lines {
                        let _ = writeln!(out, "  {line}");
                    }
                }
            }
        }

        if let Some(log_path) = self.daemon_log_path()
            && let Ok(log) = std::fs::read_to_string(&log_path)
        {
            let _ = writeln!(out, "daemon_log_path: {}", log_path.display());
            let _ = writeln!(out, "daemon_log_tail:\n{}", tail_lines(&log, 80));
        }

        out
    }
}

pub struct FixtureBuilder {
    harness: TestHarness,
    name: String,
    rendered: RenderedFixture,
}

impl FixtureBuilder {
    pub fn with_text(mut self, path: impl AsRef<Path>, contents: impl AsRef<str>) -> Self {
        self.rendered = self.rendered.text(path, contents);
        self
    }

    pub fn with_file(mut self, path: impl AsRef<Path>, contents: impl Into<Vec<u8>>) -> Self {
        self.rendered = self.rendered.bytes(path, contents);
        self
    }

    pub fn with_config_patch(
        mut self,
        patch: impl FnOnce(&mut FixtureConfig) -> Result<()>,
    ) -> Result<Self> {
        let key = PathBuf::from("devstack.toml");
        let bytes = self
            .rendered
            .files
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow!("fixture {} has no devstack.toml", self.name))?;
        let input = String::from_utf8(bytes).context("fixture devstack.toml is not utf-8")?;
        let mut config = FixtureConfig::parse_toml(&input)?;
        patch(&mut config)?;
        let output = config.to_toml_string()?;
        self.rendered.files.insert(key, output.into_bytes());
        Ok(self)
    }

    pub async fn create(self) -> Result<ProjectHandle> {
        let project_name = format!(
            "{}-{}-{}",
            self.name,
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        );
        let root = self.harness.inner.workspace.join(project_name);
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(root.join("state"))?;
        copy_fixture_bin_scripts(&root)?;
        write_rendered_fixture(&root, self.rendered)?;
        Ok(ProjectHandle { root })
    }
}

#[derive(Clone)]
pub struct ProjectHandle {
    root: PathBuf,
}

impl ProjectHandle {
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("devstack.toml")
    }

    pub(crate) fn path_string(&self) -> String {
        self.root.to_string_lossy().to_string()
    }

    pub fn patch_config(&self, patch: impl FnOnce(&mut FixtureConfig) -> Result<()>) -> Result<()> {
        let path = self.config_path();
        let input =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut config = FixtureConfig::parse_toml(&input)?;
        patch(&mut config)?;
        let output = config.to_toml_string()?;
        std::fs::write(&path, output).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }
}

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use devstack::api::{
    LogsQuery, RunResponse, RunStatusResponse, RunWatchResponse, TaskExecutionState,
    TaskStatusResponse,
};
use devstack::model::{RunLifecycle, ServiceState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Instant, sleep};

use super::{DEFAULT_TIMEOUT, POLL_INTERVAL, ProjectHandle, TestHarness, tail_lines};

#[derive(Clone)]
pub struct RunHandle {
    harness: TestHarness,
    project: ProjectHandle,
    run_id: String,
}

impl RunHandle {
    pub(crate) fn new(harness: TestHarness, project: ProjectHandle, run_id: String) -> Self {
        Self {
            harness,
            project,
            run_id,
        }
    }

    pub fn id(&self) -> &str {
        &self.run_id
    }

    pub fn project(&self) -> &ProjectHandle {
        &self.project
    }

    pub fn service(&self, name: &str) -> ServiceHandle {
        ServiceHandle {
            run: self.clone(),
            name: name.to_string(),
        }
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.harness.run_manifest_path(&self.run_id)
    }

    pub async fn status(&self) -> Result<RunStatusResponse> {
        self.harness.api().status(&self.run_id).await
    }

    pub async fn assert_ready(&self) -> Result<()> {
        self.assert_state(RunLifecycle::Running).await?;
        let status = self.status().await?;
        for service in status.services.keys() {
            self.service(service).assert_ready().await?;
        }
        Ok(())
    }

    pub async fn assert_state(&self, expected: RunLifecycle) -> Result<()> {
        match self
            .harness
            .wait_until(
                DEFAULT_TIMEOUT,
                format!("run {} to reach state {:?}", self.run_id, expected),
                || {
                    let run = self.clone();
                    let expected = expected.clone();
                    async move {
                        let status = run.status().await?;
                        if status.state == expected {
                            Ok(Some(()))
                        } else {
                            Ok(None)
                        }
                    }
                },
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let diagnostics = self.harness.diagnostics(Some(&self.run_id), None).await;
                Err(err.context(diagnostics))
            }
        }
    }

    pub async fn assert_stopped(&self) -> Result<()> {
        self.assert_state(RunLifecycle::Stopped).await
    }

    pub async fn assert_degraded(&self) -> Result<()> {
        self.assert_state(RunLifecycle::Degraded).await
    }

    pub async fn assert_service_ready(&self, service: &str) -> Result<()> {
        self.service(service).assert_ready().await
    }

    pub async fn watch_status(&self) -> Result<RunWatchResponse> {
        self.harness.api().watch_status(&self.run_id).await
    }

    pub async fn down(&self) -> Result<RunResponse> {
        self.harness.api().down(&self.run_id).await
    }

    pub async fn kill(&self) -> Result<RunResponse> {
        self.harness.api().kill(&self.run_id).await
    }
}

#[derive(Clone)]
pub struct ServiceHandle {
    run: RunHandle,
    name: String,
}

impl ServiceHandle {
    pub async fn status(&self) -> Result<devstack::api::ServiceStatus> {
        let status = self.run.status().await?;
        status
            .services
            .get(&self.name)
            .cloned()
            .ok_or_else(|| anyhow!("service {} missing from status", self.name))
    }

    pub async fn url(&self) -> Result<String> {
        let status = self.status().await?;
        status
            .url
            .ok_or_else(|| anyhow!("service {} has no url", self.name))
    }

    pub async fn http_get(&self, path: &str) -> Result<String> {
        let url = self.url().await?;
        let prefix = "http://localhost:";
        let port = url
            .strip_prefix(prefix)
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| anyhow!("unsupported service url {url}"))?
            .parse::<u16>()
            .with_context(|| format!("parse service port from {url}"))?;

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let request_path = if path.is_empty() { "/" } else { path };
        let request =
            format!("GET {request_path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        Ok(String::from_utf8_lossy(&response).to_string())
    }

    pub async fn assert_state(&self, expected: ServiceState) -> Result<()> {
        match self
            .run
            .harness
            .wait_until(
                DEFAULT_TIMEOUT,
                format!(
                    "service {} in run {} to reach state {:?}",
                    self.name, self.run.run_id, expected
                ),
                || {
                    let service = self.clone();
                    let expected = expected.clone();
                    async move {
                        let status = service.status().await?;
                        if status.state == expected {
                            Ok(Some(()))
                        } else {
                            Ok(None)
                        }
                    }
                },
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let diagnostics = self
                    .run
                    .harness
                    .diagnostics(Some(&self.run.run_id), Some(&self.name))
                    .await;
                Err(err.context(diagnostics))
            }
        }
    }

    pub async fn assert_ready(&self) -> Result<()> {
        self.assert_state(ServiceState::Ready).await
    }

    pub async fn assert_failed(&self) -> Result<()> {
        self.assert_state(ServiceState::Failed).await
    }

    pub async fn assert_degraded(&self) -> Result<()> {
        self.assert_state(ServiceState::Degraded).await
    }

    pub async fn assert_log_contains(&self, needle: &str) -> Result<()> {
        let needle = needle.to_string();
        self.assert_log_predicate(format!("contain {needle:?}"), move |line| {
            line.contains(&needle)
        })
        .await
    }

    pub async fn assert_log_not_contains(&self, needle: &str, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let logs = self
                .run
                .harness
                .api()
                .logs(
                    &self.run.run_id,
                    &self.name,
                    &LogsQuery {
                        last: Some(200),
                        since: None,
                        search: None,
                        level: None,
                        stream: None,
                        after: None,
                    },
                )
                .await?;
            if logs.lines.iter().any(|line| line.contains(needle)) {
                let diagnostics = self
                    .run
                    .harness
                    .diagnostics(Some(&self.run.run_id), Some(&self.name))
                    .await;
                return Err(anyhow!(
                    "expected logs for {} in run {} not to contain {:?} for {duration:?}\n{}",
                    self.name,
                    self.run.run_id,
                    needle,
                    diagnostics,
                ));
            }
            sleep(POLL_INTERVAL).await;
        }
        Ok(())
    }

    async fn assert_log_predicate(
        &self,
        description: String,
        predicate: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Result<()> {
        let predicate = Arc::new(predicate);
        match self
            .run
            .harness
            .wait_until(
                DEFAULT_TIMEOUT,
                format!(
                    "logs for {} in run {} to {description}",
                    self.name, self.run.run_id
                ),
                || {
                    let api = self.run.harness.api();
                    let run_id = self.run.run_id.clone();
                    let service = self.name.clone();
                    let predicate = predicate.clone();
                    async move {
                        let logs = api
                            .logs(
                                &run_id,
                                &service,
                                &LogsQuery {
                                    last: Some(200),
                                    since: None,
                                    search: None,
                                    level: None,
                                    stream: None,
                                    after: None,
                                },
                            )
                            .await?;
                        if logs.lines.iter().any(|line| predicate(line)) {
                            Ok(Some(()))
                        } else {
                            Ok(None)
                        }
                    }
                },
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let diagnostics = self
                    .run
                    .harness
                    .diagnostics(Some(&self.run.run_id), Some(&self.name))
                    .await;
                Err(err.context(diagnostics))
            }
        }
    }

    pub async fn restart(&self) -> Result<()> {
        self.run
            .harness
            .api()
            .restart_service(&self.run.run_id, &self.name)
            .await?;
        Ok(())
    }

    pub async fn restart_no_wait(&self) -> Result<()> {
        self.run
            .harness
            .api()
            .restart_service_with_options(&self.run.run_id, &self.name, true)
            .await?;
        Ok(())
    }

    pub async fn pause_watch(&self) -> Result<RunWatchResponse> {
        self.run
            .harness
            .api()
            .watch_pause(&self.run.run_id, Some(&self.name))
            .await
    }

    pub async fn resume_watch(&self) -> Result<RunWatchResponse> {
        self.run
            .harness
            .api()
            .watch_resume(&self.run.run_id, Some(&self.name))
            .await
    }
}

#[derive(Clone)]
pub struct TaskHandle {
    harness: TestHarness,
    project: ProjectHandle,
    execution_id: String,
    task_name: String,
    run_id: Option<String>,
}

impl TaskHandle {
    pub(crate) fn new(
        harness: TestHarness,
        project: ProjectHandle,
        execution_id: String,
        task_name: String,
        run_id: Option<String>,
    ) -> Self {
        Self {
            harness,
            project,
            execution_id,
            task_name,
            run_id,
        }
    }

    pub fn id(&self) -> &str {
        &self.execution_id
    }

    pub fn task_name(&self) -> &str {
        &self.task_name
    }

    pub async fn status(&self) -> Result<TaskStatusResponse> {
        self.harness.api().task_status(&self.execution_id).await
    }

    pub async fn assert_state(&self, expected: TaskExecutionState) -> Result<TaskStatusResponse> {
        let deadline = Instant::now() + DEFAULT_TIMEOUT;

        loop {
            let status = self.status().await?;
            match (&expected, &status.state) {
                (TaskExecutionState::Completed, TaskExecutionState::Completed)
                | (TaskExecutionState::Failed, TaskExecutionState::Failed)
                | (TaskExecutionState::Running, TaskExecutionState::Running) => return Ok(status),
                (_, TaskExecutionState::Failed) if expected != TaskExecutionState::Failed => {
                    let diagnostics = self.diagnostics().await;
                    return Err(anyhow!(
                        "task {} [{}] failed unexpectedly\n{}",
                        self.task_name,
                        self.execution_id,
                        diagnostics,
                    ));
                }
                _ => {}
            }

            if Instant::now() >= deadline {
                let diagnostics = self.diagnostics().await;
                let mut message = format!(
                    "timed out waiting for task {} [{}] to reach state {:?}",
                    self.task_name, self.execution_id, expected
                );
                let _ = write!(message, "\n{diagnostics}");
                return Err(anyhow!(message));
            }

            sleep(POLL_INTERVAL).await;
        }
    }

    pub async fn assert_completed(&self) -> Result<TaskStatusResponse> {
        self.assert_state(TaskExecutionState::Completed).await
    }

    pub async fn assert_failed(&self) -> Result<TaskStatusResponse> {
        self.assert_state(TaskExecutionState::Failed).await
    }

    async fn diagnostics(&self) -> String {
        let mut out = String::new();
        if let Ok(status) = self.status().await
            && let Ok(json) = serde_json::to_string_pretty(&status)
        {
            let _ = writeln!(out, "task_status:\n{json}");
        }
        let _ = writeln!(out, "task_execution_id: {}", self.execution_id);
        let _ = writeln!(out, "task_name: {}", self.task_name);
        if let Some(run_id) = self.run_id.as_deref() {
            let extra = self.harness.diagnostics(Some(run_id), None).await;
            out.push_str(&extra);
        } else if let Some(log_path) = self.harness.daemon_log_path()
            && let Ok(log) = std::fs::read_to_string(log_path)
        {
            let _ = writeln!(out, "daemon_log_tail:\n{}", tail_lines(&log, 80));
        }
        out
    }
}

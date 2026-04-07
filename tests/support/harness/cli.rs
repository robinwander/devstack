use std::path::Path;
use std::process::ExitStatus;

use anyhow::{Context, Result, anyhow};
use assert_cmd::Command;
use devstack::api::{
    LogsResponse, ProjectsResponse, RunResponse, RunStatusResponse, RunWatchResponse,
    SourcesResponse, StartTaskResponse, TaskStatusResponse,
};
use serde::de::DeserializeOwned;

use super::{ProjectHandle, RunHandle, TaskHandle, TaskStartOptions, TestHarness, UpOptions};

#[derive(Clone)]
pub struct CliHandle {
    pub(super) harness: TestHarness,
}

impl CliHandle {
    pub async fn run_in(&self, project: &ProjectHandle, args: &[&str]) -> Result<CmdResult> {
        let mut cmd = Command::new(&self.harness.inner.bin);
        cmd.current_dir(project.path());
        self.harness.apply_child_env_assert(&mut cmd);
        cmd.args(args);
        let output = cmd
            .output()
            .with_context(|| format!("run cli command {args:?}"))?;
        Ok(CmdResult::new(
            args,
            output.status,
            output.stdout,
            output.stderr,
        ))
    }

    pub async fn up(&self, project: &ProjectHandle) -> Result<RunHandle> {
        self.up_with(project, UpOptions::default()).await
    }

    pub async fn up_with(&self, project: &ProjectHandle, options: UpOptions) -> Result<RunHandle> {
        let mut args = vec![
            "up".to_string(),
            "--project".to_string(),
            project.path_string(),
            "--stack".to_string(),
            options.stack.clone(),
        ];
        if let Some(run_id) = options.run_id.as_deref() {
            args.push("--run-id".to_string());
            args.push(run_id.to_string());
        }
        if options.no_wait {
            args.push("--no-wait".to_string());
        }
        if options.new_run {
            args.push("--new".to_string());
        }
        if options.force {
            args.push("--force".to_string());
        }
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = self.run_in(project, &args_ref).await?;
        let manifest: RunResponse = result.success()?.stdout_json()?;
        Ok(RunHandle::new(
            self.harness.clone(),
            project.clone(),
            manifest.run_id,
        ))
    }

    pub async fn status_json(
        &self,
        project: &ProjectHandle,
        run_id: &str,
    ) -> Result<RunStatusResponse> {
        let args_owned = [
            "status".to_string(),
            "--run-id".to_string(),
            run_id.to_string(),
        ];
        let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        self.run_in(project, &args_ref)
            .await?
            .success()?
            .stdout_json()
    }

    pub async fn logs_json(
        &self,
        project: &ProjectHandle,
        run_id: &str,
        service: &str,
        last: usize,
    ) -> Result<LogsResponse> {
        let args_owned = [
            "logs".to_string(),
            "--run-id".to_string(),
            run_id.to_string(),
            "--service".to_string(),
            service.to_string(),
            "--last".to_string(),
            last.to_string(),
        ];
        let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        self.run_in(project, &args_ref)
            .await?
            .success()?
            .stdout_json()
    }

    pub async fn run_task_detached(
        &self,
        project: &ProjectHandle,
        task_name: &str,
        options: TaskStartOptions,
    ) -> Result<TaskHandle> {
        let mut args = vec![
            "run".to_string(),
            task_name.to_string(),
            "--project".to_string(),
            project.path_string(),
            "--detach".to_string(),
        ];
        if !options.args.is_empty() {
            args.push("--".to_string());
            args.extend(options.args.clone());
        }
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = self.run_in(project, &args_ref).await?;
        let started: StartTaskResponse = result.success()?.stdout_json()?;
        Ok(TaskHandle::new(
            self.harness.clone(),
            project.clone(),
            started.execution_id,
            started.task,
            started.run_id,
        ))
    }

    pub async fn task_status_json(
        &self,
        project: &ProjectHandle,
        execution_id: &str,
    ) -> Result<TaskStatusResponse> {
        let args_owned = [
            "run".to_string(),
            "--status".to_string(),
            execution_id.to_string(),
        ];
        let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        self.run_in(project, &args_ref)
            .await?
            .success()?
            .stdout_json()
    }

    pub async fn down(&self, project: &ProjectHandle, run_id: &str) -> Result<CmdResult> {
        let args_owned = [
            "down".to_string(),
            "--run-id".to_string(),
            run_id.to_string(),
        ];
        let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        self.run_in(project, &args_ref).await
    }

    pub async fn kill(&self, project: &ProjectHandle, run_id: &str) -> Result<CmdResult> {
        let args_owned = [
            "kill".to_string(),
            "--run-id".to_string(),
            run_id.to_string(),
        ];
        let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        self.run_in(project, &args_ref).await
    }

    pub async fn list_tasks_json(&self, project: &ProjectHandle) -> Result<serde_json::Value> {
        self.run_in(project, &["run", "--project", &project.path_string()])
            .await?
            .success()?
            .stdout_json()
    }

    pub async fn run_task_json(
        &self,
        project: &ProjectHandle,
        task_name: &str,
        args: &[&str],
    ) -> Result<serde_json::Value> {
        let mut owned = vec![
            "run".to_string(),
            task_name.to_string(),
            "--project".to_string(),
            project.path_string(),
        ];
        if !args.is_empty() {
            owned.push("--".to_string());
            owned.extend(args.iter().map(|arg| (*arg).to_string()));
        }
        let args_ref: Vec<&str> = owned.iter().map(String::as_str).collect();
        self.run_in(project, &args_ref)
            .await?
            .success()?
            .stdout_json()
    }

    pub async fn run_task(&self, project: &ProjectHandle, task_name: &str) -> Result<CmdResult> {
        self.run_in(
            project,
            &["run", task_name, "--project", &project.path_string()],
        )
        .await
    }

    pub async fn run_task_verbose(
        &self,
        project: &ProjectHandle,
        task_name: &str,
    ) -> Result<CmdResult> {
        self.run_in(
            project,
            &[
                "run",
                task_name,
                "--project",
                &project.path_string(),
                "--verbose",
            ],
        )
        .await
    }

    pub async fn run_init_json(&self, project: &ProjectHandle) -> Result<serde_json::Value> {
        self.run_in(
            project,
            &["run", "--init", "--project", &project.path_string()],
        )
        .await?
        .success()?
        .stdout_json()
    }

    pub async fn watch_status_json(
        &self,
        _project: &ProjectHandle,
        run_id: &str,
    ) -> Result<RunWatchResponse> {
        self.harness.api().watch_status(run_id).await
    }

    pub async fn projects_list_json(&self, project: &ProjectHandle) -> Result<ProjectsResponse> {
        self.run_in(project, &["projects", "ls"])
            .await?
            .success()?
            .stdout_json()
    }

    pub async fn projects_add(&self, project: &ProjectHandle, path: &Path) -> Result<CmdResult> {
        self.run_in(
            project,
            &["projects", "add", path.to_string_lossy().as_ref()],
        )
        .await
    }

    pub async fn projects_remove(
        &self,
        project: &ProjectHandle,
        target: &str,
    ) -> Result<CmdResult> {
        self.run_in(project, &["projects", "remove", target]).await
    }

    pub async fn sources_list_json(&self, project: &ProjectHandle) -> Result<SourcesResponse> {
        self.run_in(project, &["sources", "ls"])
            .await?
            .success()?
            .stdout_json()
    }

    pub async fn sources_add(
        &self,
        project: &ProjectHandle,
        name: &str,
        paths: &[String],
    ) -> Result<CmdResult> {
        let mut owned = vec!["sources".to_string(), "add".to_string(), name.to_string()];
        owned.extend(paths.iter().cloned());
        let args_ref: Vec<&str> = owned.iter().map(String::as_str).collect();
        self.run_in(project, &args_ref).await
    }

    pub async fn sources_remove(&self, project: &ProjectHandle, name: &str) -> Result<CmdResult> {
        self.run_in(project, &["sources", "rm", name]).await
    }
}

pub struct CmdResult {
    pub args: Vec<String>,
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl CmdResult {
    pub(super) fn new(args: &[&str], status: ExitStatus, stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            status,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }

    pub fn success(self) -> Result<Self> {
        if self.status.success() {
            return Ok(self);
        }
        Err(anyhow!(
            "command {:?} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            self.args,
            self.status.code(),
            self.stdout,
            self.stderr,
        ))
    }

    pub fn failure(self) -> Result<Self> {
        if !self.status.success() {
            return Ok(self);
        }
        Err(anyhow!(
            "expected command {:?} to fail but it succeeded\nstdout:\n{}\nstderr:\n{}",
            self.args,
            self.stdout,
            self.stderr,
        ))
    }

    pub fn assert_stdout_contains(&self, needle: &str) -> Result<()> {
        if self.stdout.contains(needle) {
            Ok(())
        } else {
            Err(anyhow!(
                "expected stdout for {:?} to contain {:?}\nstdout:\n{}\nstderr:\n{}",
                self.args,
                needle,
                self.stdout,
                self.stderr,
            ))
        }
    }

    pub fn assert_stderr_contains(&self, needle: &str) -> Result<()> {
        if self.stderr.contains(needle) {
            Ok(())
        } else {
            Err(anyhow!(
                "expected stderr for {:?} to contain {:?}\nstdout:\n{}\nstderr:\n{}",
                self.args,
                needle,
                self.stdout,
                self.stderr,
            ))
        }
    }

    pub fn stdout_json<T: DeserializeOwned>(&self) -> Result<T> {
        match serde_json::from_str(&self.stdout) {
            Ok(value) => Ok(value),
            Err(json_err) => {
                let opts = toon_format::DecodeOptions::new()
                    .with_expand_paths(toon_format::types::PathExpansionMode::Safe);
                let json_value: serde_json::Value = toon_format::decode(&self.stdout, &opts)
                    .with_context(|| {
                        format!(
                            "parse command stdout as json or toon:\njson error: {json_err}\nstdout:\n{}",
                            self.stdout
                        )
                    })?;
                serde_json::from_value(json_value)
                    .with_context(|| format!("deserialize toon value:\n{}", self.stdout))
            }
        }
    }

    pub fn stdout_json_lines<T: DeserializeOwned>(&self) -> Result<Vec<T>> {
        self.stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .with_context(|| format!("parse command stdout line as json:\n{line}"))
            })
            .collect()
    }
}

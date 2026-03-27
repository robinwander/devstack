use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::api::{StartTaskRequest, StartTaskResponse, TaskStatusResponse};
use crate::cli::context::{
    CliContext, DAEMON_TIMEOUT, resolve_active_run_id, resolve_project_context, resolve_stack_name,
};
use crate::cli::output::print_json;
use crate::config::ConfigFile;
use crate::paths;

#[derive(Clone, Debug)]
enum TaskExecutionTarget {
    Run(crate::ids::RunId),
    AdHoc,
}

impl TaskExecutionTarget {
    fn scope(&self) -> crate::services::tasks::TaskLogScope<'_> {
        match self {
            Self::Run(run_id) => crate::services::tasks::TaskLogScope::Run(run_id),
            Self::AdHoc => crate::services::tasks::TaskLogScope::AdHoc,
        }
    }

    fn history_path(&self, project_dir: &Path) -> Result<PathBuf> {
        match self {
            Self::Run(run_id) => paths::task_history_path(run_id),
            Self::AdHoc => paths::ad_hoc_task_history_path(project_dir),
        }
    }
}

async fn resolve_task_execution_target(
    context: &CliContext,
    project_dir: &Path,
) -> Result<TaskExecutionTarget> {
    Ok(match resolve_active_run_id(context, project_dir).await? {
        Some(run_id) => TaskExecutionTarget::Run(crate::ids::RunId::new(run_id)),
        None => TaskExecutionTarget::AdHoc,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    context: &CliContext,
    name: Option<String>,
    init: bool,
    stack: Option<String>,
    project: Option<PathBuf>,
    file: Option<PathBuf>,
    detach: bool,
    status: Option<String>,
    verbose: bool,
    json: bool,
    trailing_args: Vec<String>,
) -> Result<()> {
    if let Some(execution_id) = status {
        let path = format!("/v1/tasks/{execution_id}");
        let status: TaskStatusResponse = context
            .daemon_request_json("GET", &path, None::<()>, Some(DAEMON_TIMEOUT))
            .await?;
        if json {
            print_json(serde_json::to_value(status)?, context.pretty);
        } else {
            let duration = crate::services::tasks::format_task_duration(
                std::time::Duration::from_millis(status.duration_ms),
            );
            match status.state {
                crate::api::TaskExecutionState::Running => {
                    eprintln!(
                        "{} [{}] running for {duration}",
                        status.task, status.execution_id
                    );
                }
                crate::api::TaskExecutionState::Completed => {
                    eprintln!(
                        "{} [{}] completed in {duration}",
                        status.task, status.execution_id
                    );
                }
                crate::api::TaskExecutionState::Failed => {
                    let exit_code = status
                        .exit_code
                        .map(|code| format!("exit code {code}"))
                        .unwrap_or_else(|| "failed to start".to_string());
                    eprintln!(
                        "{} [{}] failed ({exit_code}) after {duration}",
                        status.task, status.execution_id
                    );
                }
            }
        }
        return Ok(());
    }

    let resolved_context = resolve_project_context(project, file)?;
    let config_path = resolved_context
        .config_path
        .ok_or_else(|| anyhow!("no devstack config found; run devstack init or pass --file"))?;
    if !config_path.is_file() {
        return Err(anyhow!(
            "config not found at {}; run devstack init or pass --file",
            config_path.to_string_lossy()
        ));
    }
    let config = ConfigFile::load_from_path(&config_path)?;
    let project_dir = resolved_context.project_dir;

    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();

    if init {
        let task_target = resolve_task_execution_target(context, &project_dir).await?;
        let stack_name = resolve_stack_name(stack, Some(&config_path))?;
        let stack_plan = config
            .stack_plan(&stack_name)
            .map_err(|err| anyhow!("{err}"))?;

        let history_path = task_target.history_path(&project_dir)?;
        let mut ran_any = false;
        for svc_name in &stack_plan.order {
            let svc = &stack_plan.services[svc_name];
            if let Some(init_tasks) = &svc.init
                && !init_tasks.is_empty()
            {
                crate::services::tasks::run_init_tasks(
                    &tasks_map,
                    init_tasks,
                    &project_dir,
                    task_target.scope(),
                    &history_path,
                    verbose,
                )?;
                ran_any = true;
            }
        }
        if !ran_any {
            eprintln!("no init tasks defined for stack '{stack_name}'");
        }
        if json {
            print_json(
                serde_json::json!({ "ok": true, "mode": "init", "stack": stack_name }),
                context.pretty,
            );
        }
        return Ok(());
    }

    if detach && name.is_none() {
        return Err(anyhow!("--detach requires a task name"));
    }

    let Some(task_name) = name else {
        if tasks_map.is_empty() {
            eprintln!("no tasks defined in {}", config_path.to_string_lossy());
            return Ok(());
        }
        if json {
            let names: Vec<&String> = tasks_map.keys().collect();
            print_json(serde_json::json!({ "tasks": names }), context.pretty);
        } else {
            eprintln!("Available tasks:");
            for (name, task) in &tasks_map {
                let cmd = match task {
                    crate::config::TaskConfig::Command(cmd) => cmd.clone(),
                    crate::config::TaskConfig::Structured(def) => def.cmd.clone(),
                };
                eprintln!("  {name:<24} {cmd}");
            }
        }
        return Ok(());
    };

    if detach {
        let request = StartTaskRequest {
            project_dir: project_dir.to_string_lossy().to_string(),
            file: Some(config_path.to_string_lossy().to_string()),
            task: task_name,
            args: trailing_args,
        };
        let response: StartTaskResponse = context
            .daemon_request_json("POST", "/v1/tasks/run", Some(request), Some(DAEMON_TIMEOUT))
            .await?;
        if json {
            print_json(serde_json::to_value(response)?, context.pretty);
        } else {
            println!("{}", response.execution_id);
        }
        return Ok(());
    }

    let task_target = resolve_task_execution_target(context, &project_dir).await?;
    let task = tasks_map
        .get(&task_name)
        .ok_or_else(|| anyhow!("unknown task '{task_name}'"))?;

    let history_path = task_target.history_path(&project_dir)?;
    let result = crate::services::tasks::run_task(
        &task_name,
        task,
        &project_dir,
        task_target.scope(),
        &history_path,
        verbose,
        &trailing_args,
    )?;

    if json {
        let stderr_summary = result
            .last_stderr_line
            .as_deref()
            .map(|line| crate::services::tasks::summarize_stderr_line(line, 120));
        print_json(
            serde_json::json!({
                "task": task_name,
                "exit_code": result.exit_code,
                "duration_ms": result.duration.as_millis(),
                "last_stderr_line": stderr_summary,
            }),
            context.pretty,
        );
    } else if result.success() {
        eprintln!(
            "✓ {} ({})",
            task_name,
            crate::services::tasks::format_task_duration(result.duration)
        );
    } else {
        let reason = result
            .last_stderr_line
            .as_deref()
            .map(|line| crate::services::tasks::summarize_stderr_line(line, 120))
            .filter(|line| !line.is_empty())
            .unwrap_or_else(|| format!("exit code {}", result.exit_code));
        eprintln!(
            "✗ {} ({}) — {}",
            task_name,
            crate::services::tasks::format_task_duration(result.duration),
            reason
        );
        eprintln!("  devstack logs --task {} --last 30", task_name);
    }

    if !result.success() {
        std::process::exit(result.exit_code);
    }
    Ok(())
}

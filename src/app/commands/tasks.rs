use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use rand::Rng;

use crate::api::{DaemonTaskEventKind, StartTaskRequest, StartTaskResponse, TaskExecutionState};
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::runtime::{find_latest_active_run_for_project, task_event};
use crate::config::{ConfigFile, TaskConfig};
use crate::ids::RunId;
use crate::infra::logs::index::LogSource;
use crate::paths;
use crate::stores::DetachedTaskExecution;
use crate::util::now_rfc3339;

pub async fn run_init_tasks_blocking(
    tasks_map: BTreeMap<String, TaskConfig>,
    init_tasks: Vec<String>,
    project_dir: PathBuf,
    run_id: RunId,
) -> Result<()> {
    let history_path = paths::task_history_path(&run_id)?;
    tokio::task::spawn_blocking(move || {
        crate::tasks::run_init_tasks(
            &tasks_map,
            &init_tasks,
            &project_dir,
            crate::tasks::TaskLogScope::Run(&run_id),
            &history_path,
            false,
        )
    })
    .await
    .map_err(|err| anyhow!("init task worker failed: {err}"))?
}

pub async fn run_post_init_tasks_blocking(
    tasks_map: BTreeMap<String, TaskConfig>,
    post_init_tasks: Vec<String>,
    project_dir: PathBuf,
    run_id: RunId,
) -> Result<()> {
    let history_path = paths::task_history_path(&run_id)?;
    tokio::task::spawn_blocking(move || {
        crate::tasks::run_post_init_tasks(
            &tasks_map,
            &post_init_tasks,
            &project_dir,
            crate::tasks::TaskLogScope::Run(&run_id),
            &history_path,
            false,
        )
    })
    .await
    .map_err(|err| anyhow!("post_init task worker failed: {err}"))?
}

pub async fn start_task(app: &AppContext, req: StartTaskRequest) -> AppResult<StartTaskResponse> {
    let project_dir = PathBuf::from(&req.project_dir);
    let config_path = req
        .file
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| ConfigFile::default_path(&project_dir));
    let config = ConfigFile::load_from_path(&config_path)
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let task = config
        .tasks
        .as_ref()
        .and_then(|tasks| tasks.as_map().get(&req.task))
        .cloned()
        .ok_or_else(|| AppError::bad_request(format!("unknown task '{}'", req.task)))?;

    let run_id = find_latest_active_run_for_project(app, &project_dir)
        .await
        .map_err(AppError::from)?;
    let execution_id = format!("task-{:016x}", rand::rng().random::<u64>());
    let detached_task = DetachedTaskExecution {
        execution_id: execution_id.clone(),
        task: req.task.clone(),
        project_dir: project_dir.clone(),
        run_id: run_id.clone(),
        state: TaskExecutionState::Running,
        started_at: now_rfc3339(),
        started_at_instant: std::time::Instant::now(),
        finished_at: None,
        exit_code: None,
        duration_ms: None,
    };

    let duplicate = app
        .tasks
        .has_running_task(&req.task, run_id.as_deref(), &project_dir)
        .await;
    if duplicate {
        return Err(AppError::bad_request(format!(
            "task '{}' is already running",
            req.task
        )));
    }
    app.tasks
        .add_task(detached_task.clone())
        .await
        .map_err(AppError::from)?;

    app.emit_event(task_event(&detached_task, DaemonTaskEventKind::Started));
    tokio::spawn(execute_detached_task(
        app.clone(),
        detached_task,
        task,
        req.args.clone(),
    ));

    Ok(StartTaskResponse {
        execution_id,
        task: req.task,
        run_id,
    })
}

pub async fn execute_detached_task(
    app: AppContext,
    detached_task: DetachedTaskExecution,
    task: TaskConfig,
    args: Vec<String>,
) {
    let result = if let Some(run_id) = detached_task.run_id.clone() {
        let run_id = RunId::new(run_id);
        match paths::task_history_path(&run_id) {
            Ok(history_path) => tokio::task::spawn_blocking({
                let task_name = detached_task.task.clone();
                let project_dir = detached_task.project_dir.clone();
                let args = args.clone();
                let task = task.clone();
                move || {
                    crate::tasks::run_task(
                        &task_name,
                        &task,
                        &project_dir,
                        crate::tasks::TaskLogScope::Run(&run_id),
                        &history_path,
                        false,
                        &args,
                    )
                }
            })
            .await
            .map_err(|err| anyhow!("task worker failed: {err}"))
            .and_then(|result| result),
            Err(err) => Err(err),
        }
    } else {
        match paths::ad_hoc_task_history_path(&detached_task.project_dir) {
            Ok(history_path) => tokio::task::spawn_blocking({
                let task_name = detached_task.task.clone();
                let project_dir = detached_task.project_dir.clone();
                let args = args.clone();
                move || {
                    crate::tasks::run_task(
                        &task_name,
                        &task,
                        &project_dir,
                        crate::tasks::TaskLogScope::AdHoc,
                        &history_path,
                        false,
                        &args,
                    )
                }
            })
            .await
            .map_err(|err| anyhow!("task worker failed: {err}"))
            .and_then(|result| result),
            Err(err) => Err(err),
        }
    };

    let updated_task = app
        .tasks
        .update_task_state(
            &detached_task.execution_id,
            result
                .as_ref()
                .ok()
                .map(|task_result| task_result.exit_code),
        )
        .await
        .unwrap_or(detached_task.clone());

    if let Some(run_id) = updated_task.run_id.clone() {
        let task_name = updated_task.task.clone();
        let log_index = app.log_index.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let path = paths::task_log_path(&RunId::new(&run_id), &task_name)?;
            if !path.exists() {
                return Ok::<(), anyhow::Error>(());
            }
            log_index.ingest_sources(&[LogSource {
                run_id: run_id.clone(),
                service: format!("task:{task_name}"),
                path,
            }])?;
            Ok(())
        })
        .await;
    }

    let kind = if updated_task.state == TaskExecutionState::Completed {
        DaemonTaskEventKind::Completed
    } else {
        DaemonTaskEventKind::Failed
    };
    app.emit_event(task_event(&updated_task, kind));
}

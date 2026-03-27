use std::collections::BTreeMap;

use crate::api::{TaskStatusResponse, TasksResponse};
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::runtime::{
    task_status_response, task_summary_from_detached, task_summary_from_history,
};
use crate::ids::RunId;
use crate::paths;

pub async fn task_status(app: &AppContext, execution_id: &str) -> AppResult<TaskStatusResponse> {
    let task =
        app.tasks.get_task(execution_id).await.ok_or_else(|| {
            AppError::not_found(format!("task execution {execution_id} not found"))
        })?;
    Ok(task_status_response(&task))
}

pub async fn run_tasks(app: &AppContext, run_id: &str) -> AppResult<TasksResponse> {
    app.runs
        .with_run(run_id, |_| ())
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))?;

    let detached_tasks = app.tasks.list_tasks_for_run(run_id).await;
    let history =
        crate::services::tasks::TaskHistory::load(&paths::task_history_path(&RunId::new(run_id))?)
            .map_err(AppError::from)?;

    let mut tasks = BTreeMap::new();
    for execution in history.latest_by_task().into_values() {
        merge_task_summary(&mut tasks, task_summary_from_history(execution));
    }
    for task in detached_tasks {
        merge_task_summary(&mut tasks, task_summary_from_detached(&task));
    }

    Ok(TasksResponse {
        tasks: tasks.into_values().collect(),
    })
}

fn merge_task_summary(
    tasks: &mut BTreeMap<String, crate::api::TaskExecutionSummary>,
    summary: crate::api::TaskExecutionSummary,
) {
    let replace = tasks
        .get(&summary.task)
        .map(|current| summary.started_at >= current.started_at)
        .unwrap_or(true);
    if replace {
        tasks.insert(summary.task.clone(), summary);
    }
}

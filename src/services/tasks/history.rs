use std::collections::{BTreeMap, btree_map::Entry};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use super::model::{TaskExecution, TaskHistory, TaskLogScope, TaskResult};
use crate::paths;
use crate::util::atomic_write;

impl TaskHistory {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes =
            std::fs::read(path).with_context(|| format!("read task history {}", path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse task history {}", path.display()))
    }

    pub fn append(&mut self, execution: TaskExecution, path: &Path) -> Result<()> {
        self.executions.push(execution);
        let bytes = serde_json::to_vec_pretty(self).context("serialize task history")?;
        atomic_write(path, &bytes).with_context(|| format!("write task history {}", path.display()))
    }

    pub fn latest_by_task(&self) -> BTreeMap<String, &TaskExecution> {
        let mut latest = BTreeMap::new();
        for execution in &self.executions {
            match latest.entry(execution.task.clone()) {
                Entry::Vacant(slot) => {
                    slot.insert(execution);
                }
                Entry::Occupied(mut slot) => {
                    let current = slot.get();
                    if execution.finished_at > current.finished_at
                        || (execution.finished_at == current.finished_at
                            && execution.started_at >= current.started_at)
                    {
                        slot.insert(execution);
                    }
                }
            }
        }
        latest
    }
}

pub(crate) fn append_task_execution(
    history_path: &Path,
    task_name: &str,
    scope: TaskLogScope<'_>,
    started_at: String,
    finished_at: String,
    result: &TaskResult,
) -> Result<()> {
    let mut history = TaskHistory::load(history_path)?;
    history.append(
        TaskExecution {
            task: task_name.to_string(),
            started_at,
            finished_at,
            exit_code: result.exit_code,
            duration_ms: result.duration.as_millis().try_into().unwrap_or(u64::MAX),
            log_file: format!("{task_name}.log"),
            scope: task_scope_label(scope),
        },
        history_path,
    )
}

fn task_scope_label(scope: TaskLogScope<'_>) -> String {
    match scope {
        TaskLogScope::Run(run_id) => format!("run:{}", run_id.as_str()),
        TaskLogScope::AdHoc => "adhoc".to_string(),
    }
}

pub fn task_log_path(
    project_dir: &Path,
    task_name: &str,
    scope: TaskLogScope<'_>,
) -> Result<PathBuf> {
    match scope {
        TaskLogScope::Run(run_id) => paths::task_log_path(run_id, task_name),
        TaskLogScope::AdHoc => paths::ad_hoc_task_log_path(project_dir, task_name),
    }
}

pub fn format_task_duration(duration: Duration) -> String {
    format!("{:.1}s", duration.as_secs_f64())
}

pub fn summarize_stderr_line(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let trimmed = value.trim();
    let mut out = String::new();
    for (count, ch) in trimmed.chars().enumerate() {
        if count + 1 >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

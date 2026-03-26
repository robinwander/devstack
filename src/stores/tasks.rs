use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant as StdInstant;
use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use crate::api::{DaemonEvent, DaemonTaskEvent, DaemonTaskEventKind, TaskExecutionState};

/// Detached task execution record
#[derive(Clone, Debug)]
pub struct DetachedTaskExecution {
    pub execution_id: String,
    pub task: String,
    pub project_dir: PathBuf,
    pub run_id: Option<String>,
    pub state: TaskExecutionState,
    pub started_at: String,
    pub started_at_instant: StdInstant,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
}

impl DetachedTaskExecution {
    /// Create a new detached task execution
    pub fn new(
        execution_id: String,
        task: String,
        project_dir: PathBuf,
        run_id: Option<String>,
    ) -> Self {
        Self {
            execution_id,
            task,
            project_dir,
            run_id,
            state: TaskExecutionState::Running,
            started_at: crate::util::now_rfc3339(),
            started_at_instant: StdInstant::now(),
            finished_at: None,
            exit_code: None,
            duration_ms: None,
        }
    }

    /// Mark the task as completed
    pub fn mark_completed(&mut self, exit_code: i32) {
        self.state = if exit_code == 0 {
            TaskExecutionState::Completed
        } else {
            TaskExecutionState::Failed
        };
        self.exit_code = Some(exit_code);
        self.finished_at = Some(crate::util::now_rfc3339());
        self.duration_ms = Some(
            self.started_at_instant
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        );
    }

    /// Mark the task as failed
    pub fn mark_failed(&mut self) {
        self.state = TaskExecutionState::Failed;
        self.finished_at = Some(crate::util::now_rfc3339());
        self.duration_ms = Some(
            self.started_at_instant
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        );
    }

    /// Get the current duration in milliseconds
    pub fn current_duration_ms(&self) -> u64 {
        self.duration_ms.unwrap_or_else(|| {
            self.started_at_instant
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX)
        })
    }
}

/// Store for managing detached task executions
pub struct TaskStore {
    inner: Mutex<BTreeMap<String, DetachedTaskExecution>>,
}

impl TaskStore {
    /// Create a new task store
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    /// Add a new task execution
    pub async fn add_task(&self, task: DetachedTaskExecution) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if guard.contains_key(&task.execution_id) {
            return Err(anyhow!("task execution {} already exists", task.execution_id));
        }
        guard.insert(task.execution_id.clone(), task);
        Ok(())
    }

    /// Get a task by execution ID
    pub async fn get_task(&self, execution_id: &str) -> Option<DetachedTaskExecution> {
        let guard = self.inner.lock().await;
        guard.get(execution_id).cloned()
    }

    /// List all tasks
    pub async fn list_tasks(&self) -> Vec<DetachedTaskExecution> {
        let guard = self.inner.lock().await;
        guard.values().cloned().collect()
    }

    /// List tasks for a specific run
    pub async fn list_tasks_for_run(&self, run_id: &str) -> Vec<DetachedTaskExecution> {
        let guard = self.inner.lock().await;
        guard
            .values()
            .filter(|task| task.run_id.as_deref() == Some(run_id))
            .cloned()
            .collect()
    }

    /// Check for duplicate running tasks
    pub async fn has_running_task(&self, task_name: &str, run_id: Option<&str>, project_dir: &PathBuf) -> bool {
        let guard = self.inner.lock().await;
        guard.values().any(|task| {
            task.state == TaskExecutionState::Running
                && task.task == task_name
                && match (task.run_id.as_deref(), run_id) {
                    (Some(existing), Some(candidate)) => existing == candidate,
                    (None, None) => same_project_dir(&task.project_dir, project_dir),
                    _ => false,
                }
        })
    }

    /// Update task state
    pub async fn update_task_state(
        &self,
        execution_id: &str,
        exit_code: Option<i32>,
    ) -> Result<DetachedTaskExecution> {
        let mut guard = self.inner.lock().await;
        let task = guard
            .get_mut(execution_id)
            .ok_or_else(|| anyhow!("task execution {} not found", execution_id))?;

        match exit_code {
            Some(code) => task.mark_completed(code),
            None => task.mark_failed(),
        }

        Ok(task.clone())
    }

    /// Remove a task
    pub async fn remove_task(&self, execution_id: &str) -> Option<DetachedTaskExecution> {
        let mut guard = self.inner.lock().await;
        guard.remove(execution_id)
    }

    /// Clean up finished tasks older than a certain age
    pub async fn cleanup_finished_tasks(&self, max_age_secs: u64) -> usize {
        let mut guard = self.inner.lock().await;
        let cutoff = StdInstant::now()
            .checked_sub(std::time::Duration::from_secs(max_age_secs))
            .unwrap_or(StdInstant::now());

        let before_count = guard.len();
        guard.retain(|_, task| {
            match task.state {
                TaskExecutionState::Running => true, // Keep running tasks
                _ => task.started_at_instant > cutoff, // Remove old finished tasks
            }
        });
        
        before_count - guard.len()
    }
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a task to an event
pub fn task_event(task: &DetachedTaskExecution, kind: DaemonTaskEventKind) -> DaemonEvent {
    DaemonEvent::Task(DaemonTaskEvent {
        kind,
        execution_id: task.execution_id.clone(),
        task: task.task.clone(),
        run_id: task.run_id.clone(),
        state: task.state.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
        exit_code: task.exit_code,
        duration_ms: task.duration_ms,
    })
}

/// Helper function to check if two paths refer to the same project directory
fn same_project_dir(a: &PathBuf, b: &PathBuf) -> bool {
    a.canonicalize().ok() == b.canonicalize().ok()
}
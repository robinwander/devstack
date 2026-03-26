use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ids::RunId;

#[derive(Clone, Copy, Debug)]
pub enum TaskLogScope<'a> {
    Run(&'a RunId),
    AdHoc,
}

#[derive(Clone, Debug)]
pub struct TaskResult {
    pub exit_code: i32,
    pub duration: Duration,
    pub last_stderr_line: Option<String>,
}

impl TaskResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskExecution {
    pub task: String,
    pub started_at: String,
    pub finished_at: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub log_file: String,
    pub scope: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TaskHistory {
    pub executions: Vec<TaskExecution>,
}

pub mod executor;
pub mod history;
pub mod model;
pub mod orchestration;

// Re-export the main types that were public in the original tasks.rs
pub use executor::{ServiceLogSink, run_init_tasks, run_post_init_tasks, run_task};
pub use history::{format_task_duration, summarize_stderr_line, task_log_path};
pub use model::{TaskExecution, TaskHistory, TaskLogScope, TaskResult};
pub use orchestration::{compute_watch_hash, load_stored_hash, store_hash, task_cwd, task_watch};

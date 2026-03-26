pub mod model;
pub mod executor;
pub mod orchestration;
pub mod history;

// Re-export the main types that were public in the original tasks.rs
pub use model::{TaskLogScope, TaskResult, TaskExecution, TaskHistory};
pub use executor::{run_task, run_init_tasks, run_post_init_tasks};
pub use orchestration::{compute_watch_hash, load_stored_hash, store_hash, task_watch, task_cwd};
pub use history::{format_task_duration, summarize_stderr_line, task_log_path};
use std::collections::{BTreeMap, btree_map::Entry};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::config::TaskConfig;
use crate::ids::RunId;
use crate::paths;
use crate::util::{
    atomic_write, now_rfc3339, strip_ansi_if_needed, validate_name_for_path_component,
};

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

impl TaskResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Compute a hash of the watched files for a task.
pub fn compute_watch_hash(cwd: &Path, watch: &[String]) -> Result<String> {
    crate::watch::compute_watch_hash(cwd, Some(watch), &[], &[], b"task-watch-v1")
}

pub fn load_stored_hash(project_dir: &Path, task_name: &str) -> Result<Option<String>> {
    let path = task_hash_path(project_dir, task_name)?;
    if !path.exists() {
        return Ok(None);
    }
    let value = std::fs::read_to_string(&path)
        .with_context(|| format!("read task hash {}", path.display()))?;
    Ok(Some(value.trim().to_string()))
}

pub fn store_hash(project_dir: &Path, task_name: &str, hash: &str) -> Result<()> {
    let dir = paths::task_hashes_dir(project_dir)?;
    std::fs::create_dir_all(&dir)?;
    let path = task_hash_path(project_dir, task_name)?;
    std::fs::write(&path, hash.as_bytes())
        .with_context(|| format!("write task hash {}", path.display()))?;
    Ok(())
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

fn task_scope_label(scope: TaskLogScope<'_>) -> String {
    match scope {
        TaskLogScope::Run(run_id) => format!("run:{}", run_id.as_str()),
        TaskLogScope::AdHoc => "adhoc".to_string(),
    }
}

fn append_task_execution(
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

/// Run a single task synchronously.
///
/// Default mode captures stdout/stderr into a task log file. Verbose mode
/// preserves legacy interactive behavior by inheriting stdio.
pub fn run_task(
    task_name: &str,
    task: &TaskConfig,
    project_dir: &Path,
    log_scope: TaskLogScope<'_>,
    history_path: &Path,
    verbose: bool,
    trailing_args: &[String],
) -> Result<TaskResult> {
    let (mut cmd, cwd, env, env_file) = task_cmd_parts(task);
    if !trailing_args.is_empty() {
        for arg in trailing_args {
            cmd.push(' ');
            cmd.push_str(&shlex::try_quote(arg).map_err(|e| anyhow!("failed to shell-escape arg: {e}"))?);
        }
    }
    let cwd = match cwd {
        Some(p) if p.is_absolute() => p,
        Some(p) => project_dir.join(p),
        None => project_dir.to_path_buf(),
    };

    let mut command = Command::new("/bin/bash");
    command.arg("-lc").arg(&cmd).current_dir(&cwd);

    if verbose {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    } else {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }

    if let Some(env_file) = env_file {
        let env_path = if env_file.is_absolute() {
            env_file
        } else {
            cwd.join(env_file)
        };
        if env_path.exists() {
            let iter = dotenvy::from_path_iter(&env_path)
                .with_context(|| format!("read env file {}", env_path.display()))?;
            for item in iter {
                let (k, v) =
                    item.with_context(|| format!("parse env file {}", env_path.display()))?;
                command.env(k, v);
            }
        }
    }

    for (k, v) in env {
        command.env(k, v);
    }

    let started_at = now_rfc3339();
    let start = Instant::now();

    if verbose {
        let status = command.status().context("run task")?;
        let result = TaskResult {
            exit_code: status.code().unwrap_or(1),
            duration: start.elapsed(),
            last_stderr_line: None,
        };
        append_task_execution(
            history_path,
            task_name,
            log_scope,
            started_at,
            now_rfc3339(),
            &result,
        )?;
        return Ok(result);
    }

    let log_path = task_log_path(project_dir, task_name, log_scope)?;
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create task log dir {}", parent.display()))?;
    }

    let mut child = command.spawn().context("run task")?;

    let log_file = File::create(&log_path)
        .with_context(|| format!("create task log {}", log_path.display()))?;
    let log_file = Arc::new(Mutex::new(log_file));

    let stdout = child.stdout.take().context("capture task stdout")?;
    let stderr = child.stderr.take().context("capture task stderr")?;

    let stderr_last_line = Arc::new(Mutex::new(None::<String>));

    let stdout_handle = spawn_log_pump(stdout, "stdout", log_file.clone(), None);
    let stderr_handle = spawn_log_pump(stderr, "stderr", log_file, Some(stderr_last_line.clone()));

    let status = child.wait().context("wait for task")?;
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    let last_stderr_line = stderr_last_line.lock().ok().and_then(|guard| guard.clone());

    let result = TaskResult {
        exit_code: status.code().unwrap_or(1),
        duration: start.elapsed(),
        last_stderr_line,
    };
    append_task_execution(
        history_path,
        task_name,
        log_scope,
        started_at,
        now_rfc3339(),
        &result,
    )?;
    Ok(result)
}

/// Run init tasks for a service. If any task fails, returns an error.
pub fn run_init_tasks(
    tasks: &BTreeMap<String, TaskConfig>,
    init: &[String],
    project_dir: &Path,
    log_scope: TaskLogScope<'_>,
    history_path: &Path,
    verbose: bool,
) -> Result<()> {
    for name in init {
        let task = tasks
            .get(name)
            .ok_or_else(|| anyhow!("unknown init task '{name}'"))?;

        let watch = task_watch(task);

        if !watch.is_empty() {
            let cwd = task_cwd(task, project_dir);
            let new_hash = compute_watch_hash(&cwd, &watch)?;
            if load_stored_hash(project_dir, name)?.as_deref() == Some(new_hash.as_str()) {
                eprintln!("✓ {name}: up to date");
                continue;
            }

            let result = run_task(name, task, project_dir, log_scope, history_path, verbose, &[])?;
            if !result.success() {
                emit_task_failure_summary(name, &result);
                return Err(anyhow!(
                    "init task '{name}' failed with exit code {}",
                    result.exit_code
                ));
            }

            eprintln!("✓ {name} ({})", format_task_duration(result.duration));
            store_hash(project_dir, name, &new_hash)?;
            continue;
        }

        let result = run_task(name, task, project_dir, log_scope, history_path, verbose, &[])?;
        if !result.success() {
            emit_task_failure_summary(name, &result);
            return Err(anyhow!(
                "init task '{name}' failed with exit code {}",
                result.exit_code
            ));
        }

        eprintln!("✓ {name} ({})", format_task_duration(result.duration));
    }
    Ok(())
}

fn emit_task_failure_summary(name: &str, result: &TaskResult) {
    let mut reason = format!("exit code {}", result.exit_code);
    if let Some(stderr_line) = &result.last_stderr_line {
        let summary = summarize_stderr_line(stderr_line, 120);
        if !summary.is_empty() {
            reason = summary;
        }
    }

    eprintln!(
        "✗ {name} ({}) — {reason}",
        format_task_duration(result.duration)
    );
    eprintln!("  devstack logs --task {name} --last 30");
}

fn spawn_log_pump<R: Read + Send + 'static>(
    reader: R,
    label: &'static str,
    log_file: Arc<Mutex<File>>,
    last_stderr_line: Option<Arc<Mutex<Option<String>>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            let Ok(bytes) = reader.read_line(&mut line) else {
                break;
            };
            if bytes == 0 {
                break;
            }

            let clean = strip_ansi_if_needed(line.trim_end_matches(['\n', '\r']));
            let entry = format!("[{}] [{label}] {clean}\n", now_rfc3339());

            if let Ok(mut file) = log_file.lock() {
                let _ = file.write_all(entry.as_bytes());
                let _ = file.flush();
            }

            if label == "stderr"
                && let Some(last) = &last_stderr_line
                && !clean.trim().is_empty()
                && let Ok(mut guard) = last.lock()
            {
                *guard = Some(clean);
            }
        }
    })
}

fn task_hash_path(project_dir: &Path, task_name: &str) -> Result<PathBuf> {
    validate_name_for_path_component("task", task_name)?;
    Ok(paths::task_hashes_dir(project_dir)?.join(format!("{task_name}.sha256")))
}

/// Extract (cmd, cwd, env, env_file) from a TaskConfig.
fn task_cmd_parts(
    task: &TaskConfig,
) -> (
    String,
    Option<PathBuf>,
    BTreeMap<String, String>,
    Option<PathBuf>,
) {
    match task {
        TaskConfig::Command(cmd) => (cmd.clone(), None, BTreeMap::new(), None),
        TaskConfig::Structured(def) => (
            def.cmd.clone(),
            def.cwd.clone(),
            def.env.clone(),
            def.env_file.clone(),
        ),
    }
}

/// Extract the watch list from a TaskConfig.
fn task_watch(task: &TaskConfig) -> Vec<String> {
    match task {
        TaskConfig::Command(_) => Vec::new(),
        TaskConfig::Structured(def) => def.watch.clone(),
    }
}

/// Resolve the working directory for a task.
fn task_cwd(task: &TaskConfig, project_dir: &Path) -> PathBuf {
    match task {
        TaskConfig::Command(_) => project_dir.to_path_buf(),
        TaskConfig::Structured(def) => match &def.cwd {
            Some(p) if p.is_absolute() => p.clone(),
            Some(p) => project_dir.join(p),
            None => project_dir.to_path_buf(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_hash_honors_glob_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "one\n").unwrap();

        let watch = vec!["src/**".to_string()];
        let hash_a = compute_watch_hash(root, &watch).unwrap();

        std::fs::write(root.join("README.md"), "two\n").unwrap();
        let hash_b = compute_watch_hash(root, &watch).unwrap();
        assert_eq!(hash_a, hash_b);

        std::fs::write(root.join("src/lib.rs"), "fn b() {}\n").unwrap();
        let hash_c = compute_watch_hash(root, &watch).unwrap();
        assert_ne!(hash_a, hash_c);
    }

    #[test]
    fn task_hash_storage_rejects_invalid_task_names() {
        let dir = tempfile::tempdir().unwrap();
        assert!(store_hash(dir.path(), "../escape", "abc").is_err());
        assert!(load_stored_hash(dir.path(), "nested/path").is_err());
    }

    #[test]
    fn task_history_load_append_and_latest_by_task() {
        let dir = tempfile::tempdir().unwrap();
        let history_path = dir.path().join("history.json");

        let mut history = TaskHistory::load(&history_path).unwrap();
        assert!(history.executions.is_empty());

        history
            .append(
                TaskExecution {
                    task: "build".to_string(),
                    started_at: "2025-03-01T10:00:00Z".to_string(),
                    finished_at: "2025-03-01T10:00:01Z".to_string(),
                    exit_code: 0,
                    duration_ms: 1000,
                    log_file: "build.log".to_string(),
                    scope: "adhoc".to_string(),
                },
                &history_path,
            )
            .unwrap();
        history
            .append(
                TaskExecution {
                    task: "build".to_string(),
                    started_at: "2025-03-01T11:00:00Z".to_string(),
                    finished_at: "2025-03-01T11:00:03Z".to_string(),
                    exit_code: 1,
                    duration_ms: 3000,
                    log_file: "build.log".to_string(),
                    scope: "run:run-1".to_string(),
                },
                &history_path,
            )
            .unwrap();
        history
            .append(
                TaskExecution {
                    task: "test".to_string(),
                    started_at: "2025-03-01T11:30:00Z".to_string(),
                    finished_at: "2025-03-01T11:30:02Z".to_string(),
                    exit_code: 0,
                    duration_ms: 2000,
                    log_file: "test.log".to_string(),
                    scope: "run:run-1".to_string(),
                },
                &history_path,
            )
            .unwrap();

        let loaded = TaskHistory::load(&history_path).unwrap();
        assert_eq!(loaded.executions.len(), 3);

        let latest = loaded.latest_by_task();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest["build"].exit_code, 1);
        assert_eq!(latest["test"].duration_ms, 2000);
    }

    #[test]
    fn run_task_uses_login_shell() {
        let project_dir = tempfile::tempdir().unwrap();
        let history_path = project_dir.path().join("history.json");

        let task = TaskConfig::Structured(crate::config::TaskDefinition {
            cmd: "echo $0".to_string(),
            cwd: None,
            watch: Vec::new(),
            env: BTreeMap::new(),
            env_file: None,
        });

        let result = run_task(
            "login-shell",
            &task,
            project_dir.path(),
            TaskLogScope::AdHoc,
            &history_path,
            false,
            &[],
        )
        .unwrap();

        assert!(result.success());
    }
}

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};

use super::history::{append_task_execution, format_task_duration, task_log_path};
use super::model::{TaskLogScope, TaskResult};
use super::orchestration::{
    compute_watch_hash, load_stored_hash, store_hash, task_cmd_parts, task_cwd, task_watch,
};
use crate::config::TaskConfig;
use crate::logfmt::strip_ansi_if_needed;
use crate::util::now_rfc3339;

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
            cmd.push_str(
                &shlex::try_quote(arg).map_err(|e| anyhow!("failed to shell-escape arg: {e}"))?,
            );
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

#[allow(clippy::too_many_arguments)]
fn run_service_tasks(
    tasks: &BTreeMap<String, TaskConfig>,
    task_names: &[String],
    project_dir: &Path,
    log_scope: TaskLogScope<'_>,
    history_path: &Path,
    verbose: bool,
    skip_if_unchanged: bool,
    phase: &str,
) -> Result<()> {
    for name in task_names {
        let task = tasks
            .get(name)
            .ok_or_else(|| anyhow!("unknown {phase} task '{name}'"))?;

        let watch = task_watch(task);

        if skip_if_unchanged && !watch.is_empty() {
            let cwd = task_cwd(task, project_dir);
            let new_hash = compute_watch_hash(&cwd, &watch)?;
            if load_stored_hash(project_dir, name)?.as_deref() == Some(new_hash.as_str()) {
                eprintln!("✓ {name}: up to date");
                continue;
            }

            let result = run_task(
                name,
                task,
                project_dir,
                log_scope,
                history_path,
                verbose,
                &[],
            )?;
            if !result.success() {
                emit_task_failure_summary(name, &result);
                return Err(anyhow!(
                    "{phase} task '{name}' failed with exit code {}",
                    result.exit_code
                ));
            }

            eprintln!("✓ {name} ({})", format_task_duration(result.duration));
            store_hash(project_dir, name, &new_hash)?;
            continue;
        }

        let result = run_task(
            name,
            task,
            project_dir,
            log_scope,
            history_path,
            verbose,
            &[],
        )?;
        if !result.success() {
            emit_task_failure_summary(name, &result);
            return Err(anyhow!(
                "{phase} task '{name}' failed with exit code {}",
                result.exit_code
            ));
        }

        eprintln!("✓ {name} ({})", format_task_duration(result.duration));
    }
    Ok(())
}

pub fn run_init_tasks(
    tasks: &BTreeMap<String, TaskConfig>,
    init: &[String],
    project_dir: &Path,
    log_scope: TaskLogScope<'_>,
    history_path: &Path,
    verbose: bool,
) -> Result<()> {
    run_service_tasks(
        tasks,
        init,
        project_dir,
        log_scope,
        history_path,
        verbose,
        true,
        "init",
    )
}

pub fn run_post_init_tasks(
    tasks: &BTreeMap<String, TaskConfig>,
    post_init: &[String],
    project_dir: &Path,
    log_scope: TaskLogScope<'_>,
    history_path: &Path,
    verbose: bool,
) -> Result<()> {
    run_service_tasks(
        tasks,
        post_init,
        project_dir,
        log_scope,
        history_path,
        verbose,
        false,
        "post_init",
    )
}

fn emit_task_failure_summary(name: &str, result: &TaskResult) {
    let mut reason = format!("exit code {}", result.exit_code);
    if let Some(stderr_line) = &result.last_stderr_line {
        let summary = super::history::summarize_stderr_line(stderr_line, 120);
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

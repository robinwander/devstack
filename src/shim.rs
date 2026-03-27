use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::signal::unix::{SignalKind, signal};

use crate::util::now_rfc3339;

#[derive(Clone, Debug)]
pub struct ShimArgs {
    pub run_id: String,
    pub service: String,
    pub cmd: String,
    pub cwd: PathBuf,
    pub log_file: PathBuf,
}

pub async fn run(args: ShimArgs) -> Result<()> {
    let mut command = Command::new("/bin/bash");
    command
        .arg("-lc")
        .arg(&args.cmd)
        .current_dir(&args.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // SAFETY: `pre_exec` runs in the child process after `fork` and before `exec`.
    // We only call async-signal-safe `setpgid(0, 0)` so the service runs in its own
    // process group for reliable group signaling on shutdown.
    unsafe {
        command.pre_exec(|| {
            let rc = libc::setpgid(0, 0);
            if rc != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command.spawn().context("spawn service command")?;
    let pgid = child.id().unwrap_or(0) as i32;

    let stdout = child.stdout.take().context("stdout missing")?;
    let stderr = child.stderr.take().context("stderr missing")?;

    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.log_file)
        .await
        .with_context(|| format!("open log file {:?}", args.log_file))?;
    let log_file = Arc::new(tokio::sync::Mutex::new(log_file));

    let log_out = log_file.clone();
    let log_err = log_file.clone();

    let mut out_task = Some(tokio::spawn(async move {
        pump_lines(stdout, "stdout", log_out).await;
    }));
    let mut err_task = Some(tokio::spawn(async move {
        pump_lines(stderr, "stderr", log_err).await;
    }));

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    let mut wait_task = tokio::spawn(async move { child.wait().await });

    tokio::select! {
        status = &mut wait_task => {
            let status = status.context("wait task")??;
            ensure_process_group_gone(pgid).await;
            if let Some(task) = out_task.take() {
                let _ = task.await;
            }
            if let Some(task) = err_task.take() {
                let _ = task.await;
            }
            let code = status.code().unwrap_or(1);
            std::process::exit(code);
        }
        _ = sigint.recv() => {
            let out_task = out_task.take().unwrap();
            let err_task = err_task.take().unwrap();
            shutdown_and_exit(pgid, &mut wait_task, out_task, err_task).await;
        }
        _ = sigterm.recv() => {
            let out_task = out_task.take().unwrap();
            let err_task = err_task.take().unwrap();
            shutdown_and_exit(pgid, &mut wait_task, out_task, err_task).await;
        }
    }
}

async fn shutdown_and_exit(
    pgid: i32,
    wait_task: &mut tokio::task::JoinHandle<std::io::Result<std::process::ExitStatus>>,
    out_task: tokio::task::JoinHandle<()>,
    err_task: tokio::task::JoinHandle<()>,
) -> ! {
    shutdown_process_group(pgid).await;

    // Reap the immediate child (best-effort). Even if it already exited, this ensures the
    // pipes close and the pump tasks can finish.
    let _ = wait_task.await;

    let _ = out_task.await;
    let _ = err_task.await;

    std::process::exit(0);
}

async fn shutdown_process_group(pgid: i32) {
    if pgid == 0 {
        return;
    }

    send_signal_to_pgid(pgid, libc::SIGTERM);
    let term_deadline = Instant::now() + Duration::from_secs(5);
    if wait_for_process_group_exit(pgid, term_deadline).await {
        return;
    }

    send_signal_to_pgid(pgid, libc::SIGKILL);
    let kill_deadline = Instant::now() + Duration::from_secs(1);
    let _ = wait_for_process_group_exit(pgid, kill_deadline).await;
}

async fn ensure_process_group_gone(pgid: i32) {
    if pgid == 0 {
        return;
    }

    if !process_group_exists(pgid) {
        return;
    }

    shutdown_process_group(pgid).await;

    // Verify no stragglers remain.
    let deadline = Instant::now() + Duration::from_secs(1);
    let _ = wait_for_process_group_exit(pgid, deadline).await;
}

fn send_signal_to_pgid(pgid: i32, signal: i32) {
    // SAFETY: `kill` is invoked with a negative process-group id that was captured from the
    // spawned child. This is the POSIX-defined way to signal all processes in that group.
    // Errors are intentionally ignored because shutdown is best-effort.
    unsafe {
        let _ = libc::kill(-pgid, signal);
    }
}

fn process_group_exists(pgid: i32) -> bool {
    // SAFETY: `kill(-pgid, 0)` performs a permission/existence probe without sending a signal.
    // This is a standard POSIX check for process-group liveness.
    unsafe {
        let rc = libc::kill(-pgid, 0);
        if rc == 0 {
            return true;
        }
    }
    let err = std::io::Error::last_os_error();
    err.raw_os_error() != Some(libc::ESRCH)
}

async fn wait_for_process_group_exit(pgid: i32, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        if !process_group_exists(pgid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    !process_group_exists(pgid)
}

fn encode_log_line(label: &str, line: &str, timestamp: &str) -> String {
    let content = line.trim_end_matches(['\r', '\n']);
    let mut payload = match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(map)) if content.trim_start().starts_with('{') => map,
        _ => {
            let mut map = serde_json::Map::new();
            map.insert("msg".to_string(), Value::String(content.to_string()));
            map
        }
    };

    payload.insert("time".to_string(), Value::String(timestamp.to_string()));
    payload.insert("stream".to_string(), Value::String(label.to_string()));

    serde_json::to_string(&Value::Object(payload)).unwrap_or_else(|_| {
        format!(
            "{{\"time\":\"{}\",\"stream\":\"{}\",\"msg\":\"{}\"}}",
            timestamp,
            label,
            content.replace('"', "\\\"")
        )
    })
}

async fn pump_lines<R>(reader: R, label: &str, log_file: Arc<tokio::sync::Mutex<tokio::fs::File>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).await.unwrap_or(0);
        if bytes == 0 {
            break;
        }
        let clean = crate::logfmt::strip_ansi_if_needed(&line);
        let timestamp = now_rfc3339();
        let entry = encode_log_line(label, &clean, &timestamp);
        let mut file = log_file.lock().await;
        let _ = file.write_all(entry.as_bytes()).await;
        let _ = file.write_all(b"\n").await;
        let _ = file.flush().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn plain_text_is_wrapped_as_json() {
        let encoded = encode_log_line("stdout", "server started\n", "2026-03-03T12:00:00Z");
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["time"], "2026-03-03T12:00:00Z");
        assert_eq!(value["stream"], "stdout");
        assert_eq!(value["msg"], "server started");
    }

    #[test]
    fn json_lines_merge_app_fields() {
        let encoded = encode_log_line(
            "stderr",
            r#"{"level":"error","msg":"failed","code":"ECONNREFUSED"}"#,
            "2026-03-03T12:00:00Z",
        );
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["time"], "2026-03-03T12:00:00Z");
        assert_eq!(value["stream"], "stderr");
        assert_eq!(value["level"], "error");
        assert_eq!(value["msg"], "failed");
        assert_eq!(value["code"], "ECONNREFUSED");
    }

    #[test]
    fn shim_time_overrides_app_time() {
        let encoded = encode_log_line(
            "stdout",
            r#"{"time":"old","msg":"hello"}"#,
            "2026-03-03T12:00:00Z",
        );
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["time"], "2026-03-03T12:00:00Z");
    }

    #[test]
    fn json_level_is_preserved() {
        let encoded = encode_log_line(
            "stdout",
            r#"{"level":"warn","msg":"heads up"}"#,
            "2026-03-03T12:00:00Z",
        );
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["level"], "warn");
    }

    #[test]
    fn malformed_json_is_treated_as_plain_text() {
        let broken = "{\"level\":\"error\"";
        let encoded = encode_log_line("stderr", broken, "2026-03-03T12:00:00Z");
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["stream"], "stderr");
        assert_eq!(value["msg"], broken);
        assert!(value.get("level").is_none());
    }

    #[tokio::test]
    async fn pump_lines_writes_one_json_object_per_line_and_strips_ansi() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.log");
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .unwrap();
        let log_file = Arc::new(tokio::sync::Mutex::new(file));

        let input = Cursor::new("\u{1b}[31mfirst\u{1b}[0m\nsecond\n");
        pump_lines(input, "stdout", log_file).await;

        let content = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: Value = serde_json::from_str(lines[0]).unwrap();
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["msg"], "first");
        assert_eq!(second["msg"], "second");
    }
}

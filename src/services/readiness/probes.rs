use anyhow::{Context, Result, anyhow};
use http_body_util::Full;
use hyper::{Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use notify::{RecursiveMode, Watcher};
use regex::Regex;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::{Instant, sleep};

use super::model::ReadinessContext;

pub(crate) async fn tcp_ready(port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_ok()
}

pub(crate) async fn http_ready(port: u16, path: &str, min: u16, max: u16) -> Result<bool> {
    let client = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let req = Request::get(&url).body(Full::new(hyper::body::Bytes::new()))?;
    let resp = client.request(req).await?;
    let status = resp.status();
    Ok(is_success_status(status, min, max))
}

pub(crate) fn log_regex_ready(path: &Path, pattern: &str) -> Result<bool> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    let regex = Regex::new(pattern).context("compile log regex")?;
    Ok(regex.is_match(&content))
}

pub async fn wait_for_log_regex(path: &Path, pattern: &str, timeout: Duration) -> Result<()> {
    let regex = Regex::new(pattern).context("compile regex")?;
    let deadline = Instant::now() + timeout;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.blocking_send(());
        }
    })
    .ok();

    if let Some(watcher) = watcher.as_mut() {
        let watch_path = if path.exists() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let _ = watcher.watch(watch_path.as_path(), RecursiveMode::NonRecursive);
    }

    loop {
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(path)
            && regex.is_match(&content)
        {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(anyhow!("readiness timed out"));
        }

        tokio::select! {
            _ = sleep(Duration::from_millis(100)) => {}
            _ = rx.recv() => {}
        }
    }
}

pub(crate) async fn cmd_ready(command: &str, ctx: &ReadinessContext) -> Result<bool> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(&ctx.cwd);
    for (k, v) in &ctx.env {
        cmd.env(k, v);
    }
    let output = cmd.output().await.context("run readiness command")?;
    Ok(output.status.success())
}

pub async fn wait_for_delay(duration: Duration, timeout: Duration) -> Result<()> {
    if duration > timeout {
        return Err(anyhow!("readiness timed out"));
    }
    sleep(duration).await;
    Ok(())
}

pub async fn wait_for_exit_success(ctx: &ReadinessContext, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_err: Option<anyhow::Error> = None;
    loop {
        match exit_ready_once(ctx).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) => {
                last_err = Some(err);
            }
        }
        if Instant::now() >= deadline {
            if let Some(err) = last_err {
                return Err(anyhow!("readiness timed out: {err}"));
            }
            return Err(anyhow!("readiness timed out"));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

pub async fn exit_ready_once(ctx: &ReadinessContext) -> Result<bool> {
    let systemd = ctx
        .systemd
        .as_ref()
        .ok_or_else(|| anyhow!("exit readiness requires systemd"))?;
    let unit_name = ctx
        .unit_name
        .as_ref()
        .ok_or_else(|| anyhow!("exit readiness requires unit name"))?;
    let status = systemd.unit_status(unit_name).await?;
    let status = match status {
        Some(status) => status,
        None => return Ok(false),
    };
    if status.active_state == "failed" {
        return Err(anyhow!(
            "service failed (result={})",
            status.result.unwrap_or_else(|| "unknown".to_string())
        ));
    }
    if status.active_state == "active"
        && status.sub_state == "exited"
        && matches!(status.result.as_deref(), Some("success") | None)
    {
        return Ok(true);
    }
    if status.active_state == "inactive" {
        if matches!(status.result.as_deref(), Some("success")) {
            return Ok(true);
        }
        if matches!(status.sub_state.as_str(), "exited" | "dead") && status.result.is_none() {
            return Ok(true);
        }
        if let Some(result) = status.result {
            return Err(anyhow!("service exited (result={result})"));
        }
    }
    Ok(false)
}

pub fn readiness_url(scheme: &str, port: u16) -> String {
    match scheme {
        "https" => format!("https://localhost:{}", port),
        _ => format!("http://localhost:{}", port),
    }
}

pub fn is_success_status(code: StatusCode, min: u16, max: u16) -> bool {
    let code_u16 = code.as_u16();
    code_u16 >= min && code_u16 <= max
}

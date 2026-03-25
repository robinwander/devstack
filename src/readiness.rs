use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use http_body_util::Full;
use hyper::{Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use notify::{RecursiveMode, Watcher};
use regex::Regex;
use tokio::process::Command;
use tokio::time::{Instant, sleep};

use crate::systemd::SystemdManager;

#[derive(Clone, Debug)]
pub enum ReadinessKind {
    Tcp,
    Http {
        path: String,
        expect_min: u16,
        expect_max: u16,
    },
    LogRegex {
        pattern: String,
    },
    Cmd {
        command: String,
    },
    Delay {
        duration: Duration,
    },
    Exit,
    None,
}

#[derive(Clone, Debug)]
pub struct ReadinessSpec {
    pub kind: ReadinessKind,
    pub timeout: Duration,
}

impl ReadinessSpec {
    pub fn new(kind: ReadinessKind) -> Self {
        Self {
            kind,
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone)]
pub struct ReadinessContext {
    pub port: Option<u16>,
    pub scheme: String,
    pub log_path: PathBuf,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub unit_name: Option<String>,
    pub systemd: Option<Arc<dyn SystemdManager>>,
}

pub async fn wait_for_ready(spec: &ReadinessSpec, ctx: &ReadinessContext) -> Result<()> {
    if let ReadinessKind::Delay { duration } = &spec.kind {
        return wait_for_delay(*duration, spec.timeout).await;
    }
    if let ReadinessKind::Exit = &spec.kind {
        return wait_for_exit_success(ctx, spec.timeout).await;
    }
    if let ReadinessKind::LogRegex { pattern } = &spec.kind {
        return wait_for_log_regex(&ctx.log_path, pattern, spec.timeout).await;
    }

    let deadline = Instant::now() + spec.timeout;
    let mut last_err: Option<anyhow::Error> = None;
    loop {
        if let Some(reason) = readiness_process_failure(ctx).await? {
            return Err(anyhow!(reason));
        }

        match check_ready_once(spec, ctx).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) => {
                last_err = Some(err);
            }
        }

        if let Some(reason) = readiness_process_failure(ctx).await? {
            return Err(anyhow!(reason));
        }

        if Instant::now() >= deadline {
            if let Some(err) = last_err {
                return Err(anyhow!("readiness timed out: {err}"));
            }
            return Err(anyhow!("readiness timed out"));
        }
        sleep(Duration::from_millis(200)).await;
    }
}

pub async fn check_ready_once(spec: &ReadinessSpec, ctx: &ReadinessContext) -> Result<bool> {
    match &spec.kind {
        ReadinessKind::Tcp => {
            let port = ctx.port.context("tcp readiness requires port")?;
            let ok = tcp_ready(port).await;
            if ok && let Err(binding_err) = verify_port_binding(port, ctx).await {
                let fallback_ok = tcp_ready(port).await;
                if !fallback_ok {
                    return Err(binding_err);
                }
            }
            Ok(ok)
        }
        ReadinessKind::Http {
            path,
            expect_min,
            expect_max,
        } => {
            let port = ctx.port.context("http readiness requires port")?;
            let ok = http_ready(port, path, *expect_min, *expect_max).await?;
            if ok && let Err(binding_err) = verify_port_binding(port, ctx).await {
                // Some servers (e.g. uvicorn/vite with worker/forked children)
                // bind from child processes. If HTTP readiness is still proven on
                // the expected port, accept readiness and avoid false negatives.
                let fallback_ok = http_ready(port, path, *expect_min, *expect_max)
                    .await
                    .unwrap_or(false);
                if !fallback_ok {
                    return Err(binding_err);
                }
            }
            Ok(ok)
        }
        ReadinessKind::LogRegex { pattern } => Ok(log_regex_ready(&ctx.log_path, pattern)?),
        ReadinessKind::Cmd { command } => Ok(cmd_ready(command, ctx).await?),
        ReadinessKind::Delay { .. } => Ok(true),
        ReadinessKind::Exit => exit_ready_once(ctx).await,
        ReadinessKind::None => Ok(true),
    }
}

async fn readiness_process_failure(ctx: &ReadinessContext) -> Result<Option<String>> {
    let Some(systemd) = ctx.systemd.as_ref() else {
        return Ok(None);
    };
    let Some(unit_name) = ctx.unit_name.as_deref() else {
        return Ok(None);
    };

    let Some(status) = systemd.unit_status(unit_name).await? else {
        return Ok(None);
    };

    if status.active_state == "failed" {
        let result = status.result.unwrap_or_else(|| "unknown".to_string());
        return Ok(Some(format!(
            "service exited before readiness (active_state=failed, sub_state={}, result={result})",
            status.sub_state
        )));
    }

    if status.active_state == "inactive" {
        if status.result.as_deref() == Some("success") {
            return Ok(None);
        }
        if let Some(result) = status.result {
            return Ok(Some(format!(
                "service exited before readiness (active_state=inactive, sub_state={}, result={result})",
                status.sub_state
            )));
        }
    }

    Ok(None)
}

async fn tcp_ready(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    tokio::net::TcpStream::connect(addr).await.is_ok()
}

async fn http_ready(port: u16, path: &str, min: u16, max: u16) -> Result<bool> {
    let uri = format!("http://127.0.0.1:{port}{path}");
    let connector = HttpConnector::new();
    let client: Client<_, Full<hyper::body::Bytes>> =
        Client::builder(TokioExecutor::new()).build(connector);
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Full::new(hyper::body::Bytes::new()))?;
    let response = client.request(request).await?;
    let status = response.status();
    Ok(status.as_u16() >= min && status.as_u16() <= max)
}

fn log_regex_ready(path: &Path, pattern: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let regex = Regex::new(pattern).context("compile regex")?;
    let content = std::fs::read_to_string(path).unwrap_or_default();
    Ok(regex.is_match(&content))
}

async fn wait_for_log_regex(path: &Path, pattern: &str, timeout: Duration) -> Result<()> {
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
            _ = rx.recv() => {}
            _ = sleep(Duration::from_millis(200)) => {}
        }
    }
}

async fn cmd_ready(command: &str, ctx: &ReadinessContext) -> Result<bool> {
    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-lc").arg(command).current_dir(&ctx.cwd);
    cmd.envs(&ctx.env);
    let status = cmd.status().await.context("run readiness cmd")?;
    Ok(status.success())
}

async fn wait_for_delay(duration: Duration, timeout: Duration) -> Result<()> {
    if duration > timeout {
        return Err(anyhow!("readiness timed out"));
    }
    sleep(duration).await;
    Ok(())
}

async fn wait_for_exit_success(ctx: &ReadinessContext, timeout: Duration) -> Result<()> {
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
        sleep(Duration::from_millis(200)).await;
    }
}

async fn exit_ready_once(ctx: &ReadinessContext) -> Result<bool> {
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
        "ws" => format!("ws://localhost:{port}"),
        _ => format!("http://localhost:{port}"),
    }
}

pub fn is_success_status(code: StatusCode, min: u16, max: u16) -> bool {
    let v = code.as_u16();
    v >= min && v <= max
}

#[derive(Clone, Debug)]
pub(crate) struct PortBindingInfo {
    pub listening_pids: Vec<u32>,
    pub owned_by_unit: Option<bool>,
    pub has_listener: bool,
    pub probe_supported: bool,
}

async fn verify_port_binding(port: u16, ctx: &ReadinessContext) -> Result<()> {
    let Some(unit_name) = ctx.unit_name.as_deref() else {
        return Ok(());
    };

    let info = port_binding_info(port, Some(unit_name)).await?;
    if info.owned_by_unit.unwrap_or(true) {
        return Ok(());
    }
    Err(anyhow!(
        "expected port {port} is not bound by {unit_name} (listening_pids={:?})",
        info.listening_pids
    ))
}

pub(crate) async fn port_binding_info(
    port: u16,
    unit_name: Option<&str>,
) -> Result<PortBindingInfo> {
    #[cfg(target_os = "linux")]
    {
        let unit = unit_name.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || linux_port_binding_info(port, unit.as_deref()))
            .await
            .context("port binding task")?
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = unit_name;
        Ok(PortBindingInfo {
            listening_pids: Vec::new(),
            owned_by_unit: None,
            has_listener: false,
            probe_supported: false,
        })
    }
}

#[cfg(target_os = "linux")]
fn linux_port_binding_info(port: u16, unit_name: Option<&str>) -> Result<PortBindingInfo> {
    let inodes = linux_listen_inodes_for_port(port)?;
    let mut has_listener = !inodes.is_empty();
    let mut listening_pids = linux_pids_for_inodes(&inodes);

    if let Some((ss_has_listener, ss_pids)) = linux_port_binding_info_from_ss(port) {
        has_listener |= ss_has_listener;
        if listening_pids.is_empty() {
            listening_pids = ss_pids;
        } else {
            listening_pids.extend(ss_pids);
            listening_pids.sort_unstable();
            listening_pids.dedup();
        }
    }

    let owned_by_unit = if listening_pids.is_empty() {
        None
    } else {
        unit_name.map(|unit| {
            let control_group = linux_unit_control_group(unit);
            listening_pids
                .iter()
                .any(|pid| pid_in_unit_cgroup(*pid, unit, control_group.as_deref()))
        })
    };
    Ok(PortBindingInfo {
        listening_pids,
        owned_by_unit,
        has_listener,
        probe_supported: true,
    })
}

#[cfg(target_os = "linux")]
fn linux_listen_inodes_for_port(port: u16) -> Result<HashSet<u64>> {
    let mut inodes = HashSet::new();
    linux_collect_inodes_from_proc_net("/proc/net/tcp", port, &mut inodes);
    linux_collect_inodes_from_proc_net("/proc/net/tcp6", port, &mut inodes);
    Ok(inodes)
}

#[cfg(target_os = "linux")]
fn linux_collect_inodes_from_proc_net(path: &str, port: u16, into: &mut HashSet<u64>) {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines = content.lines();
    let Some(header) = lines.next() else {
        return;
    };

    let cols: Vec<&str> = header.split_whitespace().collect();
    let local_idx = cols.iter().position(|c| *c == "local_address").unwrap_or(1);
    let st_idx = cols.iter().position(|c| *c == "st").unwrap_or(3);
    let inode_idx = cols.iter().position(|c| *c == "inode").unwrap_or(9);

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() <= inode_idx {
            continue;
        }
        if parts.get(st_idx) != Some(&"0A") {
            continue;
        }
        let local = match parts.get(local_idx) {
            Some(v) => *v,
            None => continue,
        };
        let Some((_, port_hex)) = local.split_once(':') else {
            continue;
        };
        let Ok(found_port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if found_port != port {
            continue;
        }
        if let Ok(inode) = parts[inode_idx].parse::<u64>() {
            into.insert(inode);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_pids_for_inodes(inodes: &HashSet<u64>) -> Vec<u32> {
    if inodes.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
                continue;
            };
            let fd_dir = entry.path().join("fd");
            let Ok(fds) = std::fs::read_dir(fd_dir) else {
                continue;
            };
            for fd in fds.flatten() {
                let Ok(target) = std::fs::read_link(fd.path()) else {
                    continue;
                };
                let target = target.to_string_lossy();
                let Some(inode_str) = target
                    .strip_prefix("socket:[")
                    .and_then(|s| s.strip_suffix("]"))
                else {
                    continue;
                };
                let Ok(inode) = inode_str.parse::<u64>() else {
                    continue;
                };
                if inodes.contains(&inode) {
                    out.push(pid);
                    break;
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(target_os = "linux")]
fn linux_port_binding_info_from_ss(port: u16) -> Option<(bool, Vec<u32>)> {
    let output = std::process::Command::new("ss")
        .arg("-H")
        .arg("-ltnp")
        .arg(format!("sport = :{port}"))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(parse_ss_output_for_pids(&stdout))
}

#[cfg(target_os = "linux")]
fn parse_ss_output_for_pids(output: &str) -> (bool, Vec<u32>) {
    let mut has_listener = false;
    let mut pids = Vec::new();

    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        has_listener = true;

        let mut rest = line;
        while let Some(idx) = rest.find("pid=") {
            let digits = rest[idx + 4..]
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if let Ok(pid) = digits.parse::<u32>() {
                pids.push(pid);
            }
            rest = &rest[idx + 4 + digits.len()..];
        }
    }

    pids.sort_unstable();
    pids.dedup();
    (has_listener, pids)
}

#[cfg(target_os = "linux")]
fn linux_unit_control_group(unit_name: &str) -> Option<String> {
    let output = std::process::Command::new("systemctl")
        .arg("--user")
        .arg("show")
        .arg(unit_name)
        .arg("--property=ControlGroup")
        .arg("--value")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let group = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if group.is_empty() || group == "-" {
        return None;
    }
    Some(group)
}

#[cfg(target_os = "linux")]
fn pid_in_unit_cgroup(pid: u32, unit_name: &str, control_group: Option<&str>) -> bool {
    let path = format!("/proc/{pid}/cgroup");
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };

    if cgroup_content_mentions_unit(&content, unit_name) {
        return true;
    }

    if let Some(control_group) = control_group
        && cgroup_content_has_path(&content, control_group)
    {
        return true;
    }

    false
}

#[cfg(target_os = "linux")]
fn cgroup_content_mentions_unit(content: &str, unit_name: &str) -> bool {
    content
        .lines()
        .filter_map(cgroup_path_from_line)
        .any(|path| path.contains(unit_name))
}

#[cfg(target_os = "linux")]
fn cgroup_content_has_path(content: &str, target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() || target == "-" {
        return false;
    }
    let target_prefix = format!("{target}/");

    content
        .lines()
        .filter_map(cgroup_path_from_line)
        .any(|path| path == target || path.starts_with(&target_prefix))
}

#[cfg(target_os = "linux")]
fn cgroup_path_from_line(line: &str) -> Option<&str> {
    let mut parts = line.splitn(3, ':');
    parts.next()?;
    parts.next()?;
    parts.next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use http_body_util::Full;
    use hyper::{Response, service::service_fn};
    use hyper_util::rt::TokioIo;
    use std::convert::Infallible;
    use std::net::SocketAddr;

    #[tokio::test]
    async fn tcp_ready_true_for_open_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let ready = tcp_ready(port).await;
        assert!(ready);
        drop(listener);
    }

    #[tokio::test]
    async fn http_ready_accepts_status_range() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let service = service_fn(|_req| async move {
                    Ok::<_, Infallible>(Response::new(Full::new(hyper::body::Bytes::new())))
                });
                let io = TokioIo::new(stream);
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            };
        });

        let ok = http_ready(addr.port(), "/", 200, 299).await.unwrap();
        assert!(ok);
    }

    #[tokio::test]
    async fn log_regex_ready_detects_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("svc.log");
        std::fs::write(&path, "service listening on port").unwrap();
        let ok = log_regex_ready(&path, "listening").unwrap();
        assert!(ok);
    }

    #[tokio::test]
    async fn cmd_ready_true_on_success() {
        let ctx = ReadinessContext {
            port: None,
            scheme: "http".to_string(),
            log_path: PathBuf::from("/tmp/nowhere"),
            cwd: PathBuf::from("/"),
            env: BTreeMap::new(),
            unit_name: None,
            systemd: None,
        };
        let ok = cmd_ready("exit 0", &ctx).await.unwrap();
        assert!(ok);
        let ok = cmd_ready("exit 2", &ctx).await.unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn http_readiness_falls_back_when_port_ownership_check_fails() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let service = service_fn(|_req| async move {
                        Ok::<_, Infallible>(Response::new(Full::new(hyper::body::Bytes::new())))
                    });
                    let io = TokioIo::new(stream);
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await;
                });
            }
        });

        let spec = ReadinessSpec {
            kind: ReadinessKind::Http {
                path: "/".to_string(),
                expect_min: 200,
                expect_max: 299,
            },
            timeout: Duration::from_secs(2),
        };

        let ctx = ReadinessContext {
            port: Some(addr.port()),
            scheme: "http".to_string(),
            log_path: PathBuf::from("/tmp/nowhere"),
            cwd: PathBuf::from("/"),
            env: BTreeMap::new(),
            unit_name: Some("devstack-run-FAKE-UNIT.service".to_string()),
            systemd: None,
        };

        let ok = check_ready_once(&spec, &ctx).await.unwrap();
        assert!(ok);
    }

    #[derive(Clone)]
    struct FailedSystemd;

    #[async_trait]
    impl SystemdManager for FailedSystemd {
        async fn start_transient_service(
            &self,
            _unit_name: &str,
            _props: crate::systemd::UnitProperties,
        ) -> Result<()> {
            Ok(())
        }

        async fn stop_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn restart_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn kill_unit(&self, _unit_name: &str, _signal: i32) -> Result<()> {
            Ok(())
        }

        async fn unit_status(
            &self,
            _unit_name: &str,
        ) -> Result<Option<crate::systemd::UnitStatus>> {
            Ok(Some(crate::systemd::UnitStatus {
                active_state: "failed".to_string(),
                sub_state: "failed".to_string(),
                result: Some("exit-code".to_string()),
            }))
        }
    }

    #[derive(Clone)]
    struct ActiveExitedSystemd;

    #[async_trait]
    impl SystemdManager for ActiveExitedSystemd {
        async fn start_transient_service(
            &self,
            _unit_name: &str,
            _props: crate::systemd::UnitProperties,
        ) -> Result<()> {
            Ok(())
        }

        async fn stop_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn restart_unit(&self, _unit_name: &str) -> Result<()> {
            Ok(())
        }

        async fn kill_unit(&self, _unit_name: &str, _signal: i32) -> Result<()> {
            Ok(())
        }

        async fn unit_status(
            &self,
            _unit_name: &str,
        ) -> Result<Option<crate::systemd::UnitStatus>> {
            Ok(Some(crate::systemd::UnitStatus {
                active_state: "active".to_string(),
                sub_state: "exited".to_string(),
                result: Some("success".to_string()),
            }))
        }
    }

    #[tokio::test]
    async fn wait_for_exit_ready_accepts_active_exited_state() {
        let spec = ReadinessSpec {
            kind: ReadinessKind::Exit,
            timeout: Duration::from_secs(1),
        };
        let ctx = ReadinessContext {
            port: None,
            scheme: "http".to_string(),
            log_path: PathBuf::from("/tmp/nowhere"),
            cwd: PathBuf::from("/"),
            env: BTreeMap::new(),
            unit_name: Some("devstack-run-test-migrate.service".to_string()),
            systemd: Some(Arc::new(ActiveExitedSystemd)),
        };

        wait_for_ready(&spec, &ctx).await.unwrap();
    }

    #[tokio::test]
    async fn wait_for_ready_reports_exit_instead_of_timeout() {
        let spec = ReadinessSpec {
            kind: ReadinessKind::Tcp,
            timeout: Duration::from_secs(1),
        };
        let ctx = ReadinessContext {
            port: Some(9),
            scheme: "http".to_string(),
            log_path: PathBuf::from("/tmp/nowhere"),
            cwd: PathBuf::from("/"),
            env: BTreeMap::new(),
            unit_name: Some("devstack-run-test-api.service".to_string()),
            systemd: Some(Arc::new(FailedSystemd)),
        };

        let err = wait_for_ready(&spec, &ctx).await.unwrap_err().to_string();
        assert!(err.contains("exited before readiness"));
        assert!(!err.contains("timed out"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_path_match_accepts_exact_or_descendant() {
        let content = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/devstack-run-APP-API.service\n";
        assert!(cgroup_content_has_path(
            content,
            "/user.slice/user-1000.slice/user@1000.service/app.slice/devstack-run-APP-API.service"
        ));

        let child_content = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/devstack-run-APP-API.service/scope\n";
        assert!(cgroup_content_has_path(
            child_content,
            "/user.slice/user-1000.slice/user@1000.service/app.slice/devstack-run-APP-API.service"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_path_match_rejects_similar_prefix() {
        let content = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/devstack-run-APP-API2.service\n";
        assert!(!cgroup_content_has_path(
            content,
            "/user.slice/user-1000.slice/user@1000.service/app.slice/devstack-run-APP-API.service"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_ss_output_extracts_listener_and_pids() {
        let output = "LISTEN 0 4096 127.0.0.1:3000 0.0.0.0:* users:((\"node\",pid=123,fd=10),(\"node\",pid=456,fd=11))\n";
        let (has_listener, pids) = parse_ss_output_for_pids(output);
        assert!(has_listener);
        assert_eq!(pids, vec![123, 456]);
    }
}

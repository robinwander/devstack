use std::collections::HashMap;
use std::io::ErrorKind;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::io::RawFd;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use pty_process::{Command as PtyCommand, Size};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::api::{
    AgentSessionPollResponse, AgentSessionRegisterRequest, LogsResponse, RunListResponse,
    RunStatusResponse,
};
use crate::manifest::RunLifecycle;
use crate::paths;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const AUTO_SHARE_POLL_INTERVAL: Duration = Duration::from_secs(2);
const AUTO_SHARE_RATE_LIMIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct AgentCommandArgs {
    pub auto_share: Option<String>,
    pub no_auto_share: bool,
    pub watch: Vec<String>,
    pub run_id: Option<String>,
    pub command: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoShareLevel {
    Error,
    Warn,
}

#[derive(Clone, Debug)]
struct AutoShareConfig {
    level: AutoShareLevel,
    run_id: String,
    services: Vec<String>,
}

struct ProxyIo {
    stdin: Pin<Box<dyn AsyncRead + Send>>,
    stdout: Pin<Box<dyn AsyncWrite + Send>>,
    terminal_fd: Option<RawFd>,
}

impl ProxyIo {
    fn stdio() -> Self {
        let terminal_fd = if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
            Some(libc::STDIN_FILENO)
        } else {
            None
        };

        Self {
            stdin: Box::pin(tokio::io::stdin()),
            stdout: Box::pin(tokio::io::stdout()),
            terminal_fd,
        }
    }
}

#[async_trait]
trait AgentDaemonClient: Send + Sync {
    async fn register_session(&self, request: AgentSessionRegisterRequest) -> Result<()>;
    async fn unregister_session(&self, agent_id: &str) -> Result<()>;
    async fn poll_messages(&self, agent_id: &str) -> Result<Vec<String>>;
}

#[derive(Default)]
struct HttpAgentDaemonClient;

#[async_trait]
impl AgentDaemonClient for HttpAgentDaemonClient {
    async fn register_session(&self, request: AgentSessionRegisterRequest) -> Result<()> {
        let _ = call_daemon("POST", "/v1/agent/sessions", Some(request)).await?;
        Ok(())
    }

    async fn unregister_session(&self, agent_id: &str) -> Result<()> {
        let _ = call_daemon::<serde_json::Value>(
            "DELETE",
            &format!("/v1/agent/sessions/{agent_id}"),
            None,
        )
        .await?;
        Ok(())
    }

    async fn poll_messages(&self, agent_id: &str) -> Result<Vec<String>> {
        let response = call_daemon::<serde_json::Value>(
            "GET",
            &format!("/v1/agent/sessions/{agent_id}/messages/poll"),
            None,
        )
        .await?;
        let body: AgentSessionPollResponse = serde_json::from_value(response)?;
        Ok(body.messages)
    }
}

pub async fn run(args: AgentCommandArgs) -> Result<i32> {
    let AgentCommandArgs {
        auto_share,
        no_auto_share,
        watch,
        run_id,
        command,
    } = args;

    let auto_share_level = resolve_auto_share(auto_share.as_deref(), no_auto_share)?;
    let auto_share_config = configure_auto_share(auto_share_level, run_id, watch).await;

    let agent_id = generate_agent_id();
    let daemon = Arc::new(HttpAgentDaemonClient);
    run_proxy(
        command,
        agent_id,
        daemon,
        ProxyIo::stdio(),
        auto_share_config,
    )
    .await
}

async fn configure_auto_share(
    level: Option<AutoShareLevel>,
    run_id: Option<String>,
    watch: Vec<String>,
) -> Option<AutoShareConfig> {
    let level = level?;

    let run_id = if let Some(run_id) = run_id {
        run_id
    } else {
        match resolve_latest_run_id_for_cwd().await {
            Ok(Some(run_id)) => run_id,
            Ok(None) => {
                eprintln!("warning: auto-share disabled (no active run found for current project)");
                return None;
            }
            Err(err) => {
                eprintln!("warning: auto-share disabled (failed to resolve run id): {err}");
                return None;
            }
        }
    };

    let services = if watch.is_empty() {
        match fetch_run_services(&run_id).await {
            Ok(services) if !services.is_empty() => services,
            Ok(_) => {
                eprintln!("warning: auto-share disabled (no services found for run {run_id})");
                return None;
            }
            Err(err) => {
                eprintln!(
                    "warning: auto-share disabled (failed to load services for run {run_id}): {err}"
                );
                return None;
            }
        }
    } else {
        watch
    };

    Some(AutoShareConfig {
        level,
        run_id,
        services,
    })
}

async fn resolve_latest_run_id_for_cwd() -> Result<Option<String>> {
    let response = call_daemon::<serde_json::Value>("GET", "/v1/runs", None).await?;
    let runs: RunListResponse = serde_json::from_value(response)?;
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    Ok(runs
        .runs
        .into_iter()
        .filter(|run| run.project_dir == cwd && run.state != RunLifecycle::Stopped)
        .max_by(|a, b| a.created_at.cmp(&b.created_at))
        .map(|run| run.run_id))
}

async fn fetch_run_services(run_id: &str) -> Result<Vec<String>> {
    let response =
        call_daemon::<serde_json::Value>("GET", &format!("/v1/runs/{run_id}/status"), None).await?;
    let status: RunStatusResponse = serde_json::from_value(response)?;
    let mut services: Vec<String> = status.services.keys().cloned().collect();
    services.sort();
    Ok(services)
}

async fn fetch_service_logs_after(run_id: &str, service: &str, after: u64) -> Result<LogsResponse> {
    let run_id = urlencoding::encode(run_id);
    let service = urlencoding::encode(service);
    let path = format!("/v1/runs/{run_id}/logs/{service}?after={after}&last=200");
    let response = call_daemon::<serde_json::Value>("GET", &path, None).await?;
    Ok(serde_json::from_value(response)?)
}

async fn fetch_latest_service_cursor(run_id: &str, service: &str) -> Result<u64> {
    let run_id = urlencoding::encode(run_id);
    let service = urlencoding::encode(service);
    let path = format!("/v1/runs/{run_id}/logs/{service}?last=1");
    let response = call_daemon::<serde_json::Value>("GET", &path, None).await?;
    let logs: LogsResponse = serde_json::from_value(response)?;
    Ok(logs.next_after.unwrap_or(0))
}

fn threshold_matches(level: Option<&str>, threshold: AutoShareLevel) -> bool {
    match threshold {
        AutoShareLevel::Error => level == Some("error"),
        AutoShareLevel::Warn => matches!(level, Some("warn") | Some("error")),
    }
}

fn detect_trigger_level(
    service: &str,
    lines: &[String],
    threshold: AutoShareLevel,
) -> Option<AutoShareLevel> {
    let mut saw_warn = false;
    for line in lines {
        let level = crate::logs::structured_log_from_raw(service, line)
            .level
            .unwrap_or_else(|| "info".to_string());
        if level == "error" && threshold_matches(Some("error"), threshold) {
            return Some(AutoShareLevel::Error);
        }
        if level == "warn" {
            saw_warn = true;
        }
    }

    if saw_warn && threshold == AutoShareLevel::Warn {
        return Some(AutoShareLevel::Warn);
    }

    None
}

fn build_logs_command(run_id: &str, service: &str, level: AutoShareLevel) -> String {
    let level = match level {
        AutoShareLevel::Error => "error",
        AutoShareLevel::Warn => "warn",
    };

    format!(
        "devstack logs --run {run_id} --service {service} --level {level} --since 5m --last 200"
    )
}

fn build_auto_share_message(run_id: &str, service: &str, level: AutoShareLevel) -> String {
    let level_text = match level {
        AutoShareLevel::Error => "error",
        AutoShareLevel::Warn => "warn",
    };
    let command = build_logs_command(run_id, service, level);
    format!(
        "[devstack auto-share] Detected new {level_text} logs for {service}. Investigate with:\n{command}"
    )
}

fn should_emit_auto_share(
    last_sent: &mut HashMap<String, Instant>,
    service: &str,
    now: Instant,
) -> bool {
    if let Some(previous) = last_sent.get(service)
        && now.duration_since(*previous) < AUTO_SHARE_RATE_LIMIT
    {
        return false;
    }

    last_sent.insert(service.to_string(), now);
    true
}

fn build_auto_share_notification(
    config: &AutoShareConfig,
    service: &str,
    lines: &[String],
    last_sent: &mut HashMap<String, Instant>,
    now: Instant,
) -> Option<String> {
    if !config.services.iter().any(|candidate| candidate == service) {
        return None;
    }

    let trigger_level = detect_trigger_level(service, lines, config.level)?;
    if !should_emit_auto_share(last_sent, service, now) {
        return None;
    }

    Some(build_auto_share_message(
        &config.run_id,
        service,
        trigger_level,
    ))
}

async fn run_auto_share_monitor(
    config: AutoShareConfig,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) {
    let mut cursors: HashMap<String, u64> = HashMap::new();
    for service in &config.services {
        let cursor = fetch_latest_service_cursor(&config.run_id, service)
            .await
            .unwrap_or(0);
        cursors.insert(service.clone(), cursor);
    }

    let mut last_sent = HashMap::new();

    while !stop.load(Ordering::Relaxed) {
        for service in &config.services {
            let after = *cursors.get(service).unwrap_or(&0);
            let response = match fetch_service_logs_after(&config.run_id, service, after).await {
                Ok(response) => response,
                Err(_) => continue,
            };

            if let Some(next_after) = response.next_after {
                cursors.insert(service.clone(), next_after);
            }

            if let Some(message) = build_auto_share_notification(
                &config,
                service,
                &response.lines,
                &mut last_sent,
                Instant::now(),
            ) {
                let _ = tx.send(format_bracketed_paste(&message).into_bytes());
            }
        }

        tokio::time::sleep(AUTO_SHARE_POLL_INTERVAL).await;
    }
}

async fn run_proxy(
    command: Vec<String>,
    agent_id: String,
    daemon: Arc<dyn AgentDaemonClient>,
    io: ProxyIo,
    auto_share: Option<AutoShareConfig>,
) -> Result<i32> {
    let program = command
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("agent command is required"))?;

    let mut command_builder = PtyCommand::new(&program).args(command.iter().skip(1));
    command_builder = command_builder.env("DEVSTACK_AGENT_ID", &agent_id);

    let (pty, pts) = pty_process::open().context("open pty")?;
    if let Some(term_fd) = io.terminal_fd
        && let Some(size) = read_terminal_size(term_fd)?
    {
        pty.resize(size).context("set initial pty size")?;
    }
    let pty_fd = pty.as_raw_fd();

    let mut child = command_builder
        .spawn(pts)
        .context("spawn wrapped agent command")?;

    let child_pid = child.id().unwrap_or(std::process::id());
    let register_request = AgentSessionRegisterRequest {
        agent_id: agent_id.clone(),
        project_dir: std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        stack: None,
        command: command.join(" "),
        pid: child_pid,
    };
    let mut registered = true;
    if let Err(err) = daemon.register_session(register_request).await {
        registered = false;
        eprintln!("warning: failed to register agent session: {err}");
    }

    let terminal_guard = TerminalModeGuard::activate(io.terminal_fd)?;

    let (mut pty_reader, mut pty_writer) = pty.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let mut stdin = io.stdin;
    let stdin_tx = tx.clone();
    let stdin_task = tokio::spawn(async move {
        let mut buf = [0_u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,
                Ok(read) => {
                    if stdin_tx.send(buf[..read].to_vec()).is_err() {
                        break;
                    }
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    let writer_task = tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            if pty_writer.write_all(&chunk).await.is_err() {
                break;
            }
            if pty_writer.flush().await.is_err() {
                break;
            }
        }
    });

    let mut stdout = io.stdout;
    let reader_task = tokio::spawn(async move {
        let mut buf = [0_u8; 4096];
        loop {
            match pty_reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(read) => {
                    if stdout.write_all(&buf[..read]).await.is_err() {
                        break;
                    }
                    if stdout.flush().await.is_err() {
                        break;
                    }
                }
                Err(err)
                    if matches!(err.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) =>
                {
                    continue;
                }
                Err(_) => break,
            }
        }
        let _ = stdout.flush().await;
    });

    let stop = Arc::new(AtomicBool::new(false));

    let poll_task = if registered {
        let poll_client = daemon.clone();
        let poll_agent_id = agent_id.clone();
        let poll_tx = tx.clone();
        let poll_stop = stop.clone();
        Some(tokio::spawn(async move {
            while !poll_stop.load(Ordering::Relaxed) {
                if let Ok(messages) = poll_client.poll_messages(&poll_agent_id).await {
                    for message in messages {
                        let _ = poll_tx.send(format_bracketed_paste(&message).into_bytes());
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }))
    } else {
        None
    };

    let auto_share_task = auto_share.map(|config| {
        let share_tx = tx.clone();
        let share_stop = stop.clone();
        tokio::spawn(async move {
            run_auto_share_monitor(config, share_tx, share_stop).await;
        })
    });

    let winch_task = if let Some(term_fd) = io.terminal_fd {
        let winch_stop = stop.clone();
        Some(tokio::spawn(async move {
            let mut winch = match signal(SignalKind::window_change()) {
                Ok(signal) => signal,
                Err(_) => return,
            };
            while !winch_stop.load(Ordering::Relaxed) {
                if winch.recv().await.is_none() {
                    break;
                }
                if let Ok(winsize) = read_winsize(term_fd) {
                    let _ = set_winsize(pty_fd, winsize);
                }
            }
        }))
    } else {
        None
    };

    let mut sigterm = signal(SignalKind::terminate()).context("listen for SIGTERM")?;
    let status = tokio::select! {
        status = child.wait() => status.context("wait for wrapped command")?,
        _ = sigterm.recv() => {
            let _ = child.start_kill();
            child.wait().await.context("wait for wrapped command after SIGTERM")?
        }
    };

    stop.store(true, Ordering::Relaxed);
    stdin_task.abort();
    if let Some(task) = poll_task {
        task.abort();
    }
    if let Some(task) = auto_share_task {
        task.abort();
    }
    if let Some(task) = winch_task {
        task.abort();
    }
    drop(tx);

    let _ = writer_task.await;
    let _ = reader_task.await;

    if registered {
        let _ = daemon.unregister_session(&agent_id).await;
    }

    drop(terminal_guard);

    Ok(status.code().unwrap_or(1))
}

fn resolve_auto_share(
    auto_share: Option<&str>,
    no_auto_share: bool,
) -> Result<Option<AutoShareLevel>> {
    if no_auto_share {
        return Ok(None);
    }

    match auto_share.unwrap_or("error") {
        "error" => Ok(Some(AutoShareLevel::Error)),
        "warn" => Ok(Some(AutoShareLevel::Warn)),
        other => Err(anyhow!("invalid auto-share level: {other}")),
    }
}

fn format_bracketed_paste(message: &str) -> String {
    format!("\u{1b}[200~{message}\u{1b}[201~\n")
}

fn generate_agent_id() -> String {
    let mut rng = rand::rng();
    let mut suffix = String::new();
    for _ in 0..16 {
        suffix.push_str(&format!("{:x}", rand::Rng::random_range(&mut rng, 0..16)));
    }
    format!("agent-{suffix}")
}

fn read_terminal_size(fd: RawFd) -> Result<Option<Size>> {
    let mut winsize = read_winsize(fd)?;
    if winsize.ws_row == 0 {
        winsize.ws_row = 24;
    }
    if winsize.ws_col == 0 {
        winsize.ws_col = 80;
    }
    Ok(Some(Size::new(winsize.ws_row, winsize.ws_col)))
}

fn read_winsize(fd: RawFd) -> Result<libc::winsize> {
    let mut winsize = MaybeUninit::<libc::winsize>::zeroed();
    let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, winsize.as_mut_ptr()) };
    if rc == -1 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ENOTTY) {
            return Err(err).context("fd is not a terminal");
        }
        return Err(err).context("read terminal size");
    }

    Ok(unsafe { winsize.assume_init() })
}

fn set_winsize(fd: RawFd, winsize: libc::winsize) -> Result<()> {
    let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &winsize) };
    if rc == -1 {
        return Err(std::io::Error::last_os_error()).context("resize pty");
    }
    Ok(())
}

struct TerminalModeGuard {
    fd: RawFd,
    original: libc::termios,
    active: bool,
}

impl TerminalModeGuard {
    fn activate(fd: Option<RawFd>) -> Result<Option<Self>> {
        let Some(fd) = fd else {
            return Ok(None);
        };

        if unsafe { libc::isatty(fd) } != 1 {
            return Ok(None);
        }

        let mut original = MaybeUninit::<libc::termios>::zeroed();
        let rc = unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) };
        if rc == -1 {
            return Err(std::io::Error::last_os_error()).context("read terminal mode");
        }
        let original = unsafe { original.assume_init() };

        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        let rc = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };
        if rc == -1 {
            return Err(std::io::Error::last_os_error()).context("enable raw mode");
        }

        Ok(Some(Self {
            fd,
            original,
            active: true,
        }))
    }

    fn restore(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        let rc = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
        if rc == -1 {
            return Err(std::io::Error::last_os_error()).context("restore terminal mode");
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

async fn call_daemon<T: Serialize>(
    method: &str,
    path: &str,
    body: Option<T>,
) -> Result<serde_json::Value> {
    let socket_path = paths::daemon_socket_path()?;
    let stream = UnixStream::connect(&socket_path)
        .await
        .with_context(|| format!("connect to daemon socket {}", socket_path.display()))?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("handshake with daemon")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body_bytes = if let Some(payload) = body {
        serde_json::to_vec(&payload)?
    } else {
        Vec::new()
    };

    let request = Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("content-type", "application/json")
        .body(Full::new(hyper::body::Bytes::from(body_bytes)))?;

    let response = sender.send_request(request).await.context("send request")?;
    let status = response.status();
    let body = response.into_body().collect().await?.to_bytes();

    if !status.is_success() {
        return Err(anyhow!(
            "daemon request failed: {status} {}",
            String::from_utf8_lossy(&body)
        ));
    }

    if body.is_empty() {
        return Ok(serde_json::json!({}));
    }

    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct NoopDaemonClient;

    #[async_trait]
    impl AgentDaemonClient for NoopDaemonClient {
        async fn register_session(&self, _request: AgentSessionRegisterRequest) -> Result<()> {
            Ok(())
        }

        async fn unregister_session(&self, _agent_id: &str) -> Result<()> {
            Ok(())
        }

        async fn poll_messages(&self, _agent_id: &str) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn bracketed_paste_wraps_message() {
        let payload = format_bracketed_paste("hello world");
        assert_eq!(payload, "\u{1b}[200~hello world\u{1b}[201~\n");
    }

    #[test]
    fn no_auto_share_disables_monitoring() {
        assert_eq!(resolve_auto_share(None, true).unwrap(), None);
    }

    #[test]
    fn auto_share_detects_new_error_logs() {
        let lines = vec![
            "[2025-01-01T00:00:00Z] [stdout] booting".to_string(),
            "[2025-01-01T00:00:01Z] [stderr] Error: boom".to_string(),
        ];
        let level = detect_trigger_level("api", &lines, AutoShareLevel::Error);
        assert_eq!(level, Some(AutoShareLevel::Error));
    }

    #[test]
    fn auto_share_rate_limits_per_service() {
        let mut last_sent = HashMap::new();
        let now = Instant::now();

        assert!(should_emit_auto_share(&mut last_sent, "api", now));
        assert!(!should_emit_auto_share(
            &mut last_sent,
            "api",
            now + Duration::from_secs(10)
        ));
        assert!(should_emit_auto_share(
            &mut last_sent,
            "worker",
            now + Duration::from_secs(10)
        ));
        assert!(should_emit_auto_share(
            &mut last_sent,
            "api",
            now + Duration::from_secs(31)
        ));
    }

    #[test]
    fn auto_share_respects_watch_service_filter() {
        let config = AutoShareConfig {
            level: AutoShareLevel::Error,
            run_id: "run-1".to_string(),
            services: vec!["worker".to_string()],
        };
        let mut last_sent = HashMap::new();
        let lines = vec!["[2025-01-01T00:00:01Z] [stderr] Error: boom".to_string()];

        let ignored =
            build_auto_share_notification(&config, "api", &lines, &mut last_sent, Instant::now());
        assert_eq!(ignored, None);

        let included = build_auto_share_notification(
            &config,
            "worker",
            &lines,
            &mut last_sent,
            Instant::now() + Duration::from_secs(31),
        );
        assert!(included.is_some());
    }

    #[tokio::test]
    async fn wrapped_command_receives_agent_id_env_var() {
        let command = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf %s \"$DEVSTACK_AGENT_ID\"".to_string(),
        ];
        let agent_id = "agent-test-env".to_string();
        let (proxy_stdout, mut capture) = tokio::io::duplex(4096);

        let code = run_proxy(
            command,
            agent_id.clone(),
            Arc::new(NoopDaemonClient),
            ProxyIo {
                stdin: Box::pin(tokio::io::empty()),
                stdout: Box::pin(proxy_stdout),
                terminal_fd: None,
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(code, 0);
        let mut output = Vec::new();
        capture.read_to_end(&mut output).await.unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), agent_id);
    }

    #[tokio::test]
    async fn wrapped_command_exit_code_is_preserved() {
        let command = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "exit 17".to_string(),
        ];

        let code = run_proxy(
            command,
            "agent-test-exit".to_string(),
            Arc::new(NoopDaemonClient),
            ProxyIo {
                stdin: Box::pin(tokio::io::empty()),
                stdout: Box::pin(tokio::io::sink()),
                terminal_fd: None,
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(code, 17);
    }

    #[test]
    fn terminal_mode_is_restored_on_drop() {
        let (master_fd, slave_fd) = open_pty_pair();

        let original = terminal_lflag(slave_fd);
        {
            let _guard = TerminalModeGuard::activate(Some(slave_fd))
                .unwrap()
                .unwrap();
            let raw = terminal_lflag(slave_fd);
            assert_eq!(raw & libc::ICANON, 0);
            assert_eq!(raw & libc::ECHO, 0);
        }
        let restored = terminal_lflag(slave_fd);
        assert_eq!(restored & libc::ICANON, original & libc::ICANON);
        assert_eq!(restored & libc::ECHO, original & libc::ECHO);

        unsafe {
            libc::close(master_fd);
            libc::close(slave_fd);
        }
    }

    fn terminal_lflag(fd: RawFd) -> libc::tcflag_t {
        let mut termios = MaybeUninit::<libc::termios>::zeroed();
        let rc = unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) };
        assert_eq!(rc, 0);
        unsafe { termios.assume_init() }.c_lflag
    }

    fn open_pty_pair() -> (RawFd, RawFd) {
        let mut master = 0;
        let mut slave = 0;
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(rc, 0);
        (master, slave)
    }
}

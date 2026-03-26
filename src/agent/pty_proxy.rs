use std::io::ErrorKind;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::io::RawFd;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use pty_process::{Command as PtyCommand, Size};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::api::AgentSessionRegisterRequest;
use crate::infra::ipc::UnixDaemonClient;

use super::auto_share::{AutoShareConfig, run_auto_share_monitor};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) struct ProxyIo {
    pub(crate) stdin: Pin<Box<dyn AsyncRead + Send>>,
    pub(crate) stdout: Pin<Box<dyn AsyncWrite + Send>>,
    pub(crate) terminal_fd: Option<RawFd>,
}

impl ProxyIo {
    pub(crate) fn stdio() -> Self {
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
pub(crate) trait AgentSessionClient: Send + Sync {
    async fn register_session(&self, request: AgentSessionRegisterRequest) -> Result<()>;
    async fn unregister_session(&self, agent_id: &str) -> Result<()>;
    async fn poll_messages(&self, agent_id: &str) -> Result<Vec<String>>;
}

#[derive(Clone)]
pub(crate) struct UnixAgentSessionClient {
    daemon: UnixDaemonClient,
}

impl UnixAgentSessionClient {
    pub(crate) fn new(daemon: UnixDaemonClient) -> Self {
        Self { daemon }
    }
}

#[async_trait]
impl AgentSessionClient for UnixAgentSessionClient {
    async fn register_session(&self, request: AgentSessionRegisterRequest) -> Result<()> {
        let _ = self
            .daemon
            .request("POST", "/v1/agent/sessions", Some(request), None)
            .await?;
        Ok(())
    }

    async fn unregister_session(&self, agent_id: &str) -> Result<()> {
        let _ = self
            .daemon
            .request::<()>(
                "DELETE",
                &format!("/v1/agent/sessions/{agent_id}"),
                None,
                None,
            )
            .await?;
        Ok(())
    }

    async fn poll_messages(&self, agent_id: &str) -> Result<Vec<String>> {
        let response: crate::api::AgentSessionPollResponse = self
            .daemon
            .request_json(
                "GET",
                &format!("/v1/agent/sessions/{agent_id}/messages/poll"),
                None::<()>,
                None,
            )
            .await?;
        Ok(response.messages)
    }
}

pub(crate) async fn run_proxy(
    command: Vec<String>,
    agent_id: String,
    daemon: Arc<dyn AgentSessionClient>,
    io: ProxyIo,
    auto_share: Option<(AutoShareConfig, UnixDaemonClient)>,
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
                        let _ = poll_tx.send(format_message_for_pty(&message).into_bytes());
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }))
    } else {
        None
    };

    let auto_share_task = auto_share.map(|(config, daemon)| {
        let share_tx = tx.clone();
        let share_stop = stop.clone();
        tokio::spawn(async move {
            run_auto_share_monitor(daemon, config, share_tx, share_stop).await;
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

pub(crate) fn format_message_for_pty(message: &str) -> String {
    format!("{message}\r")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct NoopDaemonClient;

    #[async_trait]
    impl AgentSessionClient for NoopDaemonClient {
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
    fn format_message_appends_newline() {
        let payload = format_message_for_pty("hello world");
        assert_eq!(payload, "hello world\r");
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

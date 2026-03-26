use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tokio::sync::Mutex;

use crate::app::AppContext;
use crate::app::runtime::sync_port_reservations_from_disk;
use crate::infra::logs::index::LogIndex;
use crate::paths;
use crate::persistence::daemon_state::load_state_from_disk;
use crate::projects::ProjectsLedger;
use crate::stores::{AgentSessionStore, NavigationStore, RunStore, TaskStore};
use crate::systemd::SystemdManager;

#[cfg(unix)]
use crate::systemd::LocalSystemd;
#[cfg(target_os = "linux")]
use crate::systemd::RealSystemd;
#[cfg(target_os = "linux")]
use sd_notify::notify;

use super::log_tailing::RunLogTailRegistry;
use super::router::{DaemonState, build_router};

pub async fn run_daemon() -> Result<()> {
    paths::ensure_base_layout()?;
    let lock = acquire_daemon_lock()?;
    let systemd = build_process_manager().await?;
    let binary_path = std::env::current_exe().context("current_exe")?;
    let runs = Arc::new(RunStore::from_runs(load_state_from_disk()?));
    let tasks = Arc::new(TaskStore::new());
    let agent_sessions = Arc::new(AgentSessionStore::new());
    let navigation = Arc::new(NavigationStore::new());
    let log_index = Arc::new(LogIndex::open_or_create()?);
    let (event_tx, _) = tokio::sync::broadcast::channel(1024);

    let app = AppContext {
        systemd,
        runs,
        tasks,
        agent_sessions,
        navigation,
        binary_path,
        log_index,
        event_tx,
    };

    let state = DaemonState {
        app: app.clone(),
        log_tails: Arc::new(Mutex::new(RunLogTailRegistry::default())),
        _lock: lock,
    };

    sync_port_reservations_from_disk(&app).await?;

    if let Ok(runs_dir) = paths::runs_dir()
        && let Ok(mut ledger) = ProjectsLedger::load()
        && let Ok(count) = ledger.seed_from_runs(&runs_dir)
        && count > 0
    {
        eprintln!("[projects] seeded {} projects from existing runs", count);
    }

    let socket_path = paths::daemon_socket_path()?;
    clear_stale_socket(&socket_path).await?;
    let listener = tokio::net::UnixListener::bind(&socket_path)
        .with_context(|| format!("bind socket {socket_path:?}"))?;

    let app = build_router(state.clone());
    let dashboard_handle = spawn_dashboard().await;

    #[cfg(target_os = "linux")]
    let _ = notify(false, &[sd_notify::NotifyState::Ready]);

    axum::serve(listener, app).await?;

    if let Some(mut child) = dashboard_handle {
        let _ = child.kill().await;
    }

    Ok(())
}

fn acquire_daemon_lock() -> Result<Arc<std::fs::File>> {
    let lock_path = paths::daemon_lock_path()?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open daemon lock at {lock_path:?}"))?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(anyhow!("daemon already running"));
        }
        return Err(err).context("flock daemon lock");
    }
    Ok(Arc::new(file))
}

async fn ping_existing_daemon(stream: UnixStream) -> Result<bool> {
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("handshake with existing daemon")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let request = hyper::Request::builder()
        .method("GET")
        .uri("http://localhost/v1/ping")
        .body(Full::new(hyper::body::Bytes::new()))?;
    let response = sender.send_request(request).await?;
    Ok(response.status().is_success())
}

async fn clear_stale_socket(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }
    match UnixStream::connect(socket_path).await {
        Ok(stream) => {
            if ping_existing_daemon(stream).await.unwrap_or(false) {
                return Err(anyhow!("daemon already running"));
            }
        }
        Err(err) => {
            if err.kind() != std::io::ErrorKind::ConnectionRefused
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(err).context("probe existing daemon socket");
            }
        }
    }

    std::fs::remove_file(socket_path)
        .with_context(|| format!("remove stale socket {}", socket_path.display()))?;
    Ok(())
}

fn process_manager_override() -> Option<String> {
    std::env::var("DEVSTACK_PROCESS_MANAGER")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

async fn build_process_manager() -> Result<Arc<dyn SystemdManager>> {
    match process_manager_override().as_deref() {
        Some("local") => return Ok(Arc::new(LocalSystemd::new())),
        Some("systemd") => {
            #[cfg(not(target_os = "linux"))]
            {
                return Err(anyhow!(
                    "DEVSTACK_PROCESS_MANAGER=systemd is only supported on Linux"
                ));
            }
        }
        Some(other) => {
            return Err(anyhow!(
                "unsupported DEVSTACK_PROCESS_MANAGER value {other:?}; expected 'local' or 'systemd'"
            ));
        }
        None => {}
    }

    #[cfg(target_os = "linux")]
    {
        Ok(Arc::new(RealSystemd::connect().await?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Arc::new(LocalSystemd::new()))
    }
}

const DASHBOARD_PORT: u16 = 47832;

fn dashboard_disabled() -> bool {
    std::env::var("DEVSTACK_DISABLE_DASHBOARD")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

async fn spawn_dashboard() -> Option<tokio::process::Child> {
    if dashboard_disabled() {
        eprintln!("[dashboard] disabled by DEVSTACK_DISABLE_DASHBOARD");
        return None;
    }

    let dashboard_dir = match paths::dashboard_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("[dashboard] failed to get dashboard dir: {}", err);
            return None;
        }
    };
    if !dashboard_dir.join("package.json").exists() {
        eprintln!("[dashboard] no package.json found at {:?}, skipping", dashboard_dir);
        return None;
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let pnpm_paths = [
        format!("{home}/.local/share/pnpm/pnpm"),
        "/opt/homebrew/bin/pnpm".to_string(),
        "/usr/local/bin/pnpm".to_string(),
        "/usr/bin/pnpm".to_string(),
    ];
    let npm_paths = [
        format!("{home}/.local/share/fnm/aliases/default/bin/npm"),
        format!("{home}/.nvm/current/bin/npm"),
        "/opt/homebrew/bin/npm".to_string(),
        "/usr/local/bin/npm".to_string(),
        "/usr/bin/npm".to_string(),
    ];

    let (cmd, is_pnpm) = pnpm_paths
        .iter()
        .find(|path| Path::new(path.as_str()).exists())
        .map(|path| (path.clone(), true))
        .or_else(|| {
            npm_paths
                .iter()
                .find(|path| Path::new(path.as_str()).exists())
                .map(|path| (path.clone(), false))
        })
        .unwrap_or_else(|| {
            eprintln!("[dashboard] pnpm/npm not found in common paths, skipping dashboard");
            (String::new(), false)
        });

    if cmd.is_empty() {
        return None;
    }

    let port_str = DASHBOARD_PORT.to_string();
    let args: Vec<&str> = if is_pnpm {
        vec!["dev", "--port", &port_str, "--host", "0.0.0.0"]
    } else {
        vec!["run", "dev", "--", "--port", &port_str, "--host", "0.0.0.0"]
    };

    let log_path = match paths::daemon_dir() {
        Ok(dir) => dir.join("dashboard.log"),
        Err(_) => dashboard_dir.join("dashboard.log"),
    };
    let log_file = match std::fs::File::create(&log_path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("[dashboard] failed to create log file: {}", err);
            return None;
        }
    };
    let log_file_err = match log_file.try_clone() {
        Ok(file) => file,
        Err(err) => {
            eprintln!("[dashboard] failed to clone log file: {}", err);
            return None;
        }
    };

    let path_env = format!(
        "{home}/.local/share/pnpm:{home}/.local/share/fnm/aliases/default/bin:{home}/.nvm/current/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
    );

    match tokio::process::Command::new(&cmd)
        .args(&args)
        .current_dir(&dashboard_dir)
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err))
        .spawn()
    {
        Ok(child) => Some(child),
        Err(err) => {
            eprintln!("[dashboard] failed to start: {}", err);
            None
        }
    }
}

pub async fn doctor() -> Result<crate::api::DoctorResponse> {
    let mut checks = Vec::new();
    let daemon_ok = ping_daemon_socket().await;
    checks.push(crate::api::DoctorCheck {
        name: "daemon_socket".to_string(),
        ok: daemon_ok,
        message: if daemon_ok {
            "daemon socket present".to_string()
        } else {
            "daemon socket missing; run devstack daemon or devstack install".to_string()
        },
    });
    let base_ok = paths::ensure_base_layout().is_ok();
    checks.push(crate::api::DoctorCheck {
        name: "filesystem".to_string(),
        ok: base_ok,
        message: if base_ok {
            "filesystem layout ok".to_string()
        } else {
            "cannot create base directories".to_string()
        },
    });
    Ok(crate::api::DoctorResponse { checks })
}

async fn ping_daemon_socket() -> bool {
    let socket_path = match paths::daemon_socket_path() {
        Ok(path) => path,
        Err(_) => return false,
    };
    if !socket_path.exists() {
        return false;
    }
    let stream = match UnixStream::connect(socket_path).await {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    let io = TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(parts) => parts,
        Err(_) => return false,
    };
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let request = match hyper::Request::builder()
        .method("GET")
        .uri("http://localhost/v1/ping")
        .body(Full::new(hyper::body::Bytes::new()))
    {
        Ok(request) => request,
        Err(_) => return false,
    };
    let response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(_) => return false,
    };
    if !response.status().is_success() {
        return false;
    }
    let body = response.into_body().collect().await.ok();
    if let Some(body) = body {
        let value: serde_json::Value = serde_json::from_slice(&body.to_bytes()).unwrap_or_default();
        return value.get("ok").and_then(|value| value.as_bool()).unwrap_or(false);
    }
    false
}

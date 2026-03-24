#![allow(dead_code, unused_imports)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use devstack::api::{
    DaemonEvent, DaemonGlobalEvent, DaemonLogEvent, DaemonRunEvent, DaemonServiceEvent,
    DaemonTaskEvent,
};
use http_body_util::BodyExt;
use tokio::sync::oneshot;

mod api;
mod cli;
mod core;
mod daemon;
mod events;
mod fs;
mod runtime;

pub use api::ApiHandle;
pub use cli::{CliHandle, CmdResult};
pub use core::{FixtureBuilder, ProjectHandle, TaskStartOptions, TestHarness, UpOptions};
pub use daemon::{DaemonController, DaemonHandle};
pub use events::{EventRecorder, EventsHandle};
pub use fs::FsHandle;
pub use runtime::{RunHandle, ServiceHandle, TaskHandle};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DAEMON_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

struct HarnessInner {
    _root: tempfile::TempDir,
    _serial_lock_file: std::fs::File,
    home: PathBuf,
    xdg_data_home: PathBuf,
    xdg_config_home: PathBuf,
    xdg_runtime_dir: PathBuf,
    workspace: PathBuf,
    bin: PathBuf,
    daemon_log_path: Mutex<Option<PathBuf>>,
}

fn write_rendered_fixture(
    root: &Path,
    rendered: crate::support::fixtures::RenderedFixture,
) -> Result<()> {
    for (path, contents) in rendered.files {
        let full_path = root.join(path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(full_path, contents)?;
    }
    Ok(())
}

fn copy_fixture_bin_scripts(root: &Path) -> Result<()> {
    let source_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bin");
    let dest_dir = root.join("bin");
    std::fs::create_dir_all(&dest_dir)?;
    for entry in std::fs::read_dir(source_dir)? {
        let entry = entry?;
        let source = entry.path();
        let dest = dest_dir.join(entry.file_name());
        std::fs::copy(&source, &dest)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&dest)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&dest, permissions)?;
        }
    }
    Ok(())
}

async fn collect_events(
    harness: TestHarness,
    filter: Option<String>,
    sink: Arc<Mutex<Vec<DaemonEvent>>>,
    ready_tx: oneshot::Sender<()>,
) -> Result<()> {
    let api = harness.api();
    let path = if let Some(run_id) = filter.as_deref() {
        format!("/v1/events?run_id={}", urlencoding::encode(run_id))
    } else {
        "/v1/events".to_string()
    };

    let response = api.raw_request::<()>("GET", &path, None).await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.into_body().collect().await?.to_bytes();
        return Err(anyhow!(
            "event subscription failed: {status} {}",
            String::from_utf8_lossy(&body)
        ));
    }

    let _ = ready_tx.send(());

    let mut body = response.into_body();
    let mut buffer = String::new();
    while let Some(frame) = body.frame().await {
        let frame = frame?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        buffer.push_str(&String::from_utf8_lossy(data).replace("\r\n", "\n"));
        while let Some(idx) = buffer.find("\n\n") {
            let block = buffer[..idx].to_string();
            buffer.drain(..idx + 2);
            if let Some(event) = parse_sse_block(&block)? {
                sink.lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .push(event);
            }
        }
    }

    Ok(())
}

fn parse_sse_block(block: &str) -> Result<Option<DaemonEvent>> {
    let mut event_name: Option<&str> = None;
    let mut data_lines = Vec::new();
    for line in block.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
    }

    let Some(event_name) = event_name else {
        return Ok(None);
    };
    let data = data_lines.join("\n");
    if data.is_empty() {
        return Ok(None);
    }

    let event = match event_name {
        "run" => DaemonEvent::Run(serde_json::from_str::<DaemonRunEvent>(&data)?),
        "service" => DaemonEvent::Service(serde_json::from_str::<DaemonServiceEvent>(&data)?),
        "task" => DaemonEvent::Task(serde_json::from_str::<DaemonTaskEvent>(&data)?),
        "global" => DaemonEvent::Global(serde_json::from_str::<DaemonGlobalEvent>(&data)?),
        "log" => DaemonEvent::Log(serde_json::from_str::<DaemonLogEvent>(&data)?),
        _ => return Ok(None),
    };
    Ok(Some(event))
}

fn tail_lines(value: &str, limit: usize) -> String {
    let mut lines: Vec<&str> = value.lines().collect();
    if lines.len() > limit {
        lines = lines.split_off(lines.len() - limit);
    }
    lines.join("\n")
}

fn acquire_global_test_lock() -> Result<std::fs::File> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    let path = std::env::temp_dir().join("devstack-e2e.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result == 0 {
        Ok(file)
    } else {
        Err(anyhow!(std::io::Error::last_os_error()))
    }
}

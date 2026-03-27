use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::api::{
    HealthCheckStats, HealthStatus, RecentErrorLine, RunStatusResponse, ServiceStatus,
    SystemdStatus,
};
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::handles::HealthHandle;
use crate::logfmt::{extract_log_content, extract_timestamp_str};
use crate::model::{RunLifecycle, ServiceState};

pub async fn build_status(app: &AppContext, run_id: &str) -> AppResult<RunStatusResponse> {
    let run = app
        .runs
        .get_run(run_id)
        .await
        .ok_or_else(|| AppError::not_found(format!("run {run_id} not found")))?;

    let mut services = BTreeMap::new();
    for (name, service) in &run.services {
        let desired = if run.state == RunLifecycle::Stopped {
            "stopped".to_string()
        } else {
            "running".to_string()
        };
        let status = app
            .systemd
            .unit_status(&service.launch.unit_name)
            .await
            .unwrap_or(None)
            .map(|unit| SystemdStatus {
                active_state: unit.active_state,
                sub_state: unit.sub_state,
                result: unit.result,
            });

        let mut derived_state = service.runtime.state.clone();
        let mut derived_failure = service.runtime.last_failure.clone();
        if let Some(systemd) = &status
            && run.state != RunLifecycle::Stopped
            && (systemd.active_state == "failed"
                || systemd.result.as_deref() == Some("start-limit-hit")
                || (systemd.active_state != "active"
                    && systemd
                        .result
                        .as_deref()
                        .is_some_and(|result| result != "success")))
        {
            derived_state = match service.runtime.state {
                ServiceState::Starting => ServiceState::Failed,
                ServiceState::Ready => ServiceState::Degraded,
                ServiceState::Failed => ServiceState::Failed,
                ServiceState::Stopped => ServiceState::Stopped,
                ServiceState::Degraded => ServiceState::Degraded,
            };
            if derived_failure.is_none() && systemd.result.as_deref() != Some("success") {
                derived_failure = systemd.result.clone();
            }
        }

        let health = service
            .handles
            .health
            .as_ref()
            .map(health_status_from_handle);
        let health_check_stats = health.as_ref().map(health_check_stats_from_status);
        let recent_errors = recent_stderr_lines(&service.launch.log_path, 3).await;

        services.insert(
            name.clone(),
            ServiceStatus {
                desired,
                systemd: status,
                ready: derived_state == ServiceState::Ready,
                state: derived_state,
                last_failure: derived_failure,
                health,
                health_check_stats,
                uptime_seconds: uptime_seconds_since(service.runtime.last_started_at.as_deref()),
                recent_errors,
                url: service.launch.url.clone(),
                auto_restart: service.spec.auto_restart,
                watch_paused: service.runtime.watch_paused,
                watch_active: service.watch_active(),
            },
        );
    }

    let mut any_degraded = false;
    let mut all_ready = true;
    for service in services.values() {
        match service.state {
            ServiceState::Ready => {}
            ServiceState::Starting => all_ready = false,
            ServiceState::Degraded | ServiceState::Failed => {
                any_degraded = true;
                all_ready = false;
            }
            ServiceState::Stopped => all_ready = false,
        }
    }
    let state = if any_degraded {
        RunLifecycle::Degraded
    } else if all_ready {
        RunLifecycle::Running
    } else {
        run.state.clone()
    };

    Ok(RunStatusResponse {
        run_id: run.run_id.as_str().to_string(),
        stack: run.stack,
        project_dir: run.project_dir.to_string_lossy().to_string(),
        state,
        services,
    })
}

fn health_status_from_handle(handle: &HealthHandle) -> HealthStatus {
    let snapshot = handle.stats.lock().unwrap_or_else(|err| err.into_inner());
    HealthStatus {
        passes: snapshot.passes,
        failures: snapshot.failures,
        consecutive_failures: snapshot.consecutive_failures,
        last_check_at: snapshot.last_check_at.clone(),
        last_ok: snapshot.last_ok,
    }
}

fn health_check_stats_from_status(health: &HealthStatus) -> HealthCheckStats {
    HealthCheckStats {
        passes: health.passes,
        failures: health.failures,
        consecutive_failures: health.consecutive_failures,
        last_check_at: health.last_check_at.clone(),
        last_ok: health.last_ok,
    }
}

fn uptime_seconds_since(last_started_at: Option<&str>) -> Option<u64> {
    let started_at = last_started_at?;
    let started_at = OffsetDateTime::parse(started_at, &Rfc3339).ok()?;
    let now = OffsetDateTime::now_utc();
    let elapsed = (now - started_at).whole_seconds();
    if elapsed < 0 {
        return Some(0);
    }
    Some(elapsed as u64)
}

fn recent_error_from_raw(raw_line: &str) -> RecentErrorLine {
    let timestamp = extract_timestamp_str(raw_line);
    let (_, message) = extract_log_content(raw_line);
    RecentErrorLine { timestamp, message }
}

fn push_recent_stderr_line(lines: &mut Vec<RecentErrorLine>, raw_line: &[u8], limit: usize) {
    if lines.len() >= limit {
        return;
    }

    let raw_line = String::from_utf8_lossy(raw_line);
    let raw_line = crate::logfmt::strip_ansi_if_needed(raw_line.trim_end_matches(['\r', '\n']));
    if raw_line.is_empty() {
        return;
    }

    let (stream, _) = extract_log_content(&raw_line);
    if stream != "stderr" {
        return;
    }

    lines.push(recent_error_from_raw(&raw_line));
}

fn recent_stderr_lines_from_file(log_path: &Path, limit: usize) -> Result<Vec<RecentErrorLine>> {
    if limit == 0 || !log_path.exists() {
        return Ok(Vec::new());
    }

    const CHUNK_SIZE: usize = 64 * 1024;

    let mut file = File::open(log_path)
        .with_context(|| format!("open log file {}", log_path.to_string_lossy()))?;
    let mut offset = file.metadata()?.len();
    let mut trailing = Vec::new();
    let mut lines = Vec::with_capacity(limit);

    while offset > 0 && lines.len() < limit {
        let read_len = (offset as usize).min(CHUNK_SIZE);
        offset -= read_len as u64;

        let mut chunk = vec![0; read_len];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&trailing);

        let mut line_end = chunk.len();
        while let Some(newline_idx) = chunk[..line_end].iter().rposition(|byte| *byte == b'\n') {
            push_recent_stderr_line(&mut lines, &chunk[newline_idx + 1..line_end], limit);
            line_end = newline_idx;
            if lines.len() >= limit {
                break;
            }
        }

        trailing = chunk[..line_end].to_vec();
    }

    if lines.len() < limit && !trailing.is_empty() {
        push_recent_stderr_line(&mut lines, &trailing, limit);
    }

    lines.reverse();
    Ok(lines)
}

async fn recent_stderr_lines(log_path: &Path, limit: usize) -> Vec<RecentErrorLine> {
    let log_path = log_path.to_path_buf();
    tokio::task::spawn_blocking(move || recent_stderr_lines_from_file(log_path.as_path(), limit))
        .await
        .ok()
        .and_then(|result| result.ok())
        .unwrap_or_default()
}

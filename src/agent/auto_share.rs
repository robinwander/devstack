use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::api::{LogsResponse, RunListResponse, RunStatusResponse};
use crate::infra::ipc::UnixDaemonClient;
use crate::model::RunLifecycle;

use super::pty_proxy::format_message_for_pty;

const AUTO_SHARE_POLL_INTERVAL: Duration = Duration::from_secs(2);
const AUTO_SHARE_RATE_LIMIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AutoShareLevel {
    Error,
    Warn,
}

#[derive(Clone, Debug)]
pub(crate) struct AutoShareConfig {
    pub(crate) level: AutoShareLevel,
    pub(crate) run_id: String,
    pub(crate) services: Vec<String>,
}

pub(crate) async fn configure_auto_share(
    level: Option<AutoShareLevel>,
    run_id: Option<String>,
    watch: Vec<String>,
    daemon: &UnixDaemonClient,
) -> Option<AutoShareConfig> {
    let level = level?;

    let run_id = if let Some(run_id) = run_id {
        run_id
    } else {
        match resolve_latest_run_id_for_cwd(daemon).await {
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
        match fetch_run_services(daemon, &run_id).await {
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

async fn resolve_latest_run_id_for_cwd(daemon: &UnixDaemonClient) -> Result<Option<String>> {
    let runs: RunListResponse = daemon
        .request_json("GET", "/v1/runs", None::<()>, None)
        .await?;
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

async fn fetch_run_services(daemon: &UnixDaemonClient, run_id: &str) -> Result<Vec<String>> {
    let status: RunStatusResponse = daemon
        .request_json(
            "GET",
            &format!("/v1/runs/{run_id}/status"),
            None::<()>,
            None,
        )
        .await?;
    let mut services: Vec<String> = status.services.keys().cloned().collect();
    services.sort();
    Ok(services)
}

async fn fetch_service_logs_after(
    daemon: &UnixDaemonClient,
    run_id: &str,
    service: &str,
    after: u64,
) -> Result<LogsResponse> {
    let run_id = urlencoding::encode(run_id);
    let service = urlencoding::encode(service);
    let path = format!("/v1/runs/{run_id}/logs/{service}?after={after}&last=200");
    daemon.request_json("GET", &path, None::<()>, None).await
}

async fn fetch_latest_service_cursor(
    daemon: &UnixDaemonClient,
    run_id: &str,
    service: &str,
) -> Result<u64> {
    let run_id = urlencoding::encode(run_id);
    let service = urlencoding::encode(service);
    let path = format!("/v1/runs/{run_id}/logs/{service}?last=1");
    let logs: LogsResponse = daemon.request_json("GET", &path, None::<()>, None).await?;
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

pub(crate) async fn run_auto_share_monitor(
    daemon: UnixDaemonClient,
    config: AutoShareConfig,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) {
    let mut cursors: HashMap<String, u64> = HashMap::new();
    for service in &config.services {
        let cursor = fetch_latest_service_cursor(&daemon, &config.run_id, service)
            .await
            .unwrap_or(0);
        cursors.insert(service.clone(), cursor);
    }

    let mut last_sent = HashMap::new();

    while !stop.load(Ordering::Relaxed) {
        for service in &config.services {
            let after = *cursors.get(service).unwrap_or(&0);
            let response =
                match fetch_service_logs_after(&daemon, &config.run_id, service, after).await {
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
                let _ = tx.send(format_message_for_pty(&message).into_bytes());
            }
        }

        tokio::time::sleep(AUTO_SHARE_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

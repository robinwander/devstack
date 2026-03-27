use std::collections::VecDeque;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::api::{HealthStatus, RunStatusResponse, ServiceStatus};
use crate::ids::{RunId, ServiceName};
use crate::logfmt::strip_ansi_if_needed;
use crate::paths;
use crate::persistence::PersistedRun;
use crate::services::readiness::PortBindingInfo;
use crate::util::{now_rfc3339, sanitize_env_key};

#[derive(Debug, Serialize)]
pub struct DiagnoseResponse {
    pub run_id: String,
    pub stack: String,
    pub project_dir: String,
    pub generated_at: String,
    pub services: Vec<DiagnoseService>,
}

#[derive(Debug, Serialize)]
pub struct DiagnoseService {
    pub name: String,
    pub daemon_state: crate::model::ServiceState,
    pub systemd: Option<crate::api::SystemdStatus>,
    pub health: Option<HealthStatus>,
    pub expected_port: Option<u16>,
    pub port_binding: Option<DiagnosePortBinding>,
    pub last_log_lines: Vec<String>,
    pub issues: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosePortBinding {
    pub listening_pids: Vec<DiagnosePid>,
    pub owned_by_unit: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosePid {
    pub pid: u32,
    pub comm: Option<String>,
    pub cmdline: Option<String>,
}

pub async fn diagnose_run(
    run_id: &str,
    status: RunStatusResponse,
    manifest: PersistedRun,
    service_filter: Option<&str>,
) -> Result<DiagnoseResponse> {
    let mut services = Vec::new();

    let mut names: Vec<String> = status.services.keys().cloned().collect();
    names.sort();

    for name in names {
        if let Some(filter) = service_filter
            && filter != name
        {
            continue;
        }

        let svc_status = status
            .services
            .get(&name)
            .context("missing service status")?;
        let expected_port = manifest.services.get(&name).and_then(|svc| svc.port);

        let unit_name = unit_name_for_run(run_id, &name);

        let binding_info = if let Some(port) = expected_port {
            Some(crate::services::readiness::port_binding_info(port, Some(&unit_name)).await?)
        } else {
            None
        };
        let port_binding = binding_info
            .as_ref()
            .map(describe_port_binding)
            .transpose()?;

        let log_path = paths::run_log_path(
            &RunId::new(run_id.to_string()),
            &ServiceName::new(name.clone()),
        )?;
        let last_log_lines = tail_file_lines(&log_path, 10)?;

        let issues = detect_issues(svc_status, expected_port, binding_info.as_ref());

        services.push(DiagnoseService {
            name: name.clone(),
            daemon_state: svc_status.state.clone(),
            systemd: svc_status.systemd.clone(),
            health: svc_status.health.clone(),
            expected_port,
            port_binding,
            last_log_lines,
            issues,
        });
    }

    Ok(DiagnoseResponse {
        run_id: status.run_id,
        stack: status.stack,
        project_dir: status.project_dir,
        generated_at: now_rfc3339(),
        services,
    })
}

fn unit_name_for_run(run_id: &str, service: &str) -> String {
    let run = sanitize_env_key(run_id);
    let svc = sanitize_env_key(service);
    format!("devstack-run-{run}-{svc}.service")
}

fn describe_port_binding(info: &PortBindingInfo) -> Result<DiagnosePortBinding> {
    let mut pids = Vec::new();
    for pid in &info.listening_pids {
        pids.push(DiagnosePid {
            pid: *pid,
            comm: pid_comm(*pid),
            cmdline: pid_cmdline(*pid),
        });
    }
    Ok(DiagnosePortBinding {
        listening_pids: pids,
        owned_by_unit: info.owned_by_unit,
    })
}

fn pid_comm(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/{pid}/comm");
        let content = std::fs::read_to_string(path).ok()?;
        Some(content.trim().to_string())
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

fn pid_cmdline(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/{pid}/cmdline");
        let bytes = std::fs::read(path).ok()?;
        if bytes.is_empty() {
            return None;
        }
        let parts: Vec<String> = bytes
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).to_string())
            .collect();
        if parts.is_empty() {
            return None;
        }
        Some(parts.join(" "))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

fn tail_file_lines(path: &Path, max: usize) -> Result<Vec<String>> {
    if max == 0 {
        return Ok(Vec::new());
    }
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = std::fs::File::open(path)
        .with_context(|| format!("open log file {}", path.to_string_lossy()))?;
    let reader = std::io::BufReader::new(file);
    let mut ring: VecDeque<String> = VecDeque::with_capacity(max);

    for line in std::io::BufRead::lines(reader) {
        let line = line.unwrap_or_default();
        let clean = strip_ansi_if_needed(&line);
        if ring.len() == max {
            ring.pop_front();
        }
        ring.push_back(clean);
    }

    Ok(ring.into_iter().collect())
}

fn detect_issues(
    svc: &ServiceStatus,
    expected_port: Option<u16>,
    binding: Option<&PortBindingInfo>,
) -> Vec<String> {
    let mut issues = Vec::new();

    if let Some(sys) = &svc.systemd {
        if sys.active_state == "failed" || sys.result.as_deref() == Some("start-limit-hit") {
            issues.push("crash-looping".to_string());
        }
        if svc.desired == "running" && sys.active_state == "inactive" {
            issues.push("inactive".to_string());
        }
    }

    if matches!(svc.state, crate::model::ServiceState::Degraded) {
        issues.push("degraded".to_string());
    }
    if matches!(svc.state, crate::model::ServiceState::Failed) {
        issues.push("failed".to_string());
    }

    if expected_port.is_some() {
        match binding {
            Some(binding) => {
                if binding.probe_supported {
                    if !binding.has_listener {
                        issues.push("port-not-bound".to_string());
                    } else if !binding.listening_pids.is_empty()
                        && matches!(binding.owned_by_unit, Some(false))
                    {
                        issues.push("wrong-port-or-orphan-listener".to_string());
                    }
                }
            }
            None => {
                issues.push("port-not-bound".to_string());
            }
        }
    }

    if let Some(health) = &svc.health
        && health.consecutive_failures >= 3
    {
        issues.push("health-checks-failing".to_string());
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ServiceStatus;
    use crate::model::ServiceState;

    fn ready_service() -> ServiceStatus {
        ServiceStatus {
            desired: "running".to_string(),
            systemd: None,
            ready: true,
            state: ServiceState::Ready,
            last_failure: None,
            health: None,
            health_check_stats: None,
            uptime_seconds: None,
            recent_errors: Vec::new(),
            url: Some("http://localhost:3000".to_string()),
            auto_restart: false,
            watch_paused: false,
            watch_active: false,
        }
    }

    #[test]
    fn detect_issues_does_not_report_port_not_bound_when_listener_exists_but_pid_is_unknown() {
        let svc = ready_service();
        let binding = PortBindingInfo {
            listening_pids: Vec::new(),
            owned_by_unit: None,
            has_listener: true,
            probe_supported: true,
        };

        let issues = detect_issues(&svc, Some(3000), Some(&binding));
        assert!(!issues.iter().any(|issue| issue == "port-not-bound"));
    }

    #[test]
    fn detect_issues_reports_port_not_bound_when_probe_finds_no_listener() {
        let svc = ready_service();
        let binding = PortBindingInfo {
            listening_pids: Vec::new(),
            owned_by_unit: None,
            has_listener: false,
            probe_supported: true,
        };

        let issues = detect_issues(&svc, Some(3000), Some(&binding));
        assert!(issues.iter().any(|issue| issue == "port-not-bound"));
    }
}

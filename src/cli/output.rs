use anyhow::Result;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::api::{FacetValueCount, LogEntry, LogViewResponse, RunWatchResponse};
use crate::logs::{
    is_health_noise_line, is_health_noise_message, structured_log_from_entry,
    structured_log_from_raw,
};
use crate::model::{RunLifecycle, ServiceState};

pub(crate) fn print_json(value: serde_json::Value, pretty: bool) {
    if pretty {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
    } else {
        println!("{}", serde_json::to_string(&value).unwrap_or_default());
    }
}

pub(crate) fn print_watch_status_human(status: &RunWatchResponse) {
    println!("watch: {}", status.run_id);
    println!("  service    auto_restart  status");

    for (name, svc) in &status.services {
        let state = if !svc.auto_restart {
            "disabled"
        } else if svc.paused {
            "paused"
        } else if svc.active {
            "watching"
        } else {
            "inactive"
        };
        println!("  {:<9}  {:<12}  {}", name, svc.auto_restart, state);
    }
}

pub(crate) fn print_status_human(status: &crate::api::RunStatusResponse) {
    let total_services = status.services.len();
    let healthy_services = status
        .services
        .values()
        .filter(|svc| is_service_healthy(svc))
        .count();

    println!(
        "stack: {} ({}, {}/{} healthy)",
        status.stack,
        run_lifecycle_label(&status.state),
        healthy_services,
        total_services
    );

    let is_tty = std::io::stdout().is_terminal();
    let max_name_len = status
        .services
        .keys()
        .map(|name| name.len())
        .max()
        .unwrap_or(0);

    for (name, svc) in &status.services {
        let state = service_state_label(&svc.state);
        let colored_state = if is_tty {
            match svc.state {
                ServiceState::Ready => format!("\x1b[32m{}\x1b[0m", state),
                ServiceState::Degraded => format!("\x1b[33m{}\x1b[0m", state),
                ServiceState::Failed => format!("\x1b[31m{}\x1b[0m", state),
                _ => state.to_string(),
            }
        } else {
            state.to_string()
        };

        let url = svc.url.as_deref().unwrap_or("");
        let uptime = svc
            .uptime_seconds
            .map(|s| format!("(up {})", format_compact_duration(s)))
            .unwrap_or_else(|| "(up unknown)".to_string());

        let watch_suffix = if svc.auto_restart {
            if svc.watch_paused {
                "  [paused]"
            } else {
                "  [watching]"
            }
        } else {
            ""
        };

        println!(
            "  {:width$}  {}  {}  {}{}",
            name,
            colored_state,
            url,
            uptime,
            watch_suffix,
            width = max_name_len
        );

        if let Some(last_error) = svc.recent_errors.last() {
            let relative = last_error
                .timestamp
                .as_deref()
                .map(format_relative_timestamp)
                .unwrap_or_else(|| "unknown".to_string());
            println!("    last error ({}): {}", relative, last_error.message);
        }
    }
}

pub(crate) fn emit_log_facets(label: &str, response: &LogViewResponse, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(response)?);
    } else {
        print!("{}", format_log_facets(label, response));
    }
    Ok(())
}

pub(crate) fn format_log_facets(label: &str, response: &LogViewResponse) -> String {
    let mut out = String::new();
    out.push_str(label);
    out.push_str("\n\n");

    if response.filters.is_empty() {
        out.push_str("filters (0):\n");
        return out;
    }

    for (index, filter) in response.filters.iter().enumerate() {
        let section_name = format!("{} [{}]", filter.field, filter.kind);
        out.push_str(&format_facet_section(&section_name, &filter.values));
        if index + 1 < response.filters.len() {
            out.push('\n');
        }
    }
    out
}

fn format_facet_section(name: &str, values: &[FacetValueCount]) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} ({}):\n", name, values.len()));

    if values.is_empty() {
        return out;
    }

    let max_value_width = values
        .iter()
        .map(|item| item.value.len())
        .max()
        .unwrap_or(0);
    let formatted_counts: Vec<String> = values
        .iter()
        .map(|item| format_count_with_commas(item.count))
        .collect();
    let max_count_width = formatted_counts
        .iter()
        .map(|count| count.len())
        .max()
        .unwrap_or(0);

    for (item, formatted_count) in values.iter().zip(formatted_counts.iter()) {
        out.push_str(&format!(
            "  {:value_width$}  {:>count_width$}\n",
            item.value,
            formatted_count,
            value_width = max_value_width,
            count_width = max_count_width,
        ));
    }

    out
}

fn format_count_with_commas(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, ch) in digits.chars().enumerate() {
        out.push(ch);
        let remaining = digits.len() - index - 1;
        if remaining > 0 && remaining.is_multiple_of(3) {
            out.push(',');
        }
    }

    out
}

pub(crate) fn emit_entry(entry: &LogEntry, json: bool, no_health: bool) -> Result<()> {
    if no_health && is_health_noise_message(&entry.message) {
        return Ok(());
    }

    if json {
        let payload = structured_log_from_entry(entry);
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        println!("[{}] {}", entry.service, entry.raw);
    }

    Ok(())
}

pub(crate) fn emit_line(line: &str, service: &str, json: bool, no_health: bool) -> Result<()> {
    if no_health && is_health_noise_line(line) {
        return Ok(());
    }

    if json {
        let payload = structured_log_from_raw(service, line);
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        println!("{line}");
    }
    Ok(())
}

pub(crate) fn emit_lines(
    lines: &[String],
    service: &str,
    json: bool,
    no_health: bool,
) -> Result<()> {
    for line in lines {
        emit_line(line, service, json, no_health)?;
    }
    Ok(())
}

pub(crate) fn is_service_healthy(svc: &crate::api::ServiceStatus) -> bool {
    if svc.state != ServiceState::Ready {
        return false;
    }
    svc.health_check_stats
        .as_ref()
        .and_then(|stats| stats.last_ok)
        .unwrap_or(true)
}

fn run_lifecycle_label(state: &RunLifecycle) -> &'static str {
    match state {
        RunLifecycle::Starting => "starting",
        RunLifecycle::Running => "running",
        RunLifecycle::Degraded => "degraded",
        RunLifecycle::Stopped => "stopped",
    }
}

fn service_state_label(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Starting => "starting",
        ServiceState::Ready => "ready",
        ServiceState::Degraded => "degraded",
        ServiceState::Stopped => "stopped",
        ServiceState::Failed => "failed",
    }
}

fn format_relative_timestamp(timestamp: &str) -> String {
    let Ok(ts) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
        return "unknown".to_string();
    };
    let elapsed = (OffsetDateTime::now_utc() - ts).whole_seconds();
    if elapsed <= 0 {
        return "just now".to_string();
    }
    format!("{} ago", format_compact_duration(elapsed as u64))
}

pub(crate) fn format_compact_duration(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    if seconds >= DAY {
        format!("{}d", seconds / DAY)
    } else if seconds >= HOUR {
        format!("{}h", seconds / HOUR)
    } else if seconds >= MINUTE {
        format!("{}m", seconds / MINUTE)
    } else {
        format!("{}s", seconds)
    }
}

use std::io::IsTerminal;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_compact_duration_prefers_largest_unit() {
        assert_eq!(format_compact_duration(5), "5s");
        assert_eq!(format_compact_duration(120), "2m");
        assert_eq!(format_compact_duration(7_200), "2h");
        assert_eq!(format_compact_duration(172_800), "2d");
    }

    #[test]
    fn format_log_facets_pretty_output() {
        let response = LogViewResponse {
            entries: Vec::new(),
            truncated: false,
            total: 1756,
            filters: vec![
                crate::api::FacetFilter {
                    field: "service".to_string(),
                    kind: "select".to_string(),
                    values: vec![
                        FacetValueCount {
                            value: "pi-agent-2026-03-03".to_string(),
                            count: 1247,
                        },
                        FacetValueCount {
                            value: "cron-2026-03-03".to_string(),
                            count: 89,
                        },
                        FacetValueCount {
                            value: "pi-extension-2026-03-02".to_string(),
                            count: 412,
                        },
                    ],
                },
                crate::api::FacetFilter {
                    field: "level".to_string(),
                    kind: "toggle".to_string(),
                    values: vec![
                        FacetValueCount {
                            value: "info".to_string(),
                            count: 1583,
                        },
                        FacetValueCount {
                            value: "error".to_string(),
                            count: 42,
                        },
                        FacetValueCount {
                            value: "warn".to_string(),
                            count: 123,
                        },
                    ],
                },
                crate::api::FacetFilter {
                    field: "stream".to_string(),
                    kind: "toggle".to_string(),
                    values: vec![
                        FacetValueCount {
                            value: "stdout".to_string(),
                            count: 1700,
                        },
                        FacetValueCount {
                            value: "stderr".to_string(),
                            count: 48,
                        },
                    ],
                },
            ],
        };

        let output = format_log_facets("Source: pi", &response);
        assert!(output.starts_with("Source: pi\n\nservice [select] (3):\n"));
        assert!(output.contains("1,247"));
        assert!(output.contains("level [toggle] (3):\n"));
        assert!(output.contains("info"));
        assert!(output.contains("stream [toggle] (2):\n"));
        assert!(output.contains("1,700"));
    }

    #[test]
    fn is_service_healthy_uses_health_check_stats() {
        let ready = crate::api::ServiceStatus {
            desired: "running".to_string(),
            systemd: None,
            ready: true,
            state: ServiceState::Ready,
            last_failure: None,
            health: None,
            health_check_stats: Some(crate::api::HealthCheckStats {
                passes: 10,
                failures: 1,
                consecutive_failures: 1,
                last_check_at: None,
                last_ok: Some(true),
            }),
            uptime_seconds: Some(42),
            recent_errors: Vec::new(),
            url: Some("http://localhost:3000".to_string()),
            auto_restart: false,
            watch_paused: false,
            watch_active: false,
        };
        assert!(is_service_healthy(&ready));

        let unhealthy = crate::api::ServiceStatus {
            health_check_stats: Some(crate::api::HealthCheckStats {
                last_ok: Some(false),
                ..crate::api::HealthCheckStats {
                    passes: 10,
                    failures: 2,
                    consecutive_failures: 2,
                    last_check_at: None,
                    last_ok: Some(true),
                }
            }),
            ..ready
        };
        assert!(!is_service_healthy(&unhealthy));
    }
}

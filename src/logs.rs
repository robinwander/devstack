use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::api::LogEntry;
use crate::logfmt::{
    classify_line_level, detect_log_level, extract_log_content, extract_timestamp_str,
    is_health_check_line, is_health_check_message,
};
use crate::util::strip_ansi_if_needed;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StructuredLogLine {
    pub(crate) timestamp: Option<String>,
    pub(crate) service: String,
    pub(crate) stream: String,
    pub(crate) level: Option<String>,
    pub(crate) message: String,
}

pub(crate) fn structured_log_from_raw(service: &str, line: &str) -> StructuredLogLine {
    let clean = strip_ansi_if_needed(line);
    let timestamp = extract_timestamp_str(&clean);
    let (stream, message) = extract_log_content(&clean);
    let level = classify_line_level(&clean);

    StructuredLogLine {
        timestamp,
        service: service.to_string(),
        stream,
        level: normalize_level(Some(&level), &message),
        message,
    }
}

pub(crate) fn structured_log_from_entry(entry: &LogEntry) -> StructuredLogLine {
    let stream = if entry.stream.is_empty() {
        "stdout"
    } else {
        entry.stream.as_str()
    };
    let message = strip_ansi_if_needed(&entry.message);

    StructuredLogLine {
        timestamp: if entry.ts.trim().is_empty() {
            None
        } else {
            Some(entry.ts.clone())
        },
        service: entry.service.clone(),
        stream: stream.to_string(),
        level: normalize_level(Some(entry.level.as_str()), &message),
        message,
    }
}

pub(crate) fn is_health_noise_line(line: &str) -> bool {
    is_health_check_line(line)
}

pub(crate) fn is_health_noise_message(message: &str) -> bool {
    is_health_check_message(message)
}

fn normalize_level(level: Option<&str>, message: &str) -> Option<String> {
    if let Some(level) = level {
        let lower = level.trim().to_ascii_lowercase();
        match lower.as_str() {
            "error" | "warn" => return Some(lower),
            "info" | "" => {}
            _ => return Some(lower),
        }
    }

    match detect_log_level(message) {
        "error" => Some("error".to_string()),
        "warn" => Some("warn".to_string()),
        _ => None,
    }
}

pub async fn stream_logs(
    path: &Path,
    service: &str,
    tail: Option<usize>,
    follow: bool,
    follow_for: Option<Duration>,
    json: bool,
    no_health: bool,
) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("log file not found: {path:?}"));
    }

    let content = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let start = tail.map(|n| lines.len().saturating_sub(n)).unwrap_or(0);

    for line in &lines[start..] {
        emit_line(line, service, json, no_health)?;
    }

    if !follow {
        return Ok(());
    }

    let mut offset = content.len() as u64;
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open log {path:?}"))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.blocking_send(());
        }
    })
    .ok();

    if let Some(watcher) = watcher.as_mut() {
        let _ = watcher.watch(path, RecursiveMode::NonRecursive);
    }

    let start = Instant::now();
    loop {
        if let Some(limit) = follow_for && start.elapsed() >= limit {
            return Ok(());
        }
        tokio::select! {
            _ = rx.recv() => {
                read_new_lines(&mut file, &mut offset, service, json, no_health).await?;
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                read_new_lines(&mut file, &mut offset, service, json, no_health).await?;
            }
        }
    }
}

async fn read_new_lines(
    file: &mut tokio::fs::File,
    offset: &mut u64,
    service: &str,
    json: bool,
    no_health: bool,
) -> Result<()> {
    let metadata = file.metadata().await?;
    if metadata.len() < *offset {
        *offset = metadata.len();
        return Ok(());
    }
    if metadata.len() == *offset {
        return Ok(());
    }

    file.seek(std::io::SeekFrom::Start(*offset)).await?;
    let mut buf = vec![0; (metadata.len() - *offset) as usize];
    file.read_exact(&mut buf).await?;
    *offset = metadata.len();

    let content = String::from_utf8_lossy(&buf);
    for line in content.lines() {
        emit_line_async(line, service, json, no_health).await?;
    }
    Ok(())
}

fn emit_line(line: &str, service: &str, json: bool, no_health: bool) -> Result<()> {
    if no_health && is_health_noise_line(line) {
        return Ok(());
    }

    if json {
        let payload = structured_log_from_raw(service, line);
        let output = serde_json::to_string(&payload)?;
        println!("{output}");
    } else {
        println!("{line}");
    }
    Ok(())
}

async fn emit_line_async(
    line: &str,
    service: &str,
    json: bool,
    no_health: bool,
) -> Result<()> {
    if no_health && is_health_noise_line(line) {
        return Ok(());
    }

    if json {
        let payload = structured_log_from_raw(service, line);
        let output = serde_json::to_string(&payload)?;
        tokio::io::stdout().write_all(output.as_bytes()).await?;
        tokio::io::stdout().write_all(b"\n").await?;
    } else {
        tokio::io::stdout().write_all(line.as_bytes()).await?;
        tokio::io::stdout().write_all(b"\n").await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_log_from_raw_extracts_standard_fields() {
        let line = "[2025-01-01T00:00:00Z] [stderr] Error: broken";
        let parsed = structured_log_from_raw("api", line);

        assert_eq!(parsed.timestamp.as_deref(), Some("2025-01-01T00:00:00Z"));
        assert_eq!(parsed.service, "api");
        assert_eq!(parsed.stream, "stderr");
        assert_eq!(parsed.level.as_deref(), Some("error"));
        assert_eq!(parsed.message, "Error: broken");
    }

    #[test]
    fn structured_log_from_raw_handles_json_lines() {
        let line = r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","level":"WARN","msg":"watch out"}"#;
        let parsed = structured_log_from_raw("api", line);

        assert_eq!(parsed.timestamp.as_deref(), Some("2025-01-01T00:00:00Z"));
        assert_eq!(parsed.stream, "stdout");
        assert_eq!(parsed.level.as_deref(), Some("warn"));
        assert_eq!(parsed.message, "watch out");
    }

    #[test]
    fn structured_log_from_entry_maps_info_to_unknown_level() {
        let entry = LogEntry {
            ts: "2025-01-01T00:00:00Z".to_string(),
            service: "web".to_string(),
            stream: "stdout".to_string(),
            level: "info".to_string(),
            message: "server ready".to_string(),
            raw: "[2025-01-01T00:00:00Z] [stdout] server ready".to_string(),
            attributes: Default::default(),
        };

        let parsed = structured_log_from_entry(&entry);
        assert_eq!(parsed.level, None);
        assert_eq!(parsed.message, "server ready");
    }

    #[test]
    fn health_noise_message_detects_access_logs() {
        assert!(is_health_noise_message("GET /health HTTP/1.1 200"));
        assert!(!is_health_noise_message("GET /api/users HTTP/1.1 200"));
    }
}

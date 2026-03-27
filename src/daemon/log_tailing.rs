use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use notify::{EventKind, RecursiveMode, Watcher};
use serde_json::Value as JsonValue;

use crate::api::{DaemonEvent, DaemonLogEvent};
use crate::ids::RunId;
use crate::logfmt::{classify_line_level, extract_log_content, extract_timestamp_str};
use crate::paths;

use super::router::DaemonState;

#[derive(Default)]
pub struct RunLogTailRegistry {
    pub runs: HashMap<String, RunLogTailHandle>,
}

pub struct RunLogTailHandle {
    pub subscribers: usize,
    pub task: tokio::task::JoinHandle<()>,
}

struct LogTailCursor {
    offset: u64,
}

pub async fn retain_run_log_tail(state: &DaemonState, run_id: &str) -> Result<()> {
    let mut registry = state.log_tails.lock().await;
    if let Some(handle) = registry.runs.get_mut(run_id) {
        handle.subscribers += 1;
        return Ok(());
    }

    let run_id_owned = run_id.to_string();
    let task_state = state.clone();
    let task_run_id = run_id_owned.clone();
    let task = tokio::spawn(async move {
        if let Err(err) = tail_run_logs(task_state, task_run_id.clone()).await {
            eprintln!("devstack: log tail failed for {task_run_id}: {err}");
        }
    });

    registry.runs.insert(
        run_id_owned,
        RunLogTailHandle {
            subscribers: 1,
            task,
        },
    );
    Ok(())
}

pub async fn release_run_log_tail(state: &DaemonState, run_id: &str) {
    let handle = {
        let mut registry = state.log_tails.lock().await;
        let Some(entry) = registry.runs.get_mut(run_id) else {
            return;
        };
        if entry.subscribers > 1 {
            entry.subscribers -= 1;
            return;
        }
        registry.runs.remove(run_id)
    };

    if let Some(handle) = handle {
        handle.task.abort();
    }
}

async fn tail_run_logs(state: DaemonState, run_id: String) -> Result<()> {
    let logs_dir = paths::run_logs_dir(&RunId::new(run_id.clone()))?;
    std::fs::create_dir_all(&logs_dir)?;
    tail_run_logs_in_dir(state, run_id, logs_dir).await
}

async fn tail_run_logs_in_dir(state: DaemonState, run_id: String, logs_dir: PathBuf) -> Result<()> {
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = notify_tx.send(event);
    })
    .context("create log tail watcher")?;
    watcher
        .watch(&logs_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watch log directory {}", logs_dir.to_string_lossy()))?;

    let mut cursors = initial_log_tail_cursors(&logs_dir)?;
    let _watcher = watcher;

    while let Some(event) = notify_rx.recv().await {
        match event {
            Ok(event) => match event.kind {
                EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) => {
                    for path in event.paths {
                        if !is_tailed_log_path(&path) {
                            continue;
                        }
                        if !path.exists() {
                            cursors.remove(&path);
                            continue;
                        }
                        let cursor = cursors
                            .entry(path.clone())
                            .or_insert(LogTailCursor { offset: 0 });
                        if let Ok(events) = read_new_log_events(&run_id, &path, cursor) {
                            for event in events {
                                state.app.emit_event(DaemonEvent::Log(event));
                            }
                        }
                    }
                }
                EventKind::Remove(_) => {
                    for path in event.paths {
                        cursors.remove(&path);
                    }
                }
                _ => {}
            },
            Err(err) => eprintln!("devstack: log tail watcher error for {run_id}: {err}"),
        }
    }

    Ok(())
}

fn initial_log_tail_cursors(logs_dir: &Path) -> Result<HashMap<PathBuf, LogTailCursor>> {
    let mut cursors = HashMap::new();
    if !logs_dir.exists() {
        return Ok(cursors);
    }

    for entry in std::fs::read_dir(logs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !is_tailed_log_path(&path) {
            continue;
        }
        let offset = std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        cursors.insert(path, LogTailCursor { offset });
    }

    Ok(cursors)
}

fn is_tailed_log_path(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("log")
}

fn read_new_log_events(
    run_id: &str,
    path: &Path,
    cursor: &mut LogTailCursor,
) -> Result<Vec<DaemonLogEvent>> {
    let file_len = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if file_len < cursor.offset {
        cursor.offset = 0;
    }

    let mut file = File::open(path).with_context(|| format!("open log {}", path.display()))?;
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    if buf.is_empty() {
        return Ok(Vec::new());
    }

    let Some(last_newline) = buf.iter().rposition(|byte| *byte == b'\n') else {
        return Ok(Vec::new());
    };

    let complete_len = last_newline + 1;
    let complete = &buf[..complete_len];
    cursor.offset = cursor.offset.saturating_add(complete_len as u64);

    let service = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("invalid log path {}", path.display()))?;

    let mut events = Vec::new();
    for raw_line in String::from_utf8_lossy(complete).lines() {
        if let Some(event) = parse_log_tail_event(run_id, &service, raw_line) {
            events.push(event);
        }
    }

    Ok(events)
}

fn parse_log_tail_event(run_id: &str, service: &str, raw_line: &str) -> Option<DaemonLogEvent> {
    let raw = crate::logfmt::strip_ansi_if_needed(raw_line.trim_end_matches(['\r', '\n']));
    if raw.is_empty() {
        return None;
    }

    let ts = extract_timestamp_str(&raw).unwrap_or_default();
    let (stream, message) = extract_log_content(&raw);
    Some(DaemonLogEvent {
        run_id: run_id.to_string(),
        service: service.to_string(),
        ts,
        stream,
        level: classify_line_level(&raw),
        message,
        raw: raw.clone(),
        attributes: extract_log_attributes(&raw),
    })
}

fn extract_log_attributes(line: &str) -> BTreeMap<String, String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return BTreeMap::new();
    }

    let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(trimmed) else {
        return BTreeMap::new();
    };

    let mut attributes = BTreeMap::new();
    for (name, value) in map {
        let Some(name) = normalize_log_attribute_name(&name) else {
            continue;
        };
        if is_reserved_log_attribute(&name) {
            continue;
        }
        let Some(value) = log_attribute_value_to_string(&value) else {
            continue;
        };
        attributes.entry(name).or_insert(value);
    }
    attributes
}

fn normalize_log_attribute_name(field_name: &str) -> Option<String> {
    let mut normalized = String::with_capacity(field_name.len());
    let mut last_was_underscore = false;

    for ch in field_name.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_underscore = false;
        } else if !last_was_underscore {
            normalized.push('_');
            last_was_underscore = true;
        }
    }

    let normalized = normalized.trim_matches('_');
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn is_reserved_log_attribute(field_name: &str) -> bool {
    matches!(
        field_name,
        "time"
            | "ts"
            | "timestamp"
            | "msg"
            | "message"
            | "level"
            | "severity"
            | "stream"
            | "run_id"
            | "service"
            | "ts_nanos"
            | "seq"
            | "raw"
    )
}

fn log_attribute_value_to_string(value: &JsonValue) -> Option<String> {
    let value = match value {
        JsonValue::String(value) => value.clone(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) | JsonValue::Null => return None,
    };

    if value.is_empty() || value.chars().count() > 256 {
        None
    } else {
        Some(value)
    }
}

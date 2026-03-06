use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

static BRACKET_LEVEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*\[(error|fatal|panic|warn(?:ing)?|info|debug|trace)\](?:\s|$)")
        .unwrap()
});

static PREFIX_LEVEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:\d{4}-\d{2}-\d{2}T\S+\s+)?(error|fatal|panic|warn(?:ing)?|info|debug|trace)\b(?::|\s|$)",
    )
    .unwrap()
});

pub(crate) fn detect_log_level(content: &str) -> &'static str {
    let fast_level = detect_log_level_fast(content);
    if fast_level != "info" {
        return fast_level;
    }

    detect_log_level_regex(content).unwrap_or("info")
}

fn detect_log_level_fast(content: &str) -> &'static str {
    let bytes = content.as_bytes();

    if contains_keyword_case_insensitive(bytes, b"error")
        || contains_keyword_case_insensitive(bytes, b"fatal")
        || contains_keyword_case_insensitive(bytes, b"panic")
        || contains_keyword_case_insensitive(bytes, b"exception")
        || contains_keyword_case_insensitive(bytes, b"traceback")
    {
        return "error";
    }

    if contains_keyword_case_insensitive(bytes, b"warn")
        || contains_keyword_case_insensitive(bytes, b"warning")
    {
        return "warn";
    }

    "info"
}

fn detect_log_level_regex(content: &str) -> Option<&'static str> {
    for regex in [&*BRACKET_LEVEL_RE, &*PREFIX_LEVEL_RE] {
        if let Some(captures) = regex.captures(content)
            && let Some(level) = captures.get(1).map(|m| m.as_str())
        {
            return normalize_plain_text_level(level);
        }
    }

    None
}

fn normalize_plain_text_level(level: &str) -> Option<&'static str> {
    if level.eq_ignore_ascii_case("error")
        || level.eq_ignore_ascii_case("fatal")
        || level.eq_ignore_ascii_case("panic")
    {
        return Some("error");
    }

    if level.eq_ignore_ascii_case("warn") || level.eq_ignore_ascii_case("warning") {
        return Some("warn");
    }

    if level.eq_ignore_ascii_case("info") {
        return Some("info");
    }

    if level.eq_ignore_ascii_case("debug") {
        return Some("debug");
    }

    if level.eq_ignore_ascii_case("trace") {
        return Some("trace");
    }

    None
}

fn contains_keyword_case_insensitive(bytes: &[u8], keyword: &[u8]) -> bool {
    if bytes.len() < keyword.len() {
        return false;
    }

    for start in 0..=bytes.len() - keyword.len() {
        let end = start + keyword.len();
        if bytes[start..end].eq_ignore_ascii_case(keyword) && has_word_boundaries(bytes, start, end)
        {
            return true;
        }
    }

    false
}

fn has_word_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let before_ok = start == 0 || !is_word_char(bytes[start - 1]);
    let after_ok = end == bytes.len() || !is_word_char(bytes[end]);
    before_ok && after_ok
}

fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn parse_json_object(line: &str) -> Option<serde_json::Map<String, Value>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn normalize_numeric_level(level: i64) -> Option<&'static str> {
    match level {
        10 => Some("trace"),
        20 => Some("debug"),
        30 => Some("info"),
        40 => Some("warn"),
        50 | 60 => Some("error"),
        _ => None,
    }
}

fn normalize_level_token(token: &str) -> Option<String> {
    let lower = token.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }

    if let Ok(num) = lower.parse::<i64>() {
        return normalize_numeric_level(num).map(|lvl| lvl.to_string());
    }

    let normalized = match lower.as_str() {
        "warning" => "warn",
        "warn" => "warn",
        "error" | "err" | "fatal" | "panic" => "error",
        "trace" => "trace",
        "debug" => "debug",
        "info" | "information" => "info",
        _ => return Some(lower),
    };

    Some(normalized.to_string())
}

fn normalize_level_value(value: &Value) -> Option<String> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .and_then(normalize_numeric_level)
            .map(|lvl| lvl.to_string()),
        Value::String(s) => normalize_level_token(s),
        _ => None,
    }
}

fn extract_bracket_timestamp(line: &str) -> Option<String> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some(rest[..end].to_string())
}

fn extract_bracket_content(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('[')?;
    let after_ts = rest.find(']')?;
    let after = rest[after_ts + 1..].trim_start();
    let rest2 = after.strip_prefix('[')?;
    let end_stream = rest2.find(']')?;
    let stream = rest2[..end_stream].to_string();
    let content = rest2[end_stream + 1..].trim_start().to_string();
    Some((stream, content))
}

pub(crate) fn extract_timestamp_str(line: &str) -> Option<String> {
    if let Some(map) = parse_json_object(line) {
        for field in ["time", "ts", "timestamp"] {
            if let Some(value) = map.get(field)
                && let Some(ts) = value_as_string(value)
            {
                return Some(ts);
            }
        }
    }

    extract_bracket_timestamp(line)
}

pub(crate) fn parse_timestamp_nanos(ts: &str) -> Option<i64> {
    let dt = OffsetDateTime::parse(ts, &Rfc3339).ok()?;
    i64::try_from(dt.unix_timestamp_nanos()).ok()
}

pub(crate) fn extract_log_content(line: &str) -> (String, String) {
    if let Some(map) = parse_json_object(line) {
        let stream = map
            .get("stream")
            .and_then(value_as_string)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "stdout".to_string());
        let message = map
            .get("msg")
            .or_else(|| map.get("message"))
            .and_then(value_as_string)
            .unwrap_or_else(|| line.to_string());
        return (stream, message);
    }

    if let Some((stream, content)) = extract_bracket_content(line) {
        return (stream, content);
    }

    ("stdout".to_string(), line.to_string())
}

pub(crate) fn classify_line_level(line: &str) -> String {
    if let Some(map) = parse_json_object(line) {
        // Check explicit level/severity fields first.
        if let Some(level) = map
            .get("level")
            .or_else(|| map.get("severity"))
            .and_then(normalize_level_value)
        {
            return level;
        }

        // No explicit level — detect from msg content and stream.
        // This handles devstack's own wrapper format where the service output
        // (which may contain level indicators) is captured in the "msg" field.
        let msg = map
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let stream = map
            .get("stream")
            .and_then(|v| v.as_str())
            .unwrap_or("stdout");
        let detected = detect_log_level(msg);
        if detected == "info" && stream == "stderr" {
            return "warn".to_string();
        }
        return detected.to_string();
    }

    let (stream, content) = extract_log_content(line);
    let detected = detect_log_level(&content);
    if detected == "info" && stream == "stderr" {
        "warn".to_string()
    } else {
        detected.to_string()
    }
}

pub(crate) fn is_health_check_line(line: &str) -> bool {
    let (_, message) = extract_log_content(line);
    is_health_check_message(&message)
}

pub(crate) fn is_health_check_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();

    if lower.contains("kube-probe")
        || lower.contains("liveness probe")
        || lower.contains("readiness probe")
        || lower.contains("startup probe")
    {
        return true;
    }

    const METHODS: &[&str] = &["get", "head", "options"];
    const ENDPOINTS: &[&str] = &[
        "/health",
        "/api/health",
        "/healthz",
        "/livez",
        "/readyz",
        "/liveness",
        "/readiness",
    ];

    for method in METHODS {
        for endpoint in ENDPOINTS {
            if contains_request_for_endpoint(&lower, method, endpoint) {
                return true;
            }
        }
    }

    false
}

fn contains_request_for_endpoint(message_lower: &str, method: &str, endpoint: &str) -> bool {
    let needle = format!("{method} {endpoint}");
    let mut offset = 0;
    while let Some(found) = message_lower[offset..].find(&needle) {
        let start = offset + found;
        let boundary = message_lower
            .as_bytes()
            .get(start + needle.len())
            .copied();
        if matches!(boundary, None | Some(b' ') | Some(b'?') | Some(b'/') | Some(b'\"') | Some(b'\'')) {
            return true;
        }
        offset = start.saturating_add(1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_time_level_and_msg_are_extracted() {
        let line = r#"{"time":"2025-01-01T00:00:00Z","stream":"stderr","level":"error","msg":"boom"}"#;
        assert_eq!(extract_timestamp_str(line).as_deref(), Some("2025-01-01T00:00:00Z"));
        assert_eq!(extract_log_content(line), ("stderr".to_string(), "boom".to_string()));
        assert_eq!(classify_line_level(line), "error");
    }

    #[test]
    fn json_extracts_ts_aliases() {
        let line_ts = r#"{"ts":"2025-01-01T00:00:01Z","msg":"hello"}"#;
        let line_timestamp = r#"{"timestamp":"2025-01-01T00:00:02Z","msg":"hello"}"#;
        assert_eq!(extract_timestamp_str(line_ts).as_deref(), Some("2025-01-01T00:00:01Z"));
        assert_eq!(
            extract_timestamp_str(line_timestamp).as_deref(),
            Some("2025-01-01T00:00:02Z")
        );
    }

    #[test]
    fn json_pino_numeric_levels_are_normalized() {
        let cases = [
            (10, "trace"),
            (20, "debug"),
            (30, "info"),
            (40, "warn"),
            (50, "error"),
            (60, "error"),
        ];
        for (raw, expected) in cases {
            let line = format!(r#"{{"level":{},"msg":"x"}}"#, raw);
            assert_eq!(classify_line_level(&line), expected);
        }
    }

    #[test]
    fn json_string_levels_are_normalized() {
        let cases = [
            ("ERROR", "error"),
            ("Warning", "warn"),
            ("warn", "warn"),
            ("INFO", "info"),
        ];
        for (raw, expected) in cases {
            let line = format!(r#"{{"level":"{}","msg":"x"}}"#, raw);
            assert_eq!(classify_line_level(&line), expected);
        }
    }

    #[test]
    fn json_message_aliases_and_missing_fields_work() {
        let line_message = r#"{"message":"hello","stream":"stdout"}"#;
        assert_eq!(
            extract_log_content(line_message),
            ("stdout".to_string(), "hello".to_string())
        );

        let no_level = r#"{"msg":"hello"}"#;
        assert_eq!(classify_line_level(no_level), "info");

        let no_timestamp = r#"{"msg":"hello"}"#;
        assert_eq!(extract_timestamp_str(no_timestamp), None);
    }

    #[test]
    fn json_wrapper_detects_level_from_msg_content() {
        // Devstack wraps service output as {"msg":"...", "stream":"...", "time":"..."}
        // When there's no explicit level field, detect from msg content.
        let error_line = r#"{"msg":"[ERROR] Something went wrong!","stream":"stderr","time":"2025-01-01T00:00:00Z"}"#;
        assert_eq!(classify_line_level(error_line), "error");

        let warn_line = r#"{"msg":"[WARN] Disk space low","stream":"stderr","time":"2025-01-01T00:00:00Z"}"#;
        assert_eq!(classify_line_level(warn_line), "warn");

        // stderr with no level indicators in msg → "warn"
        let stderr_info = r#"{"msg":"some output","stream":"stderr","time":"2025-01-01T00:00:00Z"}"#;
        assert_eq!(classify_line_level(stderr_info), "warn");

        // stdout with no level indicators → "info"
        let stdout_info = r#"{"msg":"server started","stream":"stdout","time":"2025-01-01T00:00:00Z"}"#;
        assert_eq!(classify_line_level(stdout_info), "info");

        // Traceback pattern in msg
        let traceback = r#"{"msg":"Traceback (most recent call last):","stream":"stderr"}"#;
        assert_eq!(classify_line_level(traceback), "error");
    }

    #[test]
    fn bracket_format_still_parses() {
        let line = "[2025-01-01T00:00:00Z] [stderr] Warning: old format";
        assert_eq!(extract_timestamp_str(line).as_deref(), Some("2025-01-01T00:00:00Z"));
        assert_eq!(
            extract_log_content(line),
            ("stderr".to_string(), "Warning: old format".to_string())
        );
        assert_eq!(classify_line_level(line), "warn");
    }

    #[test]
    fn plain_text_falls_back_to_defaults() {
        let line = "plain text line";
        assert_eq!(extract_timestamp_str(line), None);
        assert_eq!(
            extract_log_content(line),
            ("stdout".to_string(), "plain text line".to_string())
        );
        assert_eq!(classify_line_level(line), "info");
    }

    #[test]
    fn health_checks_detected_in_json_message() {
        let line = r#"{"msg":"GET /health HTTP/1.1 200","stream":"stdout"}"#;
        assert!(is_health_check_line(line));
    }

    #[test]
    fn malformed_json_falls_back_to_plain_text() {
        let line = "{\"level\":\"error\",\"msg\":\"oops\"";
        assert_eq!(extract_timestamp_str(line), None);
        assert_eq!(
            extract_log_content(line),
            ("stdout".to_string(), line.to_string())
        );
        assert_eq!(classify_line_level(line), "error");
    }

    #[test]
    fn detect_log_level_finds_error_and_warn() {
        assert_eq!(detect_log_level("something Error happened"), "error");
        assert_eq!(detect_log_level("Traceback (most recent call)"), "error");
        assert_eq!(detect_log_level("server started"), "info");
    }

    #[test]
    fn detect_log_level_matches_plain_text_prefix_patterns() {
        let cases = [
            ("[ERROR] connection pool exhausted", "error"),
            ("[WARN] slow query", "warn"),
            ("[INFO] request handled", "info"),
            ("ERROR: something failed", "error"),
            ("warn: deprecated", "warn"),
            ("ERROR something failed", "error"),
            ("2026-03-05T04:39:01Z error: timeout", "error"),
            ("FATAL: crash", "error"),
            ("WARNING: deprecated", "warn"),
            ("DEBUG: trace enabled", "debug"),
            ("TRACE startup complete", "trace"),
            ("plain text with no level", "info"),
            ("no errors found", "info"),
        ];

        for (line, expected) in cases {
            assert_eq!(detect_log_level(line), expected, "line: {line}");
        }
    }
}

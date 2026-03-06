use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use regex::Regex;
use std::sync::LazyLock;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\x1b",       // ESC
        r"(?:",
        r"\[[0-9;?]*[A-Za-z]",   // CSI sequences: ESC [ ... letter (colors, cursor, etc.)
        r"|\][^\x07\x1b]*(?:\x07|\x1b\\)",  // OSC sequences: ESC ] ... BEL or ESC \
        r"|\([A-B]",              // charset selection: ESC ( A/B
        r"|[=>NOMDEHcZ78]",      // simple escape codes
        r")",
    ))
    .unwrap()
});

pub fn strip_ansi(input: &str) -> String {
    ANSI_RE.replace_all(input, "").into_owned()
}

pub fn contains_ansi(input: &str) -> bool {
    memchr::memchr(0x1b, input.as_bytes()).is_some()
}

pub fn strip_ansi_if_needed(input: &str) -> String {
    if contains_ansi(input) {
        strip_ansi(input)
    } else {
        input.to_string()
    }
}

pub fn sanitize_env_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

pub fn validate_name_for_path_component(kind: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(anyhow::anyhow!("{kind} name cannot be empty"));
    }
    if value == "." || value == ".." {
        return Err(anyhow::anyhow!("invalid {kind} name '{value}'"));
    }
    let valid = value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    if !valid {
        return Err(anyhow::anyhow!(
            "invalid {kind} name '{value}' (allowed: A-Z, a-z, 0-9, '.', '_', '-')"
        ));
    }
    Ok(())
}

pub fn now_rfc3339() -> String {
    format_rfc3339(SystemTime::now())
}

pub fn format_rfc3339(ts: SystemTime) -> String {
    let dt: OffsetDateTime = ts.into();
    dt.format(&Rfc3339).unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .context("atomic_write requires parent dir")?;
    fs::create_dir_all(dir).with_context(|| format!("create dir {dir:?}"))?;

    let tmp_path = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp_path)
            .with_context(|| format!("create tmp file {tmp_path:?}"))?;
        file.write_all(data)
            .with_context(|| format!("write tmp file {tmp_path:?}"))?;
        file.sync_all().ok();
    }
    fs::rename(&tmp_path, path)
        .with_context(|| format!("rename tmp to {path:?}"))?;
    // Best-effort fsync on directory to persist rename.
    if let Ok(dir_file) = File::open(dir) {
        let _ = dir_file.sync_all();
    }
    Ok(())
}

pub fn project_hash(path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let hash = hasher.finalize();
    hash.to_hex()[..12].to_string()
}

pub fn ensure_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub fn expand_home(path: &Path) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("~")
        && let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\x1b[31mERROR\x1b[0m: something failed"), "ERROR: something failed");
        assert_eq!(strip_ansi("\x1b[1;32m✓\x1b[0m ready"), "✓ ready");
    }

    #[test]
    fn strip_ansi_handles_256_color_and_rgb() {
        assert_eq!(strip_ansi("\x1b[38;5;196mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[38;2;255;0;0mred\x1b[0m"), "red");
    }

    #[test]
    fn strip_ansi_preserves_plain_text() {
        let plain = "2025-01-15T10:30:00Z [stdout] server started on port 3000";
        assert_eq!(strip_ansi(plain), plain);
    }

    #[test]
    fn strip_ansi_handles_osc_sequences() {
        assert_eq!(strip_ansi("\x1b]0;window title\x07text"), "text");
        assert_eq!(strip_ansi("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"), "link");
    }

    #[test]
    fn strip_ansi_handles_cursor_and_erase() {
        assert_eq!(strip_ansi("\x1b[2Koverwritten line"), "overwritten line");
        assert_eq!(strip_ansi("\x1b[A\x1b[2Kprogress: 100%"), "progress: 100%");
    }

    #[test]
    fn contains_ansi_detects_escapes() {
        assert!(!contains_ansi("plain text"));
        assert!(contains_ansi("\x1b[31mred\x1b[0m"));
    }

    #[test]
    fn strip_ansi_if_needed_skips_clean_text() {
        let plain = "no escapes here";
        assert_eq!(strip_ansi_if_needed(plain), plain);
    }

    #[test]
    fn sanitize_env_key_uppercases_and_replaces() {
        assert_eq!(sanitize_env_key("api"), "API");
        assert_eq!(sanitize_env_key("web-service"), "WEB_SERVICE");
        assert_eq!(sanitize_env_key("with space"), "WITH_SPACE");
        assert_eq!(sanitize_env_key(""), "_");
    }

    #[test]
    fn validate_name_for_path_component_rejects_invalid_values() {
        assert!(validate_name_for_path_component("service", "api").is_ok());
        assert!(validate_name_for_path_component("service", "api-v2").is_ok());
        assert!(validate_name_for_path_component("service", "../escape").is_err());
        assert!(validate_name_for_path_component("service", "nested/path").is_err());
        assert!(validate_name_for_path_component("service", "").is_err());
    }

    #[test]
    fn atomic_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        atomic_write(&path, b"hello").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello");
    }
}

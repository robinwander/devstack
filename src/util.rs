use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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

pub fn now_rfc3339() -> String {
    format_rfc3339(SystemTime::now())
}

pub fn format_rfc3339(ts: SystemTime) -> String {
    let dt: OffsetDateTime = ts.into();
    dt.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().context("atomic_write requires parent dir")?;
    fs::create_dir_all(dir).with_context(|| format!("create dir {dir:?}"))?;

    let tmp_path = path.with_extension("tmp");
    {
        let mut file =
            File::create(&tmp_path).with_context(|| format!("create tmp file {tmp_path:?}"))?;
        file.write_all(data)
            .with_context(|| format!("write tmp file {tmp_path:?}"))?;
        file.sync_all().ok();
    }
    fs::rename(&tmp_path, path).with_context(|| format!("rename tmp to {path:?}"))?;
    if let Ok(dir_file) = File::open(dir) {
        let _ = dir_file.sync_all();
    }
    Ok(())
}

pub fn ensure_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub fn expand_home(path: &Path) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("~")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(stripped);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_env_key_uppercases_and_replaces() {
        assert_eq!(sanitize_env_key("api"), "API");
        assert_eq!(sanitize_env_key("web-service"), "WEB_SERVICE");
        assert_eq!(sanitize_env_key("with space"), "WITH_SPACE");
        assert_eq!(sanitize_env_key(""), "_");
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

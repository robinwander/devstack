use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::util::atomic_write;

/// Generic helper for loading JSON from disk
pub fn load_json<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let data = std::fs::read(path).with_context(|| format!("read file {path:?}"))?;
    serde_json::from_slice(&data).with_context(|| format!("parse JSON from {path:?}"))
}

/// Generic helper for saving JSON to disk atomically
pub fn save_json<T>(path: &Path, data: &T) -> Result<()>
where
    T: Serialize,
{
    let json = serde_json::to_vec_pretty(data).context("serialize to JSON")?;
    atomic_write(path, &json).with_context(|| format!("write JSON to {path:?}"))
}

/// Load JSON with a fallback default value if file doesn't exist
pub fn load_json_or_default<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    if path.exists() {
        load_json(path)
    } else {
        Ok(T::default())
    }
}
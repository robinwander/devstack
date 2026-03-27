use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::TaskConfig;
use crate::paths;
use crate::paths::validate_name_for_path_component;

pub fn compute_watch_hash(cwd: &Path, watch: &[String]) -> Result<String> {
    crate::watch::compute_watch_hash(cwd, Some(watch), &[], &[], b"task-watch-v1")
}

pub fn load_stored_hash(project_dir: &Path, task_name: &str) -> Result<Option<String>> {
    let path = task_hash_path(project_dir, task_name)?;
    if !path.exists() {
        return Ok(None);
    }
    let value = std::fs::read_to_string(&path)
        .with_context(|| format!("read task hash {}", path.display()))?;
    Ok(Some(value.trim().to_string()))
}

pub fn store_hash(project_dir: &Path, task_name: &str, hash: &str) -> Result<()> {
    let dir = paths::task_hashes_dir(project_dir)?;
    std::fs::create_dir_all(&dir)?;
    let path = task_hash_path(project_dir, task_name)?;
    std::fs::write(&path, hash.as_bytes())
        .with_context(|| format!("write task hash {}", path.display()))?;
    Ok(())
}

pub(crate) fn task_hash_path(project_dir: &Path, task_name: &str) -> Result<PathBuf> {
    validate_name_for_path_component("task", task_name)?;
    Ok(paths::task_hashes_dir(project_dir)?.join(format!("{task_name}.sha256")))
}

pub(crate) fn task_cmd_parts(
    task: &TaskConfig,
) -> (
    String,
    Option<PathBuf>,
    BTreeMap<String, String>,
    Option<PathBuf>,
) {
    match task {
        TaskConfig::Command(cmd) => (cmd.clone(), None, BTreeMap::new(), None),
        TaskConfig::Structured(def) => (
            def.cmd.clone(),
            def.cwd.clone(),
            def.env.clone(),
            def.env_file.clone(),
        ),
    }
}

pub fn task_watch(task: &TaskConfig) -> Vec<String> {
    match task {
        TaskConfig::Command(_) => Vec::new(),
        TaskConfig::Structured(def) => def.watch.clone(),
    }
}

pub fn task_cwd(task: &TaskConfig, project_dir: &Path) -> PathBuf {
    match task {
        TaskConfig::Command(_) => project_dir.to_path_buf(),
        TaskConfig::Structured(def) => match &def.cwd {
            Some(p) if p.is_absolute() => p.clone(),
            Some(p) => project_dir.join(p),
            None => project_dir.to_path_buf(),
        },
    }
}

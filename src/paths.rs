use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::BaseDirs;

use crate::ids::{RunId, ServiceName};
use crate::util::expand_home;

pub fn absolutize_path(base: &Path, path: impl AsRef<Path>) -> PathBuf {
    let expanded = expand_home(path.as_ref());
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
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

pub fn project_hash(path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let hash = hasher.finalize();
    hash.to_hex()[..12].to_string()
}

pub fn base_dir() -> Result<PathBuf> {
    if let Some(base) = BaseDirs::new() {
        return Ok(base.data_dir().join("devstack"));
    }
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".local/share/devstack"))
}

pub fn daemon_dir() -> Result<PathBuf> {
    Ok(base_dir()?.join("daemon"))
}

pub fn daemon_socket_path() -> Result<PathBuf> {
    Ok(daemon_dir()?.join("devstackd.sock"))
}

pub fn daemon_state_path() -> Result<PathBuf> {
    Ok(daemon_dir()?.join("state.json"))
}

pub fn daemon_lock_path() -> Result<PathBuf> {
    Ok(daemon_dir()?.join("daemon.lock"))
}

pub fn runs_dir() -> Result<PathBuf> {
    Ok(base_dir()?.join("runs"))
}

pub fn run_dir(run_id: &RunId) -> Result<PathBuf> {
    validate_name_for_path_component("run id", run_id.as_str())?;
    Ok(runs_dir()?.join(run_id.as_str()))
}

pub fn run_manifest_path(run_id: &RunId) -> Result<PathBuf> {
    Ok(run_dir(run_id)?.join("manifest.json"))
}

pub fn run_snapshot_path(run_id: &RunId) -> Result<PathBuf> {
    Ok(run_dir(run_id)?.join("devstack.yml.snapshot"))
}

pub fn run_logs_dir(run_id: &RunId) -> Result<PathBuf> {
    Ok(run_dir(run_id)?.join("logs"))
}

pub fn run_log_path(run_id: &RunId, service: &ServiceName) -> Result<PathBuf> {
    validate_name_for_path_component("service", service.as_str())?;
    Ok(run_logs_dir(run_id)?.join(format!("{}.log", service.as_str())))
}

pub fn run_task_logs_dir(run_id: &RunId) -> Result<PathBuf> {
    validate_name_for_path_component("run id", run_id.as_str())?;
    Ok(base_dir()?.join("task-logs").join(run_id.as_str()))
}

pub fn task_log_path(run_id: &RunId, task_name: &str) -> Result<PathBuf> {
    validate_name_for_path_component("task", task_name)?;
    Ok(run_task_logs_dir(run_id)?.join(format!("{task_name}.log")))
}

pub fn task_history_path(run_id: &RunId) -> Result<PathBuf> {
    Ok(run_task_logs_dir(run_id)?.join("history.json"))
}

pub fn globals_root() -> Result<PathBuf> {
    Ok(base_dir()?.join("globals"))
}

pub fn global_key(project_dir: &Path, name: &str) -> Result<String> {
    validate_name_for_path_component("service", name)?;
    Ok(format!("{}__{}", project_hash(project_dir), name))
}

pub fn global_dir(project_dir: &Path, name: &str) -> Result<PathBuf> {
    Ok(globals_root()?.join(global_key(project_dir, name)?))
}

pub fn global_manifest_path(project_dir: &Path, name: &str) -> Result<PathBuf> {
    Ok(global_dir(project_dir, name)?.join("manifest.json"))
}

pub fn global_logs_dir(project_dir: &Path, name: &str) -> Result<PathBuf> {
    Ok(global_dir(project_dir, name)?.join("logs"))
}

pub fn global_log_path(project_dir: &Path, name: &str) -> Result<PathBuf> {
    validate_name_for_path_component("service", name)?;
    Ok(global_logs_dir(project_dir, name)?.join(format!("{}.log", name)))
}

pub fn task_hashes_dir(project_dir: &Path) -> Result<PathBuf> {
    Ok(base_dir()?
        .join("task-hashes")
        .join(project_hash(project_dir)))
}

pub fn ad_hoc_task_logs_dir(project_dir: &Path) -> Result<PathBuf> {
    Ok(base_dir()?
        .join("task-logs")
        .join(format!("adhoc-{}", project_hash(project_dir))))
}

pub fn ad_hoc_task_log_path(project_dir: &Path, task_name: &str) -> Result<PathBuf> {
    validate_name_for_path_component("task", task_name)?;
    Ok(ad_hoc_task_logs_dir(project_dir)?.join(format!("{task_name}.log")))
}

pub fn ad_hoc_task_history_path(project_dir: &Path) -> Result<PathBuf> {
    Ok(ad_hoc_task_logs_dir(project_dir)?.join("history.json"))
}

pub fn dashboard_dir() -> Result<PathBuf> {
    Ok(base_dir()?.join("dashboard"))
}

pub fn logs_index_dir() -> Result<PathBuf> {
    Ok(base_dir()?.join("logs_index"))
}

pub fn logs_ingest_state_path() -> Result<PathBuf> {
    Ok(logs_index_dir()?.join("ingest_state.json"))
}

pub fn projects_path() -> Result<PathBuf> {
    Ok(base_dir()?.join("projects.json"))
}

pub fn sources_path() -> Result<PathBuf> {
    Ok(base_dir()?.join("sources.json"))
}

pub fn ensure_base_layout() -> Result<()> {
    std::fs::create_dir_all(daemon_dir()?)?;
    std::fs::create_dir_all(runs_dir()?)?;
    std::fs::create_dir_all(globals_root()?)?;
    std::fs::create_dir_all(base_dir()?.join("task-logs"))?;
    std::fs::create_dir_all(dashboard_dir()?)?;
    std::fs::create_dir_all(logs_index_dir()?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_run_ids() {
        let run_id = RunId::new("../escape");
        assert!(run_dir(&run_id).is_err());
        assert!(run_manifest_path(&run_id).is_err());
    }

    #[test]
    fn rejects_invalid_service_names_in_run_logs() {
        let run_id = RunId::new("run-1");
        let service = ServiceName::new("../escape");
        assert!(run_log_path(&run_id, &service).is_err());
    }

    #[test]
    fn rejects_invalid_task_names_in_task_logs() {
        let run_id = RunId::new("run-1");
        assert!(task_log_path(&run_id, "../escape").is_err());
        assert!(ad_hoc_task_log_path(Path::new("/tmp/project"), "nested/path").is_err());
    }

    #[test]
    fn rejects_invalid_global_service_names() {
        let project_dir = Path::new("/tmp/project");
        assert!(global_dir(project_dir, "bad/../../global").is_err());
        assert!(global_manifest_path(project_dir, "../global").is_err());
        assert!(global_log_path(project_dir, "nested/path").is_err());
    }

    #[test]
    fn validate_name_for_path_component_rejects_invalid_values() {
        assert!(validate_name_for_path_component("service", "api").is_ok());
        assert!(validate_name_for_path_component("service", "api-v2").is_ok());
        assert!(validate_name_for_path_component("service", "../escape").is_err());
        assert!(validate_name_for_path_component("service", "nested/path").is_err());
        assert!(validate_name_for_path_component("service", "").is_err());
    }
}

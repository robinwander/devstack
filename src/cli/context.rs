use std::cmp::Ordering;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::Serialize;
use serde::de::DeserializeOwned;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::api::{RunListResponse, RunSummary};
use crate::config::ConfigFile;
use crate::infra::ipc::UnixDaemonClient;
use crate::model::RunLifecycle;
use crate::paths;
use crate::persistence::PersistedRun;

pub(crate) const DAEMON_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const DAEMON_LONG_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_NONINTERACTIVE_FOLLOW_FOR: Duration = Duration::from_secs(15);

pub(crate) struct CliContext {
    pub(crate) interactive: bool,
    pub(crate) pretty: bool,
    daemon: UnixDaemonClient,
}

impl CliContext {
    pub(crate) fn new(interactive: bool, pretty: bool) -> Self {
        Self {
            interactive,
            pretty,
            daemon: UnixDaemonClient::for_cli(),
        }
    }

    pub(crate) async fn daemon_request<T: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: Option<T>,
        timeout_duration: Option<Duration>,
    ) -> Result<serde_json::Value> {
        self.daemon
            .request(method, path, body, timeout_duration)
            .await
    }

    pub(crate) async fn daemon_request_json<T, R>(
        &self,
        method: &str,
        path: &str,
        body: Option<T>,
        timeout_duration: Option<Duration>,
    ) -> Result<R>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        self.daemon
            .request_json(method, path, body, timeout_duration)
            .await
    }

    pub(crate) fn daemon_is_running(&self) -> bool {
        self.daemon
            .socket_path()
            .map(|path| path.exists())
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectContext {
    pub(crate) project_dir: PathBuf,
    pub(crate) config_path: Option<PathBuf>,
}

pub(crate) fn is_interactive() -> bool {
    std::io::stdout().is_terminal()
}

pub(crate) fn resolve_pretty(explicit: bool, interactive: bool) -> bool {
    explicit || interactive
}

pub(crate) fn resolve_follow_for(
    follow: bool,
    follow_for: Option<Duration>,
    interactive: bool,
) -> Option<Duration> {
    if !follow {
        return None;
    }
    if follow_for.is_some() {
        return follow_for;
    }
    if interactive {
        None
    } else {
        Some(DEFAULT_NONINTERACTIVE_FOLLOW_FOR)
    }
}

pub(crate) fn resolve_project_dir_from_cwd() -> Result<PathBuf> {
    Ok(resolve_project_context(None, None)?.project_dir)
}

pub(crate) fn resolve_project_context(
    project: Option<PathBuf>,
    file: Option<PathBuf>,
) -> Result<ProjectContext> {
    let cwd = std::env::current_dir()?;
    resolve_project_context_with_cwd(&cwd, project, file)
}

pub(crate) fn resolve_project_context_with_cwd(
    cwd: &Path,
    project: Option<PathBuf>,
    file: Option<PathBuf>,
) -> Result<ProjectContext> {
    if let Some(file) = file {
        let file_path = paths::absolutize_path(cwd, file);
        let project_dir = if let Some(project) = project {
            paths::absolutize_path(cwd, project)
        } else {
            file_path.parent().unwrap_or(cwd).to_path_buf()
        };
        return Ok(ProjectContext {
            project_dir,
            config_path: Some(file_path),
        });
    }

    if let Some(project) = project {
        let project_dir = paths::absolutize_path(cwd, project);
        let config_path = ConfigFile::default_path(&project_dir);
        return Ok(ProjectContext {
            project_dir,
            config_path: Some(config_path),
        });
    }

    if let Some(config_path) = ConfigFile::find_nearest_path(cwd) {
        let project_dir = config_path.parent().unwrap_or(cwd).to_path_buf();
        return Ok(ProjectContext {
            project_dir,
            config_path: Some(config_path),
        });
    }

    Ok(ProjectContext {
        project_dir: cwd.to_path_buf(),
        config_path: None,
    })
}

fn looks_like_config_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.ends_with(".toml") || lower.ends_with(".yaml") || lower.ends_with(".yml")
}

pub(crate) fn resolve_up_context(
    stack: Option<String>,
    project: Option<PathBuf>,
    file: Option<PathBuf>,
) -> Result<(ProjectContext, Option<String>)> {
    let cwd = std::env::current_dir()?;
    let mut stack = stack;
    let mut file = file;

    if file.is_none()
        && let Some(candidate) = stack.as_ref()
        && looks_like_config_path(candidate)
    {
        let path = paths::absolutize_path(&cwd, PathBuf::from(candidate));
        if path.is_file() {
            file = Some(path);
            stack = None;
        }
    }

    let context = resolve_project_context_with_cwd(&cwd, project, file)?;
    Ok((context, stack))
}

pub(crate) fn resolve_stack_name(
    stack: Option<String>,
    config_path: Option<&Path>,
) -> Result<String> {
    if let Some(name) = stack {
        if let Some(config_path) = config_path.filter(|p| p.is_file())
            && let Ok(config) = ConfigFile::load_from_path(config_path)
            && !config.stacks.as_map().contains_key(&name)
        {
            let available = config.stacks.as_map().keys().cloned().collect::<Vec<_>>();
            if !available.is_empty() {
                return Err(anyhow!(
                    "stack '{name}' not found in {}; available stacks: {}",
                    config_path.to_string_lossy(),
                    available.join(", ")
                ));
            }
        }
        return Ok(name);
    }

    let config_path = config_path
        .ok_or_else(|| anyhow!("no devstack config found; run devstack init or pass --file"))?;
    if !config_path.is_file() {
        return Err(anyhow!(
            "config not found at {}; run devstack init or pass --file",
            config_path.to_string_lossy()
        ));
    }
    let config = ConfigFile::load_from_path(config_path)?;
    if let Some(default_stack) = &config.default_stack {
        return Ok(default_stack.clone());
    }
    let stacks: Vec<String> = config.stacks.as_map().keys().cloned().collect();
    match stacks.len() {
        0 => Err(anyhow!(
            "no stacks defined in {}; add one or pass --stack",
            config_path.to_string_lossy()
        )),
        1 => Ok(stacks[0].clone()),
        _ => Err(anyhow!(
            "multiple stacks found in {}: {} (use --stack or default_stack)",
            config_path.to_string_lossy(),
            stacks.join(", ")
        )),
    }
}

pub(crate) async fn fetch_runs_with_fallback(
    context: &CliContext,
) -> Result<(RunListResponse, bool)> {
    match context
        .daemon_request_json::<(), RunListResponse>("GET", "/v1/runs", None, Some(DAEMON_TIMEOUT))
        .await
    {
        Ok(runs) => Ok((runs, false)),
        Err(err) => {
            if let Ok(runs) = runs_from_disk() {
                Ok((RunListResponse { runs }, true))
            } else {
                Err(err)
            }
        }
    }
}

pub(crate) async fn fetch_runs(context: &CliContext) -> Result<RunListResponse> {
    fetch_runs_with_fallback(context)
        .await
        .map(|(runs, _)| runs)
}

fn runs_from_disk() -> Result<Vec<RunSummary>> {
    let mut runs = Vec::new();
    let root = paths::runs_dir()?;
    if !root.exists() {
        return Ok(runs);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest = match PersistedRun::load_from_path(&manifest_path) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        runs.push(RunSummary {
            run_id: manifest.run_id,
            stack: manifest.stack,
            project_dir: manifest.project_dir,
            state: manifest.state,
            created_at: manifest.created_at,
            stopped_at: manifest.stopped_at,
        });
    }
    Ok(runs)
}

pub(crate) fn status_from_manifest(run_id: &str) -> Result<crate::api::RunStatusResponse> {
    let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(run_id))?;
    let manifest = PersistedRun::load_from_path(&manifest_path)?;
    let desired = if manifest.state == crate::model::RunLifecycle::Stopped {
        "stopped".to_string()
    } else {
        "running".to_string()
    };
    let mut services = std::collections::BTreeMap::new();
    for (name, svc) in manifest.services {
        services.insert(
            name,
            crate::api::ServiceStatus {
                desired: desired.clone(),
                systemd: None,
                ready: svc.state == crate::model::ServiceState::Ready,
                state: svc.state,
                last_failure: svc.last_failure,
                health: None,
                health_check_stats: None,
                uptime_seconds: None,
                recent_errors: Vec::new(),
                url: svc.url,
                auto_restart: false,
                watch_paused: false,
                watch_active: false,
            },
        );
    }
    Ok(crate::api::RunStatusResponse {
        run_id: manifest.run_id,
        stack: manifest.stack,
        project_dir: manifest.project_dir,
        state: manifest.state,
        services,
    })
}

pub(crate) async fn resolve_run_id(
    context: &CliContext,
    project_dir: &Path,
    run_id: Option<String>,
) -> Result<String> {
    if let Some(run_id) = run_id {
        return Ok(run_id);
    }
    let runs = fetch_runs(context).await?;
    let latest = select_latest_run(&runs.runs, project_dir).ok_or_else(|| {
        anyhow!(
            "no runs found for project {} (use --run or devstack ls --all)",
            project_dir.to_string_lossy()
        )
    })?;
    Ok(latest.run_id.clone())
}

pub(crate) async fn resolve_latest_run_id(
    context: &CliContext,
    project_dir: &Path,
) -> Result<Option<String>> {
    let (runs, _) = fetch_runs_with_fallback(context).await?;
    Ok(select_latest_run(&runs.runs, project_dir).map(|run| run.run_id.clone()))
}

pub(crate) async fn resolve_active_run_id(
    context: &CliContext,
    project_dir: &Path,
) -> Result<Option<String>> {
    let (runs, _) = fetch_runs_with_fallback(context).await?;
    Ok(select_latest_active_run(&runs.runs, project_dir).map(|run| run.run_id.clone()))
}

pub(crate) fn select_latest_run<'a>(
    runs: &'a [RunSummary],
    project_dir: &Path,
) -> Option<&'a RunSummary> {
    runs.iter()
        .filter(|run| same_project_dir(&run.project_dir, project_dir))
        .max_by(|a, b| compare_run_recency(a, b))
}

pub(crate) fn select_latest_active_run<'a>(
    runs: &'a [RunSummary],
    project_dir: &Path,
) -> Option<&'a RunSummary> {
    runs.iter()
        .filter(|run| same_project_dir(&run.project_dir, project_dir))
        .filter(|run| run.state != RunLifecycle::Stopped && run.stopped_at.is_none())
        .max_by(|a, b| compare_run_recency(a, b))
}

fn compare_run_recency(a: &RunSummary, b: &RunSummary) -> Ordering {
    let a_time = OffsetDateTime::parse(&a.created_at, &Rfc3339).ok();
    let b_time = OffsetDateTime::parse(&b.created_at, &Rfc3339).ok();
    match (a_time, b_time) {
        (Some(a_time), Some(b_time)) => a_time.cmp(&b_time),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => a.created_at.cmp(&b.created_at),
    }
}

pub(crate) fn same_project_dir(run_project_dir: &str, project_dir: &Path) -> bool {
    let run_path = PathBuf::from(run_project_dir);
    if run_path == project_dir {
        return true;
    }
    let run_canon = std::fs::canonicalize(&run_path).unwrap_or(run_path);
    let project_canon =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    run_canon == project_canon
}

pub(crate) fn sort_runs_for_project(runs: &mut [RunSummary], project_dir: &Path) {
    runs.sort_by(|a, b| {
        let a_matches = same_project_dir(&a.project_dir, project_dir);
        let b_matches = same_project_dir(&b.project_dir, project_dir);
        match (a_matches, b_matches) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => compare_run_recency(b, a),
        }
    });
}

#[cfg(test)]
pub(crate) fn require_existing_socket(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        return Ok(());
    }
    Err(anyhow!(
        "daemon socket missing at {}; run `devstack daemon` (foreground) or `devstack install` (system service)",
        socket_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn summary(run_id: &str, project_dir: &str, created_at: &str) -> RunSummary {
        RunSummary {
            run_id: run_id.to_string(),
            stack: "app".to_string(),
            project_dir: project_dir.to_string(),
            state: RunLifecycle::Running,
            created_at: created_at.to_string(),
            stopped_at: None,
        }
    }

    #[test]
    fn select_latest_run_prefers_newest() {
        let runs = vec![
            summary("run-a", "/tmp/project", "2025-01-01T00:00:00Z"),
            summary("run-b", "/tmp/project", "2025-01-02T00:00:00Z"),
        ];
        let latest = select_latest_run(&runs, Path::new("/tmp/project")).unwrap();
        assert_eq!(latest.run_id, "run-b");
    }

    #[test]
    fn select_latest_run_filters_by_project() {
        let runs = vec![
            summary("run-a", "/tmp/project-a", "2025-01-02T00:00:00Z"),
            summary("run-b", "/tmp/project-b", "2025-01-03T00:00:00Z"),
        ];
        let latest = select_latest_run(&runs, Path::new("/tmp/project-a")).unwrap();
        assert_eq!(latest.run_id, "run-a");
    }

    #[test]
    fn select_latest_active_run_skips_stopped_runs() {
        let runs = vec![
            summary("run-old", "/tmp/project", "2025-01-01T00:00:00Z"),
            RunSummary {
                run_id: "run-stopped".to_string(),
                stack: "app".to_string(),
                project_dir: "/tmp/project".to_string(),
                state: RunLifecycle::Stopped,
                created_at: "2025-01-03T00:00:00Z".to_string(),
                stopped_at: Some("2025-01-03T01:00:00Z".to_string()),
            },
            summary("run-active", "/tmp/project", "2025-01-02T00:00:00Z"),
        ];

        let latest = select_latest_active_run(&runs, Path::new("/tmp/project")).unwrap();
        assert_eq!(latest.run_id, "run-active");
    }

    #[test]
    fn resolve_stack_name_chooses_only_stack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devstack.toml");
        fs::write(
            &path,
            "version = 1\n[stacks.app.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let name = resolve_stack_name(None, Some(&path)).unwrap();
        assert_eq!(name, "app");
    }

    #[test]
    fn resolve_stack_name_errors_on_multiple() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devstack.toml");
        fs::write(
            &path,
            "version = 1\n[stacks.app.services.api]\ncmd = \"echo\"\n[stacks.other.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let err = resolve_stack_name(None, Some(&path)).unwrap_err();
        assert!(err.to_string().contains("multiple stacks found"));
    }

    #[test]
    fn resolve_stack_name_validates_when_config_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devstack.toml");
        fs::write(
            &path,
            "version = 1\n[stacks.app.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let err = resolve_stack_name(Some("missing".to_string()), Some(&path)).unwrap_err();
        assert!(err.to_string().contains("available stacks"));
    }

    #[test]
    fn resolve_stack_name_uses_default_stack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devstack.toml");
        fs::write(
            &path,
            "version = 1\ndefault_stack = \"other\"\n[stacks.app.services.api]\ncmd = \"echo\"\n[stacks.other.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let name = resolve_stack_name(None, Some(&path)).unwrap();
        assert_eq!(name, "other");
    }

    #[test]
    fn sort_runs_for_project_orders_matches_first() {
        let mut runs = vec![
            summary("run-a", "/tmp/project-a", "2025-01-02T00:00:00Z"),
            summary("run-b", "/tmp/project-b", "2025-01-03T00:00:00Z"),
            summary("run-c", "/tmp/project-a", "2025-01-04T00:00:00Z"),
        ];
        sort_runs_for_project(&mut runs, Path::new("/tmp/project-a"));
        assert_eq!(runs[0].run_id, "run-c");
        assert_eq!(runs[1].run_id, "run-a");
    }

    #[test]
    fn resolve_pretty_prefers_explicit_or_tty() {
        assert!(resolve_pretty(true, false));
        assert!(resolve_pretty(false, true));
        assert!(!resolve_pretty(false, false));
    }

    #[test]
    fn resolve_follow_for_respects_interactive_default() {
        assert_eq!(resolve_follow_for(false, None, false), None);
        assert_eq!(
            resolve_follow_for(true, Some(Duration::from_secs(5)), false),
            Some(Duration::from_secs(5))
        );
        assert_eq!(resolve_follow_for(true, None, true), None);
        assert_eq!(
            resolve_follow_for(true, None, false),
            Some(DEFAULT_NONINTERACTIVE_FOLLOW_FOR)
        );
    }

    #[test]
    fn require_existing_socket_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("missing.sock");
        let err = require_existing_socket(&socket_path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("daemon socket missing"));
        assert!(msg.contains("devstack daemon"));
        assert!(msg.contains("devstack install"));
    }
}

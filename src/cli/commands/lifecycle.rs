use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::api::{
    DownRequest, GcRequest, KillRequest, RunWatchResponse, SetNavigationIntentRequest, UpRequest,
    WatchControlRequest,
};
use crate::cli::args::WatchAction;
use crate::cli::commands::logs::normalize_since_arg;
use crate::cli::context::{
    CliContext, DAEMON_LONG_TIMEOUT, DAEMON_TIMEOUT, fetch_runs_with_fallback,
    resolve_project_dir_from_cwd, resolve_run_id, resolve_stack_name, resolve_up_context,
    status_from_manifest,
};
use crate::cli::output::{print_json, print_status_human, print_watch_status_human};
use crate::config::ConfigFile;
use crate::paths;
use crate::persistence::PersistedRun;

const DASHBOARD_PORT: u16 = 47832;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn up(
    context: &CliContext,
    targets: Vec<String>,
    stack_flag: Option<String>,
    new: bool,
    force: bool,
    all: bool,
    project: Option<PathBuf>,
    run_id: Option<String>,
    file: Option<PathBuf>,
    no_wait: bool,
) -> Result<()> {
    let (stack_positional, services) = split_up_targets(targets);
    let stack = stack_flag.or(stack_positional);
    let (resolved_context, stack) = resolve_up_context(stack, project, file)?;
    let project_dir = resolved_context.project_dir.clone();
    let config_path = resolved_context.config_path.clone();
    if all {
        let config_path = config_path
            .ok_or_else(|| anyhow!("no devstack config found; run devstack init or pass --file"))?;
        let config = ConfigFile::load_from_path(&config_path)?;
        let mut runs = Vec::new();
        for stack_name in config.stacks.as_map().keys() {
            let req = UpRequest {
                stack: stack_name.clone(),
                project_dir: project_dir.to_string_lossy().to_string(),
                run_id: None,
                file: Some(config_path.to_string_lossy().to_string()),
                no_wait,
                new_run: new,
                force,
                services: vec![],
            };
            let response = context
                .daemon_request("POST", "/v1/runs/up", Some(req), None)
                .await?;
            runs.push(response);
        }
        print_json(serde_json::Value::Array(runs), context.pretty);
        return Ok(());
    }

    let stack = resolve_stack_name(stack, config_path.as_deref())?;
    let req = UpRequest {
        stack,
        project_dir: project_dir.to_string_lossy().to_string(),
        run_id,
        file: config_path.map(|p| p.to_string_lossy().to_string()),
        no_wait,
        new_run: new,
        force,
        services,
    };
    let response = context
        .daemon_request("POST", "/v1/runs/up", Some(req), None)
        .await?;
    print_json(response, context.pretty);
    Ok(())
}

pub(crate) async fn status(context: &CliContext, run_id: Option<String>, json: bool) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let run_id = resolve_run_id(context, &project_dir, run_id).await?;
    let path = format!("/v1/runs/{run_id}/status");
    let output_json = json || !context.interactive;

    match context
        .daemon_request::<()>("GET", &path, None, Some(DAEMON_TIMEOUT))
        .await
    {
        Ok(response) => {
            let status: crate::api::RunStatusResponse = serde_json::from_value(response)?;
            if output_json {
                print_json(serde_json::to_value(status)?, context.pretty);
            } else {
                print_status_human(&status);
            }
        }
        Err(err) => {
            if let Ok(fallback) = status_from_manifest(&run_id) {
                eprintln!(
                    "warning: daemon unavailable ({}); using cached manifest",
                    err
                );
                if output_json {
                    print_json(serde_json::to_value(fallback)?, context.pretty);
                } else {
                    print_status_human(&fallback);
                }
            } else {
                return Err(err);
            }
        }
    }
    Ok(())
}

pub(crate) async fn watch(context: &CliContext, action: Option<WatchAction>) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let run_id = resolve_run_id(context, &project_dir, None).await?;
    match action {
        None => {
            let status = fetch_watch_status(context, &run_id).await?;
            if context.interactive {
                print_watch_status_human(&status);
            } else {
                print_json(serde_json::to_value(status)?, context.pretty);
            }
        }
        Some(WatchAction::Pause { service }) => {
            let status = update_watch_state(context, &run_id, "pause", service).await?;
            if context.interactive {
                print_watch_status_human(&status);
            } else {
                print_json(serde_json::to_value(status)?, context.pretty);
            }
        }
        Some(WatchAction::Resume { service }) => {
            let status = update_watch_state(context, &run_id, "resume", service).await?;
            if context.interactive {
                print_watch_status_human(&status);
            } else {
                print_json(serde_json::to_value(status)?, context.pretty);
            }
        }
    }
    Ok(())
}

pub(crate) async fn diagnose(
    context: &CliContext,
    run_id: Option<String>,
    service: Option<String>,
) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let run_id = resolve_run_id(context, &project_dir, run_id).await?;

    let path = format!("/v1/runs/{run_id}/status");
    let value = context
        .daemon_request::<()>("GET", &path, None, Some(DAEMON_TIMEOUT))
        .await?;
    let status: crate::api::RunStatusResponse = serde_json::from_value(value)?;

    let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(&run_id))?;
    let manifest = PersistedRun::load_from_path(&manifest_path)?;

    let diag = crate::diagnose::diagnose_run(&run_id, status, manifest, service.as_deref()).await?;
    print_json(serde_json::to_value(diag)?, context.pretty);
    Ok(())
}

pub(crate) async fn list_runs(context: &CliContext, all: bool) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let (mut runs, fallback) = fetch_runs_with_fallback(context).await?;
    if !all {
        runs.runs
            .retain(|run| crate::cli::context::same_project_dir(&run.project_dir, &project_dir));
    }
    if fallback {
        eprintln!("warning: daemon unavailable; showing cached manifests");
    }
    print_json(serde_json::to_value(runs)?, context.pretty);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn show(
    context: &CliContext,
    run_id: Option<String>,
    service: Option<String>,
    q: Option<String>,
    level: Option<String>,
    stream: Option<String>,
    since: Option<String>,
    tail: Option<usize>,
) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let resolved_run_id = if let Some(run_id) = run_id {
        Some(run_id)
    } else {
        crate::cli::context::resolve_latest_run_id(context, &project_dir).await?
    };

    let req = SetNavigationIntentRequest {
        run_id: resolved_run_id,
        service,
        search: q,
        level,
        stream,
        since: normalize_since_arg(since)?,
        last: tail,
    };

    context
        .daemon_request(
            "POST",
            "/v1/navigation/intent",
            Some(req),
            Some(DAEMON_TIMEOUT),
        )
        .await?;

    open_dashboard()?;
    Ok(())
}

pub(crate) async fn agent(
    auto_share: Option<String>,
    no_auto_share: bool,
    watch: Option<Vec<String>>,
    run_id: Option<String>,
    command: Vec<String>,
) -> Result<()> {
    let args = crate::agent::AgentCommandArgs {
        auto_share,
        no_auto_share,
        watch: watch.unwrap_or_default(),
        run_id,
        command,
    };
    let exit_code = crate::agent::run(args).await?;
    std::process::exit(exit_code);
}

pub(crate) async fn down(context: &CliContext, run_id: Option<String>, purge: bool) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let run_id = resolve_run_id(context, &project_dir, run_id).await?;
    let req = DownRequest { run_id, purge };
    let response = context
        .daemon_request(
            "POST",
            "/v1/runs/down",
            Some(req),
            Some(DAEMON_LONG_TIMEOUT),
        )
        .await?;
    print_json(response, context.pretty);
    Ok(())
}

pub(crate) async fn kill(context: &CliContext, run_id: Option<String>) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let run_id = resolve_run_id(context, &project_dir, run_id).await?;
    let req = KillRequest { run_id };
    let response = context
        .daemon_request(
            "POST",
            "/v1/runs/kill",
            Some(req),
            Some(DAEMON_LONG_TIMEOUT),
        )
        .await?;
    print_json(response, context.pretty);
    Ok(())
}

pub(crate) async fn exec(
    context: &CliContext,
    run_id: Option<String>,
    command: Vec<String>,
) -> Result<()> {
    let project_dir = resolve_project_dir_from_cwd()?;
    let run_id = resolve_run_id(context, &project_dir, run_id).await?;
    exec_command(&run_id, &command)
}

pub(crate) async fn gc(context: &CliContext, older_than: Option<String>, all: bool) -> Result<()> {
    let req = GcRequest { older_than, all };
    let response = context
        .daemon_request("POST", "/v1/gc", Some(req), Some(DAEMON_LONG_TIMEOUT))
        .await?;
    print_json(response, context.pretty);
    Ok(())
}

pub(crate) fn ui() -> Result<()> {
    open_dashboard()
}

async fn fetch_watch_status(context: &CliContext, run_id: &str) -> Result<RunWatchResponse> {
    let path = format!("/v1/runs/{run_id}/watch");
    context
        .daemon_request_json::<(), RunWatchResponse>("GET", &path, None, Some(DAEMON_TIMEOUT))
        .await
}

async fn update_watch_state(
    context: &CliContext,
    run_id: &str,
    action: &str,
    service: Option<String>,
) -> Result<RunWatchResponse> {
    let path = format!("/v1/runs/{run_id}/watch/{action}");
    let request = WatchControlRequest { service };
    context
        .daemon_request_json("POST", &path, Some(request), Some(DAEMON_TIMEOUT))
        .await
}

fn open_dashboard() -> Result<()> {
    let url = format!("http://localhost:{}", DASHBOARD_PORT);
    let addr = SocketAddr::from(([127, 0, 0, 1], DASHBOARD_PORT));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
        eprintln!(
            "Note: no dashboard server detected on {}. Ensure the daemon is running (`devstack install` or `devstack daemon`) and the dashboard is installed (dev repo: `./scripts/install-cli.sh`).",
            addr
        );
    }

    println!("Opening dashboard at {}", url);

    if std::env::var("DEVSTACK_DISABLE_DASHBOARD")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
    {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        if let Err(err) = Command::new("open").arg(&url).spawn() {
            eprintln!("Warning: failed to open browser automatically: {err}");
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(err) = Command::new("xdg-open").arg(&url).spawn() {
            eprintln!("Warning: failed to open browser automatically: {err}");
        }
    }

    Ok(())
}

fn split_up_targets(targets: Vec<String>) -> (Option<String>, Vec<String>) {
    if targets.is_empty() {
        return (None, vec![]);
    }
    if targets.len() == 1 {
        return (Some(targets.into_iter().next().unwrap()), vec![]);
    }
    let stack = targets[0].clone();
    let services = targets[1..].to_vec();
    (Some(stack), services)
}

fn exec_command(run_id: &str, command: &[String]) -> Result<()> {
    if command.is_empty() {
        return Err(anyhow!("exec requires a command"));
    }
    let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(run_id))?;
    let manifest = PersistedRun::load_from_path(&manifest_path)?;

    let mut cmd = Command::new(&command[0]);
    if command.len() > 1 {
        cmd.args(&command[1..]);
    }
    cmd.current_dir(&manifest.project_dir);
    cmd.envs(&manifest.env);
    cmd.stdin(std::process::Stdio::inherit());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    let status = cmd.status().context("run exec command")?;
    let code = status.code().unwrap_or(1);
    std::process::exit(code);
}

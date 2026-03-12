use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::api::{
    DownRequest, FacetValueCount, GcRequest, KillRequest, LogEntry, LogFacetsQuery,
    LogFacetsResponse, LogSearchQuery, LogSearchResponse, LogsResponse, PingResponse,
    ProjectsResponse, RegisterProjectResponse, RunListResponse, RunSummary, RunWatchResponse,
    SetNavigationIntentRequest, UpRequest, WatchControlRequest,
};
use crate::config::ConfigFile;
use crate::log_index::{LogIndex, LogSource};
use crate::logs::{
    is_health_noise_line, is_health_noise_message, stream_logs, structured_log_from_entry,
    structured_log_from_raw,
};
use crate::manifest::{RunLifecycle, RunManifest, ServiceState};
use crate::openapi;
use crate::paths;
use crate::shim::ShimArgs;
use crate::sources::{SourcesLedger, source_run_id};
use crate::util::expand_home;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Parser, Debug)]
#[command(
    name = "devstack",
    about = "Local development orchestration for multi-service stacks"
)]
pub struct Cli {
    /// Pretty-print JSON output.
    #[arg(long, global = true, help = "Pretty-print JSON output")]
    pub pretty: bool,
    #[command(subcommand)]
    pub command: Commands,
}

const DAEMON_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_LONG_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_NONINTERACTIVE_FOLLOW_FOR: Duration = Duration::from_secs(15);

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Install and start the devstack daemon service.
    Install,
    /// Initialize a devstack config in the current project.
    Init {
        /// Project directory used to resolve config placement.
        #[arg(long, help = "Project directory used to resolve config placement")]
        project: Option<PathBuf>,
        /// Config file path to create.
        #[arg(long, help = "Config file path to create")]
        file: Option<PathBuf>,
    },
    /// Run the daemon in the foreground.
    Daemon,
    /// Start or refresh a stack.
    Up {
        /// Stack name (positional).
        #[arg(
            value_name = "STACK",
            index = 1,
            conflicts_with = "all",
            help = "Stack name (positional)"
        )]
        stack: Option<String>,
        /// Stack name (flag form).
        #[arg(long = "stack", value_name = "STACK", conflicts_with_all = ["stack", "all"], help = "Stack name (flag form)")]
        stack_flag: Option<String>,
        /// Always create a new run instead of refreshing an existing one.
        #[arg(
            long,
            help = "Always create a new run instead of refreshing an existing one"
        )]
        new: bool,
        /// Skip confirmation prompts.
        #[arg(long, help = "Skip confirmation prompts")]
        force: bool,
        /// Start all stacks in the project config.
        #[arg(long, conflicts_with_all = ["stack", "stack_flag", "run_id"], help = "Start all stacks in the project config")]
        all: bool,
        /// Project directory to resolve config and run context.
        #[arg(long, help = "Project directory to resolve config and run context")]
        project: Option<PathBuf>,
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Explicit config file path.
        #[arg(long, help = "Explicit config file path")]
        file: Option<PathBuf>,
        /// Return immediately without waiting for readiness.
        #[arg(long, help = "Return immediately without waiting for readiness")]
        no_wait: bool,
    },
    /// Show run status and service health.
    Status {
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Output machine-readable JSON.
        #[arg(long, help = "Output machine-readable JSON")]
        json: bool,
    },
    /// Manage auto-restart file watching.
    Watch {
        #[command(subcommand)]
        action: Option<WatchAction>,
    },
    /// Diagnose service startup and runtime issues.
    Diagnose {
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Restrict diagnostics to a specific service.
        #[arg(long, help = "Restrict diagnostics to a specific service")]
        service: Option<String>,
    },
    /// List runs known to the daemon.
    Ls {
        /// List runs from all projects instead of only cwd project.
        #[arg(long, help = "List runs from all projects instead of only cwd project")]
        all: bool,
    },
    /// Query and stream service logs.
    Logs {
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Query a registered external source.
        #[arg(long, conflicts_with_all = ["run_id", "all", "service", "task"], help = "Query a registered external source")]
        source: Option<String>,
        /// Show available facet values for discoverability.
        #[arg(long, conflicts_with_all = ["follow", "tail", "q", "task"], help = "Show available facet values for discoverability")]
        facets: bool,
        /// Search all services in the run (cannot be combined with --follow).
        #[arg(long, conflicts_with_all = ["service", "task", "source"], help = "Search all services in the run (cannot be combined with --follow)")]
        all: bool,
        /// Filter to a specific service.
        #[arg(long, required_unless_present_any = ["all", "task", "source", "facets"], conflicts_with_all = ["all", "task", "source"], help = "Filter to a specific service")]
        service: Option<String>,
        /// Show logs for a named task.
        #[arg(long, conflicts_with_all = ["all", "service", "source"], help = "Show logs for a named task")]
        task: Option<String>,
        /// Show the last N lines (alias: --tail).
        #[arg(long = "last", alias = "tail", help = "Show the last N lines")]
        tail: Option<usize>,
        /// Full-text search query (alias: --q).
        #[arg(long = "search", alias = "q", help = "Full-text search query")]
        q: Option<String>,
        /// Filter by log level.
        #[arg(long, value_parser = ["all", "warn", "error"], conflicts_with = "errors", help = "Filter by log level")]
        level: Option<String>,
        /// Hidden alias for --level error.
        #[arg(
            long,
            conflicts_with = "level",
            hide = true,
            help = "Alias for --level error"
        )]
        errors: bool,
        /// Filter by output stream.
        #[arg(long, value_parser = ["stdout", "stderr"], help = "Filter by output stream")]
        stream: Option<String>,
        /// RFC3339 timestamp or duration (e.g. 5m, 1h).
        #[arg(long, help = "RFC3339 timestamp or duration (e.g. 5m, 1h)")]
        since: Option<String>,
        /// Filter health-check noise (alias: --no-health).
        #[arg(
            long = "no-noise",
            alias = "no-health",
            help = "Filter health-check noise"
        )]
        no_health: bool,
        /// Stream logs in real-time.
        #[arg(long, conflicts_with = "all", help = "Stream logs in real-time")]
        follow: bool,
        /// Stop following after the specified duration.
        #[arg(long, value_name = "DURATION", requires = "follow", value_parser = humantime::parse_duration, help = "Stop following after the specified duration")]
        follow_for: Option<Duration>,
        /// Output machine-readable JSON.
        #[arg(long, help = "Output machine-readable JSON")]
        json: bool,
    },
    /// Open the dashboard at a filtered log view.
    Show {
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Filter to a specific service.
        #[arg(long, help = "Filter to a specific service")]
        service: Option<String>,
        /// Full-text search query (alias: --q).
        #[arg(long = "search", alias = "q", help = "Full-text search query")]
        q: Option<String>,
        /// Filter by log level.
        #[arg(long, value_parser = ["all", "warn", "error"], help = "Filter by log level")]
        level: Option<String>,
        /// Filter by output stream.
        #[arg(long, value_parser = ["stdout", "stderr"], help = "Filter by output stream")]
        stream: Option<String>,
        /// RFC3339 timestamp or duration (e.g. 5m, 1h).
        #[arg(long, help = "RFC3339 timestamp or duration (e.g. 5m, 1h)")]
        since: Option<String>,
        /// Show the last N lines (alias: --tail).
        #[arg(long = "last", alias = "tail", help = "Show the last N lines")]
        tail: Option<usize>,
    },
    /// Wrap an agent CLI with devstack integration.
    Agent {
        /// Auto-share logs at this level or above.
        #[arg(long, value_parser = ["error", "warn"], help = "Auto-share logs at this level or above")]
        auto_share: Option<String>,
        /// Disable auto-sharing entirely.
        #[arg(
            long,
            conflicts_with = "auto_share",
            help = "Disable auto-sharing entirely"
        )]
        no_auto_share: bool,
        /// Restrict auto-sharing to specific services.
        #[arg(
            long,
            value_delimiter = ',',
            value_name = "SERVICES",
            help = "Restrict auto-sharing to specific services"
        )]
        watch: Option<Vec<String>>,
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Agent command and args (must follow --).
        #[arg(
            last = true,
            required = true,
            help = "Agent command and args (must follow --)"
        )]
        command: Vec<String>,
    },
    /// Stop the active run.
    Down {
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Remove run artifacts from disk after stopping.
        #[arg(long, help = "Remove run artifacts from disk after stopping")]
        purge: bool,
    },
    /// Force-kill the active run.
    Kill {
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
    },
    /// Run an arbitrary command in the run context.
    Exec {
        /// Target run id (alias: --run-id).
        #[arg(long = "run", alias = "run-id", help = "Target run id")]
        run_id: Option<String>,
        /// Command to execute.
        #[arg(last = true, required = true, help = "Command to execute")]
        command: Vec<String>,
    },
    /// Validate devstack config files.
    Lint {
        /// Project directory to resolve config from.
        #[arg(long, help = "Project directory to resolve config from")]
        project: Option<PathBuf>,
        /// Explicit config file path to validate.
        #[arg(long, help = "Explicit config file path to validate")]
        file: Option<PathBuf>,
    },
    /// Check daemon health and local prerequisites.
    Doctor,
    /// Generate shell completion scripts.
    Completions {
        /// Target shell (bash, zsh, fish, etc.).
        #[arg(value_name = "SHELL", help = "Target shell (bash, zsh, fish, etc.)")]
        shell: String,
    },
    /// Garbage collect old runs and globals.
    Gc {
        /// Delete entries older than this duration (e.g. 7d).
        #[arg(long, help = "Delete entries older than this duration (e.g. 7d)")]
        older_than: Option<String>,
        /// Delete all stopped runs/globals regardless of age.
        #[arg(long, help = "Delete all stopped runs/globals regardless of age")]
        all: bool,
    },
    /// Open the devstack dashboard in browser.
    Ui,
    /// Manage registered projects.
    Projects {
        #[command(subcommand)]
        action: Option<ProjectsAction>,
    },
    /// Manage external log sources.
    Sources {
        #[command(subcommand)]
        action: Option<SourcesAction>,
    },
    /// Run a named task from [tasks].
    Run {
        /// Task name to run (omit to list available tasks).
        #[arg(
            value_name = "TASK",
            help = "Task name to run (omit to list available tasks)"
        )]
        name: Option<String>,
        /// Run all init tasks for the current stack without starting services.
        #[arg(
            long,
            help = "Run all init tasks for the current stack without starting services"
        )]
        init: bool,
        /// Stack to use when running --init.
        #[arg(long, requires = "init", help = "Stack to use when running --init")]
        stack: Option<String>,
        /// Project directory to resolve config and task context.
        #[arg(long, help = "Project directory to resolve config and task context")]
        project: Option<PathBuf>,
        /// Explicit config file path.
        #[arg(long, help = "Explicit config file path")]
        file: Option<PathBuf>,
        /// Stream task stdout/stderr directly to the terminal.
        #[arg(long, help = "Stream task stdout/stderr directly to the terminal")]
        verbose: bool,
        /// Output machine-readable JSON.
        #[arg(long, help = "Output machine-readable JSON")]
        json: bool,
    },
    /// Print the OpenAPI spec.
    Openapi {
        /// Path to write OpenAPI output; stdout when omitted.
        #[arg(long, help = "Path to write OpenAPI output; stdout when omitted")]
        out: Option<PathBuf>,
        /// Regenerate output whenever source files change.
        #[arg(long, help = "Regenerate output whenever source files change")]
        watch: bool,
    },
    #[command(name = "__complete", hide = true)]
    Complete {
        #[arg(long)]
        cword: usize,
        #[arg(last = true)]
        words: Vec<String>,
    },
    #[command(name = "__shim", hide = true)]
    Shim {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        service: String,
        #[arg(long)]
        cmd: String,
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long)]
        log_file: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectsAction {
    /// List registered projects.
    Ls,
    /// Register a new project.
    Add {
        /// Path to the project directory.
        #[arg(default_value = ".", help = "Path to the project directory")]
        path: PathBuf,
    },
    /// Remove a project from the ledger.
    Remove {
        /// Project id or project path.
        #[arg(help = "Project id or project path")]
        project: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SourcesAction {
    /// List registered log sources.
    Ls,
    /// Register a source name with one or more file paths/globs.
    Add {
        /// Source name.
        #[arg(help = "Source name")]
        name: String,
        /// Source file paths or glob patterns.
        #[arg(required = true, help = "Source file paths or glob patterns")]
        paths: Vec<String>,
    },
    /// Remove a registered source.
    Rm {
        /// Source name.
        #[arg(help = "Source name")]
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WatchAction {
    /// Pause automatic file-watch restarts.
    Pause {
        /// Restrict pause to a specific service.
        #[arg(long, help = "Restrict pause to a specific service")]
        service: Option<String>,
    },
    /// Resume automatic file-watch restarts.
    Resume {
        /// Restrict resume to a specific service.
        #[arg(long, help = "Restrict resume to a specific service")]
        service: Option<String>,
    },
}

struct ProjectContext {
    project_dir: PathBuf,
    config_path: Option<PathBuf>,
}

fn is_interactive() -> bool {
    std::io::stdout().is_terminal()
}

fn resolve_pretty(explicit: bool, interactive: bool) -> bool {
    explicit || interactive
}

fn resolve_follow_for(
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

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let interactive = is_interactive();
    let pretty = resolve_pretty(cli.pretty, interactive);
    match cli.command {
        Commands::Install => install().await,
        Commands::Init { project, file } => init(project, file).await,
        Commands::Daemon => crate::daemon::run_daemon().await,
        Commands::Up {
            stack,
            stack_flag,
            new,
            force,
            all,
            project,
            run_id,
            file,
            no_wait,
        } => {
            let stack = stack_flag.or(stack);
            let (context, stack) = resolve_up_context(stack, project, file)?;
            let project_dir = context.project_dir.clone();
            let config_path = context.config_path.clone();
            if all {
                let config_path = config_path.ok_or_else(|| {
                    anyhow!("no devstack config found; run devstack init or pass --file")
                })?;
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
                    };
                    let response = call_daemon("POST", "/v1/runs/up", Some(req), None).await?;
                    runs.push(response);
                }
                print_json(serde_json::Value::Array(runs), pretty);
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
            };
            let response = call_daemon("POST", "/v1/runs/up", Some(req), None).await?;
            print_json(response, pretty);
            Ok(())
        }
        Commands::Status { run_id, json } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let run_id = resolve_run_id(&project_dir, run_id).await?;
            let path = format!("/v1/runs/{run_id}/status");
            let output_json = json || !interactive;

            match call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_TIMEOUT)).await {
                Ok(response) => {
                    let status: crate::api::RunStatusResponse = serde_json::from_value(response)?;
                    if output_json {
                        print_json(serde_json::to_value(status)?, pretty);
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
                            print_json(serde_json::to_value(fallback)?, pretty);
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
        Commands::Watch { action } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let run_id = resolve_run_id(&project_dir, None).await?;
            match action {
                None => {
                    let status = fetch_watch_status(&run_id).await?;
                    if interactive {
                        print_watch_status_human(&status);
                    } else {
                        print_json(serde_json::to_value(status)?, pretty);
                    }
                }
                Some(WatchAction::Pause { service }) => {
                    let status = update_watch_state(&run_id, "pause", service).await?;
                    if interactive {
                        print_watch_status_human(&status);
                    } else {
                        print_json(serde_json::to_value(status)?, pretty);
                    }
                }
                Some(WatchAction::Resume { service }) => {
                    let status = update_watch_state(&run_id, "resume", service).await?;
                    if interactive {
                        print_watch_status_human(&status);
                    } else {
                        print_json(serde_json::to_value(status)?, pretty);
                    }
                }
            }
            Ok(())
        }
        Commands::Diagnose { run_id, service } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let run_id = resolve_run_id(&project_dir, run_id).await?;

            let path = format!("/v1/runs/{run_id}/status");
            let value =
                call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_TIMEOUT)).await?;
            let status: crate::api::RunStatusResponse = serde_json::from_value(value)?;

            let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(&run_id))?;
            let manifest = RunManifest::load_from_path(&manifest_path)?;

            let diag = crate::diagnose::diagnose_run(&run_id, status, manifest, service.as_deref())
                .await?;
            print_json(serde_json::to_value(diag)?, pretty);
            Ok(())
        }
        Commands::Ls { all } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let (mut runs, fallback) = fetch_runs_with_fallback().await?;
            if !all {
                runs.runs
                    .retain(|run| same_project_dir(&run.project_dir, &project_dir));
            }
            if fallback {
                eprintln!("warning: daemon unavailable; showing cached manifests");
            }
            print_json(serde_json::to_value(runs)?, pretty);
            Ok(())
        }
        Commands::Logs {
            run_id,
            source,
            facets,
            all,
            service,
            task,
            tail,
            q,
            level,
            errors,
            stream,
            since,
            no_health,
            follow,
            follow_for,
            json,
        } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let follow_for = resolve_follow_for(follow, follow_for, interactive);
            let since = normalize_since_arg(since)?;
            let level = if errors {
                Some("error".to_string())
            } else {
                level
            };

            if let Some(source_name) = source {
                if facets {
                    let response = query_source_log_facets(
                        &source_name,
                        level.as_deref(),
                        stream.as_deref(),
                        since.as_deref(),
                    )
                    .await?;
                    emit_log_facets(&format!("Source: {source_name}"), &response, json)?;
                    return Ok(());
                }

                if follow {
                    return Err(anyhow!("--follow is not supported with --source"));
                }
                let response = query_source_logs(
                    &source_name,
                    tail.unwrap_or(500),
                    q.as_deref(),
                    level.as_deref(),
                    stream.as_deref(),
                    since.as_deref(),
                )
                .await?;
                for entry in &response.entries {
                    emit_entry(entry, json, no_health)?;
                }
                return Ok(());
            }

            if facets {
                let run_id = resolve_run_id(&project_dir, run_id).await?;
                let response = fetch_run_log_facets(
                    &run_id,
                    service.as_deref(),
                    level.as_deref(),
                    stream.as_deref(),
                    since.as_deref(),
                )
                .await?;
                emit_log_facets(&format!("Run: {run_id}"), &response, json)?;
                return Ok(());
            }

            if let Some(task_name) = task {
                if all
                    || service.is_some()
                    || q.is_some()
                    || level.is_some()
                    || stream.is_some()
                    || since.is_some()
                {
                    return Err(anyhow!(
                        "--task cannot be combined with --all, --service, --search, --level, --stream, or --since"
                    ));
                }

                let candidates =
                    task_log_path_candidates(&project_dir, &task_name, run_id.as_deref()).await?;
                let log_path = candidates.iter().find(|path| path.exists()).cloned();

                let Some(log_path) = log_path else {
                    let looked_at = candidates
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(anyhow!(
                        "task log not found for '{task_name}' (looked at {looked_at})"
                    ));
                };

                return stream_logs(
                    &log_path, &task_name, tail, follow, follow_for, json, no_health,
                )
                .await;
            }

            let run_id = resolve_run_id(&project_dir, run_id).await?;

            if all {
                if follow {
                    return Err(anyhow!(
                        "--follow requires --service (cannot be used with --all)"
                    ));
                }
                let tail = tail.unwrap_or(500);
                let response = fetch_run_log_search(
                    &run_id,
                    tail,
                    q.as_deref(),
                    level.as_deref(),
                    stream.as_deref(),
                    since.as_deref(),
                )
                .await?;
                for entry in &response.entries {
                    emit_entry(entry, json, no_health)?;
                }
                return Ok(());
            }

            let Some(service) = service else {
                return Err(anyhow!(
                    "--service is required unless --all or --task is set"
                ));
            };

            let api_only = q.is_some() || level.is_some() || stream.is_some() || since.is_some();

            let api_result = if follow {
                stream_service_logs_api(
                    &run_id,
                    &service,
                    tail.unwrap_or(200),
                    q.as_deref(),
                    level.as_deref(),
                    stream.as_deref(),
                    since.as_deref(),
                    follow_for,
                    json,
                    no_health,
                )
                .await
            } else {
                let tail = tail.unwrap_or(500);
                let response = fetch_service_logs_api(
                    &run_id,
                    &service,
                    tail,
                    None,
                    q.as_deref(),
                    level.as_deref(),
                    stream.as_deref(),
                    since.as_deref(),
                )
                .await?;
                emit_lines(&response.lines, &service, json, no_health)?;
                Ok(())
            };

            match api_result {
                Ok(()) => Ok(()),
                Err(err) if !api_only => {
                    // Fallback to direct file read when daemon is unavailable and no search filters are requested.
                    let log_path = paths::run_log_path(
                        &crate::ids::RunId::new(run_id),
                        &crate::ids::ServiceName::new(&service),
                    )?;
                    stream_logs(
                        &log_path, &service, tail, follow, follow_for, json, no_health,
                    )
                    .await
                }
                Err(err) => Err(err),
            }
        }
        Commands::Show {
            run_id,
            service,
            q,
            level,
            stream,
            since,
            tail,
        } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let resolved_run_id = if let Some(run_id) = run_id {
                Some(run_id)
            } else {
                resolve_latest_run_id(&project_dir).await?
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

            call_daemon(
                "POST",
                "/v1/navigation/intent",
                Some(req),
                Some(DAEMON_TIMEOUT),
            )
            .await?;

            open_dashboard()?;
            Ok(())
        }
        Commands::Agent {
            auto_share,
            no_auto_share,
            watch,
            run_id,
            command,
        } => {
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
        Commands::Down { run_id, purge } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let run_id = resolve_run_id(&project_dir, run_id).await?;
            let req = DownRequest { run_id, purge };
            let response = call_daemon(
                "POST",
                "/v1/runs/down",
                Some(req),
                Some(DAEMON_LONG_TIMEOUT),
            )
            .await?;
            print_json(response, pretty);
            Ok(())
        }
        Commands::Kill { run_id } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let run_id = resolve_run_id(&project_dir, run_id).await?;
            let req = KillRequest { run_id };
            let response = call_daemon(
                "POST",
                "/v1/runs/kill",
                Some(req),
                Some(DAEMON_LONG_TIMEOUT),
            )
            .await?;
            print_json(response, pretty);
            Ok(())
        }
        Commands::Exec { run_id, command } => {
            let project_dir = resolve_project_dir_from_cwd()?;
            let run_id = resolve_run_id(&project_dir, run_id).await?;
            exec_command(&run_id, &command)
        }
        Commands::Lint { project, file } => lint(project, file, pretty),
        Commands::Doctor => doctor(pretty).await,
        Commands::Completions { shell } => {
            print_completions(&shell)?;
            Ok(())
        }
        Commands::Gc { older_than, all } => {
            let req = GcRequest { older_than, all };
            let response =
                call_daemon("POST", "/v1/gc", Some(req), Some(DAEMON_LONG_TIMEOUT)).await?;
            print_json(response, pretty);
            Ok(())
        }
        Commands::Ui => {
            open_dashboard()?;
            Ok(())
        }
        Commands::Projects { action } => {
            handle_projects(action, pretty).await?;
            Ok(())
        }
        Commands::Sources { action } => {
            handle_sources(action, pretty).await?;
            Ok(())
        }
        Commands::Run {
            name,
            init,
            stack,
            project,
            file,
            verbose,
            json,
        } => run_task_command_cli(name, init, stack, project, file, verbose, json, pretty).await,
        Commands::Openapi { out, watch } => {
            if watch {
                watch_openapi(out)?;
            } else {
                write_openapi(out)?;
            }
            Ok(())
        }
        Commands::Complete { cword, words } => {
            complete(cword, words).await?;
            Ok(())
        }
        Commands::Shim {
            run_id,
            service,
            cmd,
            cwd,
            log_file,
        } => {
            let args = ShimArgs {
                run_id,
                service,
                cmd,
                cwd,
                log_file,
            };
            crate::shim::run(args).await
        }
    }
}

#[derive(Debug)]
struct DaemonTimeout;

impl std::fmt::Display for DaemonTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "daemon request timed out")
    }
}

impl std::error::Error for DaemonTimeout {}

async fn call_daemon<T: serde::Serialize>(
    method: &str,
    path: &str,
    body: Option<T>,
    timeout_duration: Option<Duration>,
) -> Result<serde_json::Value> {
    let fut = call_daemon_inner(method, path, body);
    if let Some(timeout_duration) = timeout_duration {
        match timeout(timeout_duration, fut).await {
            Ok(result) => result,
            Err(_) => Err(DaemonTimeout.into()),
        }
    } else {
        fut.await
    }
}

async fn call_daemon_inner<T: serde::Serialize>(
    method: &str,
    path: &str,
    body: Option<T>,
) -> Result<serde_json::Value> {
    let socket_path = paths::daemon_socket_path()?;
    require_existing_socket(&socket_path)?;
    let stream = UnixStream::connect(&socket_path)
        .await
        .with_context(|| format!(
            "connect to daemon socket at {} (is the daemon running? try `devstack daemon` or `devstack install`)",
            socket_path.display()
        ))?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("handshake with daemon")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body_bytes = if let Some(value) = body {
        serde_json::to_vec(&value)?
    } else {
        Vec::new()
    };

    let req = Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("content-type", "application/json")
        .body(Full::new(hyper::body::Bytes::from(body_bytes)))?;

    let response = sender.send_request(req).await.context("send request")?;
    let status = response.status();
    let body_bytes = response.into_body().collect().await?.to_bytes();
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body_bytes);
        return Err(anyhow!("daemon error: {status} {text}"));
    }
    if body_bytes.is_empty() {
        return Ok(serde_json::json!({}));
    }
    let value: serde_json::Value = serde_json::from_slice(&body_bytes)?;
    Ok(value)
}

fn normalize_since_arg(since: Option<String>) -> Result<Option<String>> {
    let Some(since) = since else {
        return Ok(None);
    };
    let since = since.trim().to_string();
    if since.is_empty() {
        return Ok(None);
    }
    if OffsetDateTime::parse(&since, &Rfc3339).is_ok() {
        return Ok(Some(since));
    }
    if let Ok(dur) = humantime::parse_duration(&since) {
        let dt = OffsetDateTime::now_utc() - dur;
        return Ok(Some(dt.format(&Rfc3339)?));
    }
    Err(anyhow!(
        "invalid --since value {since:?}; use RFC3339 (e.g. 2025-01-01T00:00:00Z) or a duration (e.g. 5m, 1h)"
    ))
}

fn build_query_string(params: Vec<(&str, String)>) -> String {
    let mut out = String::new();
    for (k, v) in params {
        if v.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push('?');
        } else {
            out.push('&');
        }
        out.push_str(k);
        out.push('=');
        out.push_str(&urlencoding::encode(&v));
    }
    out
}

#[allow(clippy::too_many_arguments)]
async fn fetch_service_logs_api(
    run_id: &str,
    service: &str,
    tail: usize,
    after: Option<u64>,
    q: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
) -> Result<LogsResponse> {
    let mut params = Vec::new();
    params.push(("last", tail.to_string()));
    if let Some(after) = after {
        params.push(("after", after.to_string()));
    }
    if let Some(q) = q {
        params.push(("search", q.to_string()));
    }
    if let Some(level) = level
        && level != "all"
    {
        params.push(("level", level.to_string()));
    }
    if let Some(stream) = stream {
        params.push(("stream", stream.to_string()));
    }
    if let Some(since) = since {
        params.push(("since", since.to_string()));
    }
    let query = build_query_string(params);
    let path = format!("/v1/runs/{run_id}/logs/{service}{query}");
    let value =
        call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_LONG_TIMEOUT)).await?;
    Ok(serde_json::from_value(value)?)
}

async fn fetch_run_log_search(
    run_id: &str,
    tail: usize,
    q: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
) -> Result<LogSearchResponse> {
    let mut params = Vec::new();
    params.push(("last", tail.to_string()));
    if let Some(q) = q {
        params.push(("search", q.to_string()));
    }
    if let Some(level) = level
        && level != "all"
    {
        params.push(("level", level.to_string()));
    }
    if let Some(stream) = stream {
        params.push(("stream", stream.to_string()));
    }
    if let Some(since) = since {
        params.push(("since", since.to_string()));
    }
    let query = build_query_string(params);
    let path = format!("/v1/runs/{run_id}/logs{query}");
    let value =
        call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_LONG_TIMEOUT)).await?;
    Ok(serde_json::from_value(value)?)
}

async fn fetch_run_log_facets(
    run_id: &str,
    service: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
) -> Result<LogFacetsResponse> {
    let mut params = Vec::new();
    if let Some(since) = since {
        params.push(("since", since.to_string()));
    }
    if let Some(service) = service {
        params.push(("service", service.to_string()));
    }
    if let Some(level) = level
        && level != "all"
    {
        params.push(("level", level.to_string()));
    }
    if let Some(stream) = stream {
        params.push(("stream", stream.to_string()));
    }

    let query = build_query_string(params);
    let path = format!("/v1/runs/{run_id}/logs/facets{query}");
    let value =
        call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_LONG_TIMEOUT)).await?;
    Ok(serde_json::from_value(value)?)
}

async fn fetch_watch_status(run_id: &str) -> Result<RunWatchResponse> {
    let path = format!("/v1/runs/{run_id}/watch");
    let value = call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_TIMEOUT)).await?;
    Ok(serde_json::from_value(value)?)
}

async fn update_watch_state(
    run_id: &str,
    action: &str,
    service: Option<String>,
) -> Result<RunWatchResponse> {
    let path = format!("/v1/runs/{run_id}/watch/{action}");
    let request = WatchControlRequest { service };
    let value = call_daemon("POST", &path, Some(request), Some(DAEMON_TIMEOUT)).await?;
    Ok(serde_json::from_value(value)?)
}

fn source_log_sources(ledger: &SourcesLedger, source_name: &str) -> Result<Vec<LogSource>> {
    let run_id = source_run_id(source_name);
    let resolved = ledger.resolve_log_sources(source_name)?;
    Ok(resolved
        .into_iter()
        .map(|item| LogSource {
            run_id: run_id.clone(),
            service: item.service,
            path: item.path,
        })
        .collect())
}

fn search_source_logs(
    index: &LogIndex,
    ledger: &SourcesLedger,
    source_name: &str,
    query: LogSearchQuery,
) -> Result<LogSearchResponse> {
    let sources = source_log_sources(ledger, source_name)?;
    if sources.is_empty() {
        return Ok(LogSearchResponse {
            entries: Vec::new(),
            truncated: false,
            total: 0,
            error_count: 0,
            warn_count: 0,
            matched_total: 0,
        });
    }

    index.ingest_sources(&sources)?;
    let run_id = source_run_id(source_name);
    index.search_run(&run_id, &sources, query)
}

fn search_source_log_facets(
    index: &LogIndex,
    ledger: &SourcesLedger,
    source_name: &str,
    query: LogFacetsQuery,
) -> Result<LogFacetsResponse> {
    let sources = source_log_sources(ledger, source_name)?;
    if !sources.is_empty() {
        index.ingest_sources(&sources)?;
    }
    let run_id = source_run_id(source_name);
    index.facets_run(&run_id, &sources, query)
}

async fn query_source_logs(
    source_name: &str,
    tail: usize,
    q: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
) -> Result<LogSearchResponse> {
    if daemon_is_running().await {
        let mut params = Vec::new();
        params.push(("last", tail.to_string()));
        if let Some(q) = q {
            params.push(("search", q.to_string()));
        }
        if let Some(level) = level
            && level != "all"
        {
            params.push(("level", level.to_string()));
        }
        if let Some(stream) = stream {
            params.push(("stream", stream.to_string()));
        }
        if let Some(since) = since {
            params.push(("since", since.to_string()));
        }
        let query_str = build_query_string(params);
        let path = format!("/v1/sources/{source_name}/logs{query_str}");
        let value =
            call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_LONG_TIMEOUT)).await?;
        let logs_response: crate::api::LogsResponse = serde_json::from_value(value)?;
        // Convert raw lines to LogSearchResponse entries
        let entries: Vec<crate::api::LogEntry> = logs_response
            .lines
            .iter()
            .map(|line| {
                let s = structured_log_from_raw(source_name, line);
                crate::api::LogEntry {
                    ts: s.timestamp.unwrap_or_default(),
                    service: s.service,
                    stream: s.stream,
                    level: s.level.unwrap_or_else(|| "info".to_string()),
                    message: s.message,
                    raw: line.clone(),
                    attributes: Default::default(),
                }
            })
            .collect();
        return Ok(LogSearchResponse {
            total: logs_response.total,
            truncated: logs_response.truncated,
            error_count: logs_response.error_count,
            warn_count: logs_response.warn_count,
            matched_total: logs_response.matched_total,
            entries,
        });
    }

    let source_name = source_name.to_string();
    let query = LogSearchQuery {
        last: Some(tail),
        since: since.map(|value| value.to_string()),
        search: q.map(|value| value.to_string()),
        level: level.map(|value| value.to_string()),
        stream: stream.map(|value| value.to_string()),
        service: None,
    };

    let response = tokio::task::spawn_blocking(move || {
        let ledger = SourcesLedger::load()?;
        let index = LogIndex::open_or_create()?;
        search_source_logs(&index, &ledger, &source_name, query)
    })
    .await
    .map_err(|e| anyhow!("source log search task failed: {e}"))??;

    Ok(response)
}

async fn query_source_log_facets(
    source_name: &str,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
) -> Result<LogFacetsResponse> {
    if daemon_is_running().await {
        let mut params = Vec::new();
        if let Some(since) = since {
            params.push(("since", since.to_string()));
        }
        if let Some(level) = level
            && level != "all"
        {
            params.push(("level", level.to_string()));
        }
        if let Some(stream) = stream {
            params.push(("stream", stream.to_string()));
        }

        let query_str = build_query_string(params);
        let path = format!("/v1/sources/{source_name}/facets{query_str}");
        let value =
            call_daemon::<serde_json::Value>("GET", &path, None, Some(DAEMON_LONG_TIMEOUT)).await?;
        return Ok(serde_json::from_value(value)?);
    }

    let source_name = source_name.to_string();
    let query = LogFacetsQuery {
        since: since.map(|value| value.to_string()),
        service: None,
        level: level.map(|value| value.to_string()),
        stream: stream.map(|value| value.to_string()),
    };

    let response = tokio::task::spawn_blocking(move || {
        let ledger = SourcesLedger::load()?;
        let index = LogIndex::open_or_create()?;
        search_source_log_facets(&index, &ledger, &source_name, query)
    })
    .await
    .map_err(|e| anyhow!("source log facets task failed: {e}"))??;

    Ok(response)
}

async fn refresh_source_index(source_name: &str) -> Result<()> {
    let source_name = source_name.to_string();
    tokio::task::spawn_blocking(move || {
        let index = LogIndex::open_or_create()?;
        let run_id = source_run_id(&source_name);
        index.delete_run(&run_id)?;

        let ledger = SourcesLedger::load()?;
        if ledger.get(&source_name).is_some() {
            let sources = source_log_sources(&ledger, &source_name)?;
            if !sources.is_empty() {
                index.ingest_sources(&sources)?;
            }
        }

        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow!("source index refresh task failed: {e}"))??;
    Ok(())
}

fn absolutize_source_patterns(paths: Vec<String>) -> Result<Vec<String>> {
    let cwd = std::env::current_dir()?;
    Ok(paths
        .into_iter()
        .map(|pattern| {
            let expanded = expand_home(Path::new(&pattern));
            if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            }
            .to_string_lossy()
            .to_string()
        })
        .collect())
}

fn emit_log_facets(label: &str, response: &LogFacetsResponse, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(response)?);
    } else {
        print!("{}", format_log_facets(label, response));
    }
    Ok(())
}

fn format_log_facets(label: &str, response: &LogFacetsResponse) -> String {
    let mut out = String::new();
    out.push_str(label);
    out.push_str("\n\n");

    if response.filters.is_empty() {
        out.push_str("filters (0):\n");
        return out;
    }

    for (index, filter) in response.filters.iter().enumerate() {
        let section_name = format!("{} [{}]", filter.field, filter.kind);
        out.push_str(&format_facet_section(&section_name, &filter.values));
        if index + 1 < response.filters.len() {
            out.push('\n');
        }
    }
    out
}

fn format_facet_section(name: &str, values: &[FacetValueCount]) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} ({}):\n", name, values.len()));

    if values.is_empty() {
        return out;
    }

    let max_value_width = values
        .iter()
        .map(|item| item.value.len())
        .max()
        .unwrap_or(0);
    let formatted_counts: Vec<String> = values
        .iter()
        .map(|item| format_count_with_commas(item.count))
        .collect();
    let max_count_width = formatted_counts
        .iter()
        .map(|count| count.len())
        .max()
        .unwrap_or(0);

    for (item, formatted_count) in values.iter().zip(formatted_counts.iter()) {
        out.push_str(&format!(
            "  {:value_width$}  {:>count_width$}\n",
            item.value,
            formatted_count,
            value_width = max_value_width,
            count_width = max_count_width,
        ));
    }

    out
}

fn format_count_with_commas(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, ch) in digits.chars().enumerate() {
        out.push(ch);
        let remaining = digits.len() - index - 1;
        if remaining > 0 && remaining.is_multiple_of(3) {
            out.push(',');
        }
    }

    out
}

fn emit_entry(entry: &LogEntry, json: bool, no_health: bool) -> Result<()> {
    if no_health && is_health_noise_message(&entry.message) {
        return Ok(());
    }

    if json {
        let payload = structured_log_from_entry(entry);
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        println!("[{}] {}", entry.service, entry.raw);
    }

    Ok(())
}

fn emit_line(line: &str, service: &str, json: bool, no_health: bool) -> Result<()> {
    if no_health && is_health_noise_line(line) {
        return Ok(());
    }

    if json {
        let payload = structured_log_from_raw(service, line);
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        println!("{line}");
    }
    Ok(())
}

fn emit_lines(lines: &[String], service: &str, json: bool, no_health: bool) -> Result<()> {
    for line in lines {
        emit_line(line, service, json, no_health)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn stream_service_logs_api(
    run_id: &str,
    service: &str,
    initial_tail: usize,
    q: Option<&str>,
    level: Option<&str>,
    stream: Option<&str>,
    since: Option<&str>,
    follow_for: Option<Duration>,
    json: bool,
    no_health: bool,
) -> Result<()> {
    let start = Instant::now();

    let response =
        fetch_service_logs_api(run_id, service, initial_tail, None, q, level, stream, since)
            .await?;
    for line in &response.lines {
        emit_line(line, service, json, no_health)?;
    }
    let mut after = response.next_after;

    loop {
        if let Some(limit) = follow_for
            && start.elapsed() >= limit
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let response =
            fetch_service_logs_api(run_id, service, 500, after, q, level, stream, since).await?;
        for line in &response.lines {
            emit_line(line, service, json, no_health)?;
        }
        if let Some(next) = response.next_after {
            after = Some(after.map(|a| a.max(next)).unwrap_or(next));
        }
    }
}

fn print_json(value: serde_json::Value, pretty: bool) {
    if pretty {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
    } else {
        println!("{}", serde_json::to_string(&value).unwrap_or_default());
    }
}

fn print_watch_status_human(status: &RunWatchResponse) {
    println!("watch: {}", status.run_id);
    println!("  service    auto_restart  status");

    for (name, svc) in &status.services {
        let state = if !svc.auto_restart {
            "disabled"
        } else if svc.paused {
            "paused"
        } else if svc.active {
            "watching"
        } else {
            "inactive"
        };
        println!("  {:<9}  {:<12}  {}", name, svc.auto_restart, state);
    }
}

fn print_status_human(status: &crate::api::RunStatusResponse) {
    let total_services = status.services.len();
    let healthy_services = status
        .services
        .values()
        .filter(|svc| is_service_healthy(svc))
        .count();

    // Header line: stack: dev (running, 3/3 healthy)
    println!(
        "stack: {} ({}, {}/{} healthy)",
        status.stack,
        run_lifecycle_label(&status.state),
        healthy_services,
        total_services
    );

    let is_tty = is_interactive();

    // Find the longest service name for alignment
    let max_name_len = status
        .services
        .keys()
        .map(|name| name.len())
        .max()
        .unwrap_or(0);

    for (name, svc) in &status.services {
        let state = service_state_label(&svc.state);
        let colored_state = if is_tty {
            match svc.state {
                ServiceState::Ready => format!("\x1b[32m{}\x1b[0m", state), // green
                ServiceState::Degraded => format!("\x1b[33m{}\x1b[0m", state), // yellow
                ServiceState::Failed => format!("\x1b[31m{}\x1b[0m", state), // red
                _ => state.to_string(), // no color for starting/stopped
            }
        } else {
            state.to_string()
        };

        let url = svc.url.as_deref().unwrap_or("");
        let uptime = svc
            .uptime_seconds
            .map(|s| format!("(up {})", format_compact_duration(s)))
            .unwrap_or_else(|| "(up unknown)".to_string());

        let watch_suffix = if svc.auto_restart {
            if svc.watch_paused {
                "  [paused]"
            } else {
                "  [watching]"
            }
        } else {
            ""
        };

        // Format: "  service-name  ready  http://localhost:40665  (up 2h)"
        println!(
            "  {:width$}  {}  {}  {}{}",
            name,
            colored_state,
            url,
            uptime,
            watch_suffix,
            width = max_name_len
        );

        // Show last error if present
        if let Some(last_error) = svc.recent_errors.last() {
            let relative = last_error
                .timestamp
                .as_deref()
                .map(format_relative_timestamp)
                .unwrap_or_else(|| "unknown".to_string());
            println!("    last error ({}): {}", relative, last_error.message);
        }
    }
}

fn is_service_healthy(svc: &crate::api::ServiceStatus) -> bool {
    if svc.state != ServiceState::Ready {
        return false;
    }
    svc.health_check_stats
        .as_ref()
        .and_then(|stats| stats.last_ok)
        .unwrap_or(true)
}

fn run_lifecycle_label(state: &RunLifecycle) -> &'static str {
    match state {
        RunLifecycle::Starting => "starting",
        RunLifecycle::Running => "running",
        RunLifecycle::Degraded => "degraded",
        RunLifecycle::Stopped => "stopped",
    }
}

fn service_state_label(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Starting => "starting",
        ServiceState::Ready => "ready",
        ServiceState::Degraded => "degraded",
        ServiceState::Stopped => "stopped",
        ServiceState::Failed => "failed",
    }
}

fn format_relative_timestamp(timestamp: &str) -> String {
    let Ok(ts) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
        return "unknown".to_string();
    };
    let elapsed = (OffsetDateTime::now_utc() - ts).whole_seconds();
    if elapsed <= 0 {
        return "just now".to_string();
    }
    format!("{} ago", format_compact_duration(elapsed as u64))
}

fn format_compact_duration(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    if seconds >= DAY {
        format!("{}d", seconds / DAY)
    } else if seconds >= HOUR {
        format!("{}h", seconds / HOUR)
    } else if seconds >= MINUTE {
        format!("{}m", seconds / MINUTE)
    } else {
        format!("{}s", seconds)
    }
}

fn write_openapi(out: Option<PathBuf>) -> Result<()> {
    let out = resolve_openapi_output(out)?;
    let spec = openapi::openapi();
    let json = serde_json::to_string_pretty(&spec)?;
    std::fs::write(&out, json)?;
    println!("Wrote OpenAPI spec to {}", out.to_string_lossy());
    Ok(())
}

fn watch_openapi(out: Option<PathBuf>) -> Result<()> {
    let root = find_repo_root()?;
    let out = resolve_openapi_output(out)?;
    let watch_paths = vec![
        root.join("src/api.rs"),
        root.join("src/manifest.rs"),
        root.join("src/daemon.rs"),
        root.join("src/openapi.rs"),
        root.join("Cargo.toml"),
    ];
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    let mut watched_any = false;
    for path in &watch_paths {
        if path.exists() {
            watcher.watch(path, RecursiveMode::NonRecursive)?;
            watched_any = true;
        }
    }
    if !watched_any {
        return Err(anyhow!(
            "no watch paths found; run from the repo root so src/*.rs is available"
        ));
    }
    write_openapi(Some(out.clone()))?;
    println!("Watching for API changes...");
    let mut last_write = Instant::now() - Duration::from_secs(1);
    for event in rx {
        match event {
            Ok(event) => {
                if matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) && last_write.elapsed() >= Duration::from_millis(200)
                {
                    if let Err(err) = write_openapi(Some(out.clone())) {
                        eprintln!("warning: failed to write OpenAPI: {err}");
                    }
                    last_write = Instant::now();
                }
            }
            Err(err) => {
                eprintln!("warning: watcher error: {err}");
            }
        }
    }
    Ok(())
}

fn resolve_openapi_output(out: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(out) = out {
        return Ok(out);
    }
    Ok(find_repo_root()?.join("openapi.json"))
}

fn find_repo_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("Cargo.toml").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(anyhow!("could not find Cargo.toml (run from repo root)"))
}

fn resolve_project_dir_from_cwd() -> Result<PathBuf> {
    Ok(resolve_project_context(None, None)?.project_dir)
}

fn resolve_project_context(
    project: Option<PathBuf>,
    file: Option<PathBuf>,
) -> Result<ProjectContext> {
    let cwd = std::env::current_dir()?;
    resolve_project_context_with_cwd(&cwd, project, file)
}

fn resolve_project_context_with_cwd(
    cwd: &Path,
    project: Option<PathBuf>,
    file: Option<PathBuf>,
) -> Result<ProjectContext> {
    if let Some(file) = file {
        let file_path = absolutize_path(cwd, file);
        let project_dir = if let Some(project) = project {
            absolutize_path(cwd, project)
        } else {
            file_path.parent().unwrap_or(cwd).to_path_buf()
        };
        return Ok(ProjectContext {
            project_dir,
            config_path: Some(file_path),
        });
    }

    if let Some(project) = project {
        let project_dir = absolutize_path(cwd, project);
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

fn absolutize_path(base: &Path, path: PathBuf) -> PathBuf {
    let expanded = expand_home(&path);
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

fn looks_like_config_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.ends_with(".toml") || lower.ends_with(".yaml") || lower.ends_with(".yml")
}

fn resolve_up_context(
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
        let path = absolutize_path(&cwd, PathBuf::from(candidate));
        if path.is_file() {
            file = Some(path);
            stack = None;
        }
    }

    let context = resolve_project_context_with_cwd(&cwd, project, file)?;
    Ok((context, stack))
}

fn resolve_stack_name(stack: Option<String>, config_path: Option<&Path>) -> Result<String> {
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

async fn fetch_runs_with_fallback() -> Result<(RunListResponse, bool)> {
    match call_daemon::<serde_json::Value>("GET", "/v1/runs", None, Some(DAEMON_TIMEOUT)).await {
        Ok(response) => {
            let runs: RunListResponse = serde_json::from_value(response)?;
            Ok((runs, false))
        }
        Err(err) => {
            if let Ok(runs) = runs_from_disk() {
                Ok((RunListResponse { runs }, true))
            } else {
                Err(err)
            }
        }
    }
}

async fn fetch_runs() -> Result<RunListResponse> {
    fetch_runs_with_fallback().await.map(|(runs, _)| runs)
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
        let manifest = match RunManifest::load_from_path(&manifest_path) {
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

fn status_from_manifest(run_id: &str) -> Result<crate::api::RunStatusResponse> {
    let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(run_id))?;
    let manifest = RunManifest::load_from_path(&manifest_path)?;
    let desired = if manifest.state == crate::manifest::RunLifecycle::Stopped {
        "stopped".to_string()
    } else {
        "running".to_string()
    };
    let mut services = BTreeMap::new();
    for (name, svc) in manifest.services {
        services.insert(
            name,
            crate::api::ServiceStatus {
                desired: desired.clone(),
                systemd: None,
                ready: svc.state == crate::manifest::ServiceState::Ready,
                state: svc.state,
                last_failure: None,
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

async fn resolve_run_id(project_dir: &Path, run_id: Option<String>) -> Result<String> {
    if let Some(run_id) = run_id {
        return Ok(run_id);
    }
    let runs = fetch_runs().await?;
    let latest = select_latest_run(&runs.runs, project_dir).ok_or_else(|| {
        anyhow!(
            "no runs found for project {} (use --run or devstack ls --all)",
            project_dir.to_string_lossy()
        )
    })?;
    Ok(latest.run_id.clone())
}

async fn resolve_latest_run_id(project_dir: &Path) -> Result<Option<String>> {
    let (runs, _) = fetch_runs_with_fallback().await?;
    Ok(select_latest_run(&runs.runs, project_dir).map(|run| run.run_id.clone()))
}

async fn resolve_active_run_id(project_dir: &Path) -> Result<Option<String>> {
    let (runs, _) = fetch_runs_with_fallback().await?;
    Ok(select_latest_active_run(&runs.runs, project_dir).map(|run| run.run_id.clone()))
}

fn select_latest_run<'a>(runs: &'a [RunSummary], project_dir: &Path) -> Option<&'a RunSummary> {
    runs.iter()
        .filter(|run| same_project_dir(&run.project_dir, project_dir))
        .max_by(|a, b| compare_run_recency(a, b))
}

fn select_latest_active_run<'a>(
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

fn same_project_dir(run_project_dir: &str, project_dir: &Path) -> bool {
    let run_path = PathBuf::from(run_project_dir);
    if run_path == project_dir {
        return true;
    }
    let run_canon = std::fs::canonicalize(&run_path).unwrap_or(run_path);
    let project_canon =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    run_canon == project_canon
}

fn sort_runs_for_project(runs: &mut [RunSummary], project_dir: &Path) {
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

#[derive(Clone, Debug)]
enum TaskExecutionTarget {
    Run(crate::ids::RunId),
    AdHoc,
}

impl TaskExecutionTarget {
    fn scope(&self) -> crate::tasks::TaskLogScope<'_> {
        match self {
            Self::Run(run_id) => crate::tasks::TaskLogScope::Run(run_id),
            Self::AdHoc => crate::tasks::TaskLogScope::AdHoc,
        }
    }

    fn history_path(&self, project_dir: &Path) -> Result<PathBuf> {
        match self {
            Self::Run(run_id) => paths::task_history_path(run_id),
            Self::AdHoc => paths::ad_hoc_task_history_path(project_dir),
        }
    }
}

async fn resolve_task_execution_target(project_dir: &Path) -> Result<TaskExecutionTarget> {
    Ok(match resolve_active_run_id(project_dir).await? {
        Some(run_id) => TaskExecutionTarget::Run(crate::ids::RunId::new(run_id)),
        None => TaskExecutionTarget::AdHoc,
    })
}

fn task_log_path_candidates_for(
    project_dir: &Path,
    task_name: &str,
    explicit_run_id: Option<&str>,
    active_run_id: Option<&str>,
    latest_run_id: Option<&str>,
) -> Result<Vec<PathBuf>> {
    fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
        if !paths.iter().any(|candidate| candidate == &path) {
            paths.push(path);
        }
    }

    let mut candidates = Vec::new();
    if let Some(run_id) = explicit_run_id {
        push_unique(
            &mut candidates,
            paths::task_log_path(&crate::ids::RunId::new(run_id), task_name)?,
        );
        return Ok(candidates);
    }

    if let Some(run_id) = active_run_id {
        push_unique(
            &mut candidates,
            paths::task_log_path(&crate::ids::RunId::new(run_id), task_name)?,
        );
    }
    if let Some(run_id) = latest_run_id {
        push_unique(
            &mut candidates,
            paths::task_log_path(&crate::ids::RunId::new(run_id), task_name)?,
        );
    }
    push_unique(
        &mut candidates,
        paths::ad_hoc_task_log_path(project_dir, task_name)?,
    );
    Ok(candidates)
}

async fn task_log_path_candidates(
    project_dir: &Path,
    task_name: &str,
    explicit_run_id: Option<&str>,
) -> Result<Vec<PathBuf>> {
    if explicit_run_id.is_some() {
        return task_log_path_candidates_for(project_dir, task_name, explicit_run_id, None, None);
    }

    let active_run_id = resolve_active_run_id(project_dir).await?;
    let latest_run_id = resolve_latest_run_id(project_dir).await?;
    task_log_path_candidates_for(
        project_dir,
        task_name,
        None,
        active_run_id.as_deref(),
        latest_run_id.as_deref(),
    )
}

async fn complete(cword: usize, mut words: Vec<String>) -> Result<()> {
    if words.is_empty() {
        return Ok(());
    }
    if words[0] != "devstack"
        && let Some((idx, _)) = words.iter().enumerate().find(|(_, w)| *w == "devstack")
    {
        words = words[idx..].to_vec();
    }

    let cur = words.get(cword).cloned().unwrap_or_default();
    let prev = if cword > 0 {
        words.get(cword - 1).cloned().unwrap_or_default()
    } else {
        String::new()
    };
    let subcommand = find_subcommand(&words);

    let mut candidates: Vec<String> = Vec::new();
    if subcommand.is_none() {
        if cur.starts_with('-') {
            candidates.extend(global_options());
        } else {
            candidates.extend(subcommands());
        }
        return print_completions_filtered(candidates, &cur);
    }

    let sub = subcommand.unwrap();
    if is_option_value(&prev, &cur, "--run") || is_option_value(&prev, &cur, "--run-id") {
        if let Ok(project_dir) = resolve_project_dir_from_cwd()
            && let Ok(runs) = fetch_runs().await.map(|mut r| {
                sort_runs_for_project(&mut r.runs, &project_dir);
                r
            })
        {
            candidates = runs.runs.into_iter().map(|r| r.run_id).collect();
        }

        let option = if prev == "--run-id" || cur.starts_with("--run-id=") {
            "--run-id"
        } else {
            "--run"
        };
        let value_prefix = option_value_prefix(&cur, option);
        if cur.starts_with(&format!("{option}=")) {
            candidates = candidates
                .into_iter()
                .map(|id| format!("{option}={id}"))
                .collect();
            return print_completions_filtered(candidates, &format!("{option}={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--service") {
        if let Some(services) = completion_services(&words).await? {
            candidates = services;
        }
        let value_prefix = option_value_prefix(&cur, "--service");
        if cur.starts_with("--service=") {
            candidates = candidates
                .into_iter()
                .map(|svc| format!("--service={svc}"))
                .collect();
            return print_completions_filtered(candidates, &format!("--service={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--task") {
        if let Ok(tasks) = completion_tasks() {
            candidates = tasks;
        }
        let value_prefix = option_value_prefix(&cur, "--task");
        if cur.starts_with("--task=") {
            candidates = candidates
                .into_iter()
                .map(|task| format!("--task={task}"))
                .collect();
            return print_completions_filtered(candidates, &format!("--task={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--stack") {
        if let Ok(stacks) = completion_stacks() {
            candidates = stacks;
        }
        let value_prefix = option_value_prefix(&cur, "--stack");
        if cur.starts_with("--stack=") {
            candidates = candidates
                .into_iter()
                .map(|stack| format!("--stack={stack}"))
                .collect();
            return print_completions_filtered(candidates, &format!("--stack={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--file") || is_option_value(&prev, &cur, "--project") {
        return Ok(());
    }

    if sub == "up" && is_positional_stack(&words, cword, &cur) {
        if let Ok(stacks) = completion_stacks() {
            candidates = stacks;
        }
        return print_completions_filtered(candidates, &cur);
    }

    if cur.starts_with('-') {
        candidates = options_for_subcommand(&sub);
        return print_completions_filtered(candidates, &cur);
    }

    Ok(())
}

fn find_subcommand(words: &[String]) -> Option<String> {
    for word in words.iter().skip(1) {
        if word == "--pretty" {
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        return Some(word.clone());
    }
    None
}

fn subcommands() -> Vec<String> {
    vec![
        "install".to_string(),
        "init".to_string(),
        "daemon".to_string(),
        "up".to_string(),
        "status".to_string(),
        "watch".to_string(),
        "diagnose".to_string(),
        "ls".to_string(),
        "logs".to_string(),
        "show".to_string(),
        "down".to_string(),
        "kill".to_string(),
        "exec".to_string(),
        "lint".to_string(),
        "doctor".to_string(),
        "gc".to_string(),
        "ui".to_string(),
        "run".to_string(),
        "projects".to_string(),
        "sources".to_string(),
        "openapi".to_string(),
        "completions".to_string(),
    ]
}

fn global_options() -> Vec<String> {
    vec!["--pretty".to_string()]
}

fn options_for_subcommand(sub: &str) -> Vec<String> {
    match sub {
        "up" => vec![
            "--stack".to_string(),
            "--project".to_string(),
            "--run".to_string(),
            "--file".to_string(),
            "--no-wait".to_string(),
            "--all".to_string(),
            "--new".to_string(),
            "--force".to_string(),
        ],
        "status" => vec!["--run".to_string(), "--json".to_string()],
        "watch" => vec!["--service".to_string()],
        "diagnose" => vec!["--run".to_string(), "--service".to_string()],
        "ls" => vec!["--all".to_string()],
        "logs" => vec![
            "--run".to_string(),
            "--source".to_string(),
            "--facets".to_string(),
            "--all".to_string(),
            "--service".to_string(),
            "--task".to_string(),
            "--last".to_string(),
            "--search".to_string(),
            "--level".to_string(),
            "--stream".to_string(),
            "--since".to_string(),
            "--no-noise".to_string(),
            "--follow".to_string(),
            "--follow-for".to_string(),
            "--json".to_string(),
        ],
        "show" => vec![
            "--run".to_string(),
            "--service".to_string(),
            "--search".to_string(),
            "--level".to_string(),
            "--stream".to_string(),
            "--since".to_string(),
            "--last".to_string(),
        ],
        "down" => vec!["--run".to_string(), "--purge".to_string()],
        "kill" => vec!["--run".to_string()],
        "exec" => vec!["--run".to_string()],
        "gc" => vec!["--older-than".to_string(), "--all".to_string()],
        "init" => vec!["--project".to_string(), "--file".to_string()],
        "lint" => vec!["--project".to_string(), "--file".to_string()],
        "run" => vec![
            "--init".to_string(),
            "--stack".to_string(),
            "--project".to_string(),
            "--file".to_string(),
            "--verbose".to_string(),
            "--json".to_string(),
        ],
        "openapi" => vec!["--out".to_string(), "--watch".to_string()],
        _ => Vec::new(),
    }
}

fn is_option_value(prev: &str, cur: &str, option: &str) -> bool {
    prev == option || cur.starts_with(&format!("{option}="))
}

fn option_value_prefix(cur: &str, option: &str) -> String {
    if let Some(rest) = cur.strip_prefix(&format!("{option}=")) {
        rest.to_string()
    } else {
        cur.to_string()
    }
}

fn is_positional_stack(words: &[String], cword: usize, cur: &str) -> bool {
    if cur.starts_with('-') {
        return false;
    }
    let sub_idx = match words.iter().position(|w| w == "up") {
        Some(idx) => idx,
        None => return false,
    };
    if cword <= sub_idx {
        return false;
    }

    let mut i = sub_idx + 1;
    let mut stack_set = false;
    let mut all_set = false;
    while i < words.len() && i < cword {
        let word = &words[i];
        if word == "--" {
            break;
        }
        if word == "--all" {
            all_set = true;
            i += 1;
            continue;
        }
        if word == "--new" {
            i += 1;
            continue;
        }
        if word == "--force" {
            i += 1;
            continue;
        }
        if word == "--stack" {
            stack_set = true;
            i += 2;
            continue;
        }
        if word.starts_with("--stack=") {
            stack_set = true;
            i += 1;
            continue;
        }
        if let Some(step) = up_option_value_step(word) {
            i += step;
            continue;
        }
        if word.starts_with('-') {
            i += 1;
            continue;
        }
        return false;
    }

    if stack_set || all_set {
        return false;
    }

    if cword > 0
        && let Some(step) = up_option_value_step(&words[cword - 1])
        && step > 1
    {
        return false;
    }

    true
}

fn up_option_value_step(word: &str) -> Option<usize> {
    match word {
        "--stack" | "--project" | "--run" | "--run-id" | "--file" => Some(2),
        "--no-wait" | "--new" => Some(1),
        _ => {
            if word.starts_with("--stack=")
                || word.starts_with("--project=")
                || word.starts_with("--run=")
                || word.starts_with("--run-id=")
                || word.starts_with("--file=")
            {
                Some(1)
            } else {
                None
            }
        }
    }
}

fn completion_stacks() -> Result<Vec<String>> {
    let context = resolve_project_context(None, None)?;
    let config_path = match context.config_path {
        Some(path) if path.is_file() => path,
        _ => return Ok(Vec::new()),
    };
    let config = ConfigFile::load_from_path(&config_path)?;
    let mut stacks: Vec<String> = config.stacks.as_map().keys().cloned().collect();
    stacks.sort();
    Ok(stacks)
}

fn completion_tasks() -> Result<Vec<String>> {
    let context = resolve_project_context(None, None)?;
    let config_path = match context.config_path {
        Some(path) if path.is_file() => path,
        _ => return Ok(Vec::new()),
    };
    let config = ConfigFile::load_from_path(&config_path)?;
    let mut tasks: Vec<String> = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().keys().cloned().collect())
        .unwrap_or_default();
    tasks.sort();
    Ok(tasks)
}

async fn completion_services(words: &[String]) -> Result<Option<Vec<String>>> {
    let run_id = completion_run_id(words).await?;
    let run_id = match run_id {
        Some(run_id) => run_id,
        None => return Ok(None),
    };
    let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(&run_id))?;
    let manifest = match RunManifest::load_from_path(&manifest_path) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(None),
    };
    let mut services: Vec<String> = manifest.services.keys().cloned().collect();
    services.sort();
    Ok(Some(services))
}

async fn completion_run_id(words: &[String]) -> Result<Option<String>> {
    if let Some(value) = extract_arg_value(words, "--run") {
        return Ok(Some(value));
    }
    if let Some(value) = extract_arg_value(words, "--run-id") {
        return Ok(Some(value));
    }
    let project_dir = match resolve_project_dir_from_cwd() {
        Ok(dir) => dir,
        Err(_) => return Ok(None),
    };
    let runs = match fetch_runs().await {
        Ok(runs) => runs,
        Err(_) => return Ok(None),
    };
    Ok(select_latest_run(&runs.runs, &project_dir).map(|run| run.run_id.clone()))
}

fn extract_arg_value(words: &[String], option: &str) -> Option<String> {
    let mut iter = words.iter().peekable();
    while let Some(word) = iter.next() {
        if word == "--" {
            break;
        }
        if word == option
            && let Some(value) = iter.next()
        {
            return Some(value.clone());
        }
        if let Some(value) = word.strip_prefix(&format!("{option}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn print_completions_filtered(mut candidates: Vec<String>, prefix: &str) -> Result<()> {
    if !prefix.is_empty() {
        candidates.retain(|item| item.starts_with(prefix));
    }
    for item in candidates {
        println!("{item}");
    }
    Ok(())
}

fn print_completions(shell: &str) -> Result<()> {
    match shell {
        "bash" => {
            print!("{}", include_str!("../scripts/completions/devstack.bash"));
            Ok(())
        }
        "zsh" => {
            print!("{}", include_str!("../scripts/completions/devstack.zsh"));
            Ok(())
        }
        "fish" => {
            print!("{}", include_str!("../scripts/completions/devstack.fish"));
            Ok(())
        }
        _ => Err(anyhow!(
            "unsupported shell {shell} (use bash, zsh, or fish)"
        )),
    }
}

async fn install() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().context("current_exe")?;
        let home = std::env::var("HOME").context("HOME not set")?;
        let launch_agents = Path::new(&home).join("Library/LaunchAgents");
        std::fs::create_dir_all(&launch_agents)?;
        let plist_path = launch_agents.join("devstack.plist");

        let stdout_path = Path::new(&home).join("Library/Logs/devstack-daemon.log");
        let stderr_path = Path::new(&home).join("Library/Logs/devstack-daemon.err.log");
        let plist_contents = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.devstack.daemon</string>
    <key>ProgramArguments</key>
    <array>
      <string>{}</string>
      <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
  </dict>
</plist>
"#,
            exe.to_string_lossy(),
            stdout_path.to_string_lossy(),
            stderr_path.to_string_lossy()
        );
        std::fs::write(&plist_path, plist_contents)?;

        let _ = Command::new("launchctl")
            .arg("unload")
            .arg(&plist_path)
            .status();
        let status = Command::new("launchctl")
            .arg("load")
            .arg("-w")
            .arg(&plist_path)
            .status()?;
        if !status.success() {
            return Err(anyhow!("launchctl load -w failed"));
        }

        println!("Installed LaunchAgent at {}", plist_path.to_string_lossy());
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let exe = std::env::current_exe().context("current_exe")?;
        let service_dir = std::env::var("HOME")?;
        let service_dir = Path::new(&service_dir).join(".config/systemd/user");
        std::fs::create_dir_all(&service_dir)?;
        let unit_path = service_dir.join("devstack.service");
        let unit_contents = format!(
            "[Unit]\nDescription=devstack daemon\n\n[Service]\nType=notify\nExecStart={} daemon\nRestart=on-failure\nNotifyAccess=main\n\n[Install]\nWantedBy=default.target\n",
            exe.to_string_lossy()
        );
        std::fs::write(&unit_path, unit_contents)?;

        let status = Command::new("systemctl")
            .arg("--user")
            .arg("daemon-reload")
            .status()?;
        if !status.success() {
            return Err(anyhow!("systemctl --user daemon-reload failed"));
        }

        let status = Command::new("systemctl")
            .arg("--user")
            .arg("enable")
            .arg("--now")
            .arg("devstack.service")
            .status()?;
        if !status.success() {
            return Err(anyhow!("systemctl --user enable --now failed"));
        }

        println!(
            "Installed systemd user service at {}",
            unit_path.to_string_lossy()
        );
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        println!("devstack install is only supported on Linux or macOS.");
        println!("Run `devstack daemon` in a terminal.");
        return Ok(());
    }
}

async fn init(project: Option<PathBuf>, file: Option<PathBuf>) -> Result<()> {
    let project_dir = project.unwrap_or(std::env::current_dir()?);
    let config_path = file.unwrap_or_else(|| crate::config::ConfigFile::default_path(&project_dir));
    if config_path.exists() {
        return Err(anyhow!(
            "config already exists at {}",
            config_path.to_string_lossy()
        ));
    }
    let template = r#"# devstack config
version = 1

[stacks.app.services.api]
cmd = "python3 -m http.server {{ services.api.port }}"
readiness = { tcp = {} }

[stacks.app.services.web]
cmd = "python3 -m http.server {{ services.web.port }}"
deps = ["api"]
readiness = { tcp = {} }
env = { API_URL = "{{ services.api.url }}" }
"#;
    std::fs::create_dir_all(&project_dir)?;
    std::fs::write(&config_path, template)?;
    println!("Wrote {}", config_path.to_string_lossy());
    Ok(())
}

fn exec_command(run_id: &str, command: &[String]) -> Result<()> {
    if command.is_empty() {
        return Err(anyhow!("exec requires a command"));
    }
    let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(run_id))?;
    let manifest = RunManifest::load_from_path(&manifest_path)?;

    let mut cmd = Command::new(&command[0]);
    if command.len() > 1 {
        cmd.args(&command[1..]);
    }
    cmd.current_dir(&manifest.project_dir);
    cmd.envs(&manifest.env);
    cmd.stdin(std::process::Stdio::inherit());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    let status = cmd.status()?;
    let code = status.code().unwrap_or(1);
    std::process::exit(code);
}

#[allow(clippy::too_many_arguments)]
async fn run_task_command_cli(
    name: Option<String>,
    init: bool,
    stack: Option<String>,
    project: Option<PathBuf>,
    file: Option<PathBuf>,
    verbose: bool,
    json: bool,
    pretty: bool,
) -> Result<()> {
    let context = resolve_project_context(project, file)?;
    let config_path = context
        .config_path
        .ok_or_else(|| anyhow!("no devstack config found; run devstack init or pass --file"))?;
    if !config_path.is_file() {
        return Err(anyhow!(
            "config not found at {}; run devstack init or pass --file",
            config_path.to_string_lossy()
        ));
    }
    let config = ConfigFile::load_from_path(&config_path)?;
    let project_dir = context.project_dir;
    let task_target = resolve_task_execution_target(&project_dir).await?;

    let tasks_map = config
        .tasks
        .as_ref()
        .map(|t| t.as_map().clone())
        .unwrap_or_default();

    // devstack run --init: run all init tasks for the stack
    if init {
        let stack_name = resolve_stack_name(stack, Some(&config_path))?;
        let stack_plan = config
            .stack_plan(&stack_name)
            .map_err(|err| anyhow!("{err}"))?;

        let history_path = task_target.history_path(&project_dir)?;
        let mut ran_any = false;
        for svc_name in &stack_plan.order {
            let svc = &stack_plan.services[svc_name];
            if let Some(init_tasks) = &svc.init
                && !init_tasks.is_empty()
            {
                crate::tasks::run_init_tasks(
                    &tasks_map,
                    init_tasks,
                    &project_dir,
                    task_target.scope(),
                    &history_path,
                    verbose,
                )?;
                ran_any = true;
            }
        }
        if !ran_any {
            eprintln!("no init tasks defined for stack '{stack_name}'");
        }
        if json {
            print_json(
                serde_json::json!({ "ok": true, "mode": "init", "stack": stack_name }),
                pretty,
            );
        }
        return Ok(());
    }

    // devstack run (no name): list available tasks
    let Some(task_name) = name else {
        if tasks_map.is_empty() {
            eprintln!("no tasks defined in {}", config_path.to_string_lossy());
            return Ok(());
        }
        if json {
            let names: Vec<&String> = tasks_map.keys().collect();
            print_json(serde_json::json!({ "tasks": names }), pretty);
        } else {
            eprintln!("Available tasks:");
            for (name, task) in &tasks_map {
                let cmd = match task {
                    crate::config::TaskConfig::Command(cmd) => cmd.clone(),
                    crate::config::TaskConfig::Structured(def) => def.cmd.clone(),
                };
                eprintln!("  {name:<24} {cmd}");
            }
        }
        return Ok(());
    };

    // devstack run <name>: execute a specific task
    let task = tasks_map
        .get(&task_name)
        .ok_or_else(|| anyhow!("unknown task '{task_name}'"))?;

    let history_path = task_target.history_path(&project_dir)?;
    let result = crate::tasks::run_task(
        &task_name,
        task,
        &project_dir,
        task_target.scope(),
        &history_path,
        verbose,
    )?;

    if json {
        let stderr_summary = result
            .last_stderr_line
            .as_deref()
            .map(|line| crate::tasks::summarize_stderr_line(line, 120));
        print_json(
            serde_json::json!({
                "task": task_name,
                "exit_code": result.exit_code,
                "duration_ms": result.duration.as_millis(),
                "last_stderr_line": stderr_summary,
            }),
            pretty,
        );
    } else if result.success() {
        eprintln!(
            "✓ {} ({})",
            task_name,
            crate::tasks::format_task_duration(result.duration)
        );
    } else {
        let reason = result
            .last_stderr_line
            .as_deref()
            .map(|line| crate::tasks::summarize_stderr_line(line, 120))
            .filter(|line| !line.is_empty())
            .unwrap_or_else(|| format!("exit code {}", result.exit_code));
        eprintln!(
            "✗ {} ({}) — {}",
            task_name,
            crate::tasks::format_task_duration(result.duration),
            reason
        );
        eprintln!("  devstack logs --task {} --last 30", task_name);
    }

    if !result.success() {
        std::process::exit(result.exit_code);
    }
    Ok(())
}

fn lint(project: Option<PathBuf>, file: Option<PathBuf>, pretty: bool) -> Result<()> {
    let context = resolve_project_context(project, file)?;
    let config_path = context
        .config_path
        .ok_or_else(|| anyhow!("no devstack config found; run devstack init or pass --file"))?;
    if !config_path.is_file() {
        return Err(anyhow!(
            "config not found at {}; run devstack init or pass --file",
            config_path.to_string_lossy()
        ));
    }
    let config = ConfigFile::load_from_path(&config_path)?;
    let mut stacks: Vec<String> = config.stacks.as_map().keys().cloned().collect();
    stacks.sort();
    let mut globals: Vec<String> = config
        .globals
        .as_ref()
        .map(|g| g.as_map().keys().cloned().collect())
        .unwrap_or_default();
    globals.sort();

    let response = serde_json::json!({
        "ok": true,
        "path": config_path.to_string_lossy(),
        "default_stack": config.default_stack,
        "stacks": stacks,
        "globals": globals,
    });
    print_json(response, pretty);
    Ok(())
}

async fn doctor(pretty: bool) -> Result<()> {
    let response = crate::daemon::doctor().await;
    match response {
        Ok(result) => print_json(serde_json::to_value(result)?, pretty),
        Err(err) => return Err(err),
    }
    Ok(())
}

const DASHBOARD_PORT: u16 = 47832;

fn open_dashboard() -> Result<()> {
    let url = format!("http://localhost:{}", DASHBOARD_PORT);

    // Helpful hint when the dashboard isn't actually being served (it is spawned by the daemon
    // when the dashboard has been installed into the devstack data dir).
    let addr = SocketAddr::from(([127, 0, 0, 1], DASHBOARD_PORT));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
        eprintln!(
            "Note: no dashboard server detected on {}. Ensure the daemon is running (`devstack install` or `devstack daemon`) and the dashboard is installed (dev repo: `./scripts/install-cli.sh`).",
            addr
        );
    }

    // Don't fail in headless environments or minimal installs; always print the URL.
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = Command::new("open").arg(&url).spawn() {
            eprintln!("Warning: failed to open browser automatically: {e}");
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = Command::new("xdg-open").arg(&url).spawn() {
            eprintln!("Warning: failed to open browser automatically: {e}");
        }
    }

    println!("Opening dashboard at {}", url);
    Ok(())
}

fn require_existing_socket(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        return Ok(());
    }
    Err(anyhow!(
        "daemon socket missing at {}; run `devstack daemon` (foreground) or `devstack install` (system service)",
        socket_path.display()
    ))
}

async fn daemon_is_running() -> bool {
    paths::daemon_socket_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

async fn handle_sources(action: Option<SourcesAction>, pretty: bool) -> Result<()> {
    let action = action.unwrap_or(SourcesAction::Ls);

    match action {
        SourcesAction::Ls => {
            let ledger = SourcesLedger::load()?;
            let sources = ledger.list();
            if pretty {
                if sources.is_empty() {
                    println!("No sources registered.");
                } else {
                    for source in &sources {
                        println!("{}", source.name);
                        println!("  created: {}", source.created_at);
                        for path in &source.paths {
                            println!("  - {}", path);
                        }
                    }
                }
            } else {
                print_json(serde_json::json!({ "sources": sources }), false);
            }
        }
        SourcesAction::Add { name, paths } => {
            let patterns = absolutize_source_patterns(paths)?;
            if daemon_is_running().await {
                let req = crate::api::AddSourceRequest {
                    name: name.clone(),
                    paths: patterns,
                };
                call_daemon::<crate::api::AddSourceRequest>(
                    "POST",
                    "/v1/sources",
                    Some(req),
                    Some(DAEMON_TIMEOUT),
                )
                .await?;
            } else {
                let mut ledger = SourcesLedger::load()?;
                ledger.add(&name, patterns)?;
                refresh_source_index(&name).await?;
            }
            if pretty {
                println!("Added source: {name}");
            } else {
                print_json(serde_json::json!({ "ok": true, "name": name }), false);
            }
        }
        SourcesAction::Rm { name } => {
            if daemon_is_running().await {
                call_daemon::<()>(
                    "DELETE",
                    &format!("/v1/sources/{name}"),
                    None,
                    Some(DAEMON_TIMEOUT),
                )
                .await?;
            } else {
                let mut ledger = SourcesLedger::load()?;
                let removed = ledger.remove(&name)?;
                if !removed {
                    return Err(anyhow!("source not found: {name}"));
                }
                refresh_source_index(&name).await?;
            }
            if pretty {
                println!("Removed source: {name}");
            } else {
                print_json(serde_json::json!({ "ok": true, "name": name }), false);
            }
        }
    }

    Ok(())
}

async fn handle_projects(action: Option<ProjectsAction>, pretty: bool) -> Result<()> {
    let action = action.unwrap_or(ProjectsAction::Ls);

    match action {
        ProjectsAction::Ls => {
            let response =
                call_daemon::<serde_json::Value>("GET", "/v1/projects", None, Some(DAEMON_TIMEOUT))
                    .await?;
            let projects: ProjectsResponse = serde_json::from_value(response)?;

            if pretty {
                if projects.projects.is_empty() {
                    println!("No projects registered.");
                    println!(
                        "Run 'devstack up' in a project or 'devstack projects add <path>' to register."
                    );
                } else {
                    for project in &projects.projects {
                        let status = if project.config_exists {
                            format!("{} stacks", project.stacks.len())
                        } else {
                            "no config".to_string()
                        };
                        println!("{} ({})", project.name, status);
                        println!("  path: {}", project.path);
                        println!("  id:   {}", project.id);
                        if !project.stacks.is_empty() {
                            println!("  stacks: {}", project.stacks.join(", "));
                        }
                        if let Some(last_used) = &project.last_used {
                            println!("  last used: {}", last_used);
                        }
                        println!();
                    }
                }
            } else {
                println!("{}", serde_json::to_string(&projects)?);
            }
        }
        ProjectsAction::Add { path } => {
            let abs_path = std::fs::canonicalize(&path)
                .with_context(|| format!("path does not exist: {}", path.display()))?;

            let body = serde_json::json!({ "path": abs_path.to_string_lossy() });
            let response = call_daemon::<serde_json::Value>(
                "POST",
                "/v1/projects/register",
                Some(body),
                Some(DAEMON_TIMEOUT),
            )
            .await?;
            let registered: RegisterProjectResponse = serde_json::from_value(response)?;

            if pretty {
                println!("Registered project: {}", registered.project.name);
                println!("  path: {}", registered.project.path);
                println!("  id:   {}", registered.project.id);
            } else {
                println!("{}", serde_json::to_string(&registered)?);
            }
        }
        ProjectsAction::Remove { project } => {
            // Try to find by ID first, then by path
            let response =
                call_daemon::<serde_json::Value>("GET", "/v1/projects", None, Some(DAEMON_TIMEOUT))
                    .await?;
            let projects: ProjectsResponse = serde_json::from_value(response)?;

            let project_id = if let Some(p) = projects.projects.iter().find(|p| p.id == project) {
                p.id.clone()
            } else if let Some(p) = projects
                .projects
                .iter()
                .find(|p| p.path == project || p.name == project)
            {
                p.id.clone()
            } else {
                return Err(anyhow!("project not found: {}", project));
            };

            let _ = call_daemon::<serde_json::Value>(
                "DELETE",
                &format!("/v1/projects/{}", project_id),
                None,
                Some(DAEMON_TIMEOUT),
            )
            .await?;

            if pretty {
                println!("Removed project: {}", project);
            } else {
                println!(
                    "{}",
                    serde_json::json!({ "ok": true, "removed": project_id })
                );
            }
        }
    }

    Ok(())
}

#[allow(dead_code)]
async fn ping_daemon() -> Result<PingResponse> {
    let response =
        call_daemon::<serde_json::Value>("GET", "/v1/ping", None, Some(DAEMON_TIMEOUT)).await?;
    let ping: PingResponse = serde_json::from_value(response)?;
    Ok(ping)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::RunLifecycle;
    use clap::{CommandFactory, Parser};
    use std::fs;
    use std::time::Duration;

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
    fn task_log_path_candidates_prioritize_run_logs() {
        let dir = tempfile::tempdir().unwrap();
        let candidates = task_log_path_candidates_for(
            dir.path(),
            "lint",
            None,
            Some("run-active"),
            Some("run-latest"),
        )
        .unwrap();

        assert_eq!(
            candidates,
            vec![
                paths::task_log_path(&crate::ids::RunId::new("run-active"), "lint").unwrap(),
                paths::task_log_path(&crate::ids::RunId::new("run-latest"), "lint").unwrap(),
                paths::ad_hoc_task_log_path(dir.path(), "lint").unwrap(),
            ]
        );
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
    fn positional_stack_allowed_after_project_option() {
        let words = vec![
            "devstack".to_string(),
            "up".to_string(),
            "--project".to_string(),
            "/tmp/project".to_string(),
            "".to_string(),
        ];
        assert!(is_positional_stack(&words, 4, ""));
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
    fn format_compact_duration_prefers_largest_unit() {
        assert_eq!(format_compact_duration(5), "5s");
        assert_eq!(format_compact_duration(120), "2m");
        assert_eq!(format_compact_duration(7_200), "2h");
        assert_eq!(format_compact_duration(172_800), "2d");
    }

    #[test]
    fn logs_supports_new_flag_names() {
        let cli = Cli::try_parse_from([
            "devstack",
            "logs",
            "--run",
            "run-123",
            "--service",
            "api",
            "--search",
            "timeout",
            "--last",
            "25",
            "--no-noise",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                run_id,
                service,
                q,
                tail,
                no_health,
                ..
            } => {
                assert_eq!(run_id.as_deref(), Some("run-123"));
                assert_eq!(service.as_deref(), Some("api"));
                assert_eq!(q.as_deref(), Some("timeout"));
                assert_eq!(tail, Some(25));
                assert!(no_health);
            }
            other => panic!("expected logs command, got {other:?}"),
        }
    }

    #[test]
    fn logs_supports_legacy_hidden_aliases() {
        let cli = Cli::try_parse_from([
            "devstack",
            "logs",
            "--run-id",
            "run-123",
            "--service",
            "api",
            "--q",
            "timeout",
            "--tail",
            "25",
            "--no-health",
            "--errors",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                run_id,
                service,
                q,
                tail,
                no_health,
                errors,
                ..
            } => {
                assert_eq!(run_id.as_deref(), Some("run-123"));
                assert_eq!(service.as_deref(), Some("api"));
                assert_eq!(q.as_deref(), Some("timeout"));
                assert_eq!(tail, Some(25));
                assert!(no_health);
                assert!(errors);
            }
            other => panic!("expected logs command, got {other:?}"),
        }
    }

    #[test]
    fn logs_help_shows_new_flags_only() {
        let mut help = Vec::new();
        let mut command = Cli::command();
        let logs = command
            .find_subcommand_mut("logs")
            .expect("logs subcommand should exist");
        logs.write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();

        assert!(help.contains("--search"));
        assert!(help.contains("--last"));
        assert!(help.contains("--run"));
        assert!(help.contains("--no-noise"));
        assert!(!help.contains("--q"));
        assert!(!help.contains("--tail"));
        assert!(!help.contains("--run-id"));
        assert!(!help.contains("--no-health"));
        assert!(!help.contains("--errors"));
    }

    #[test]
    fn show_supports_log_filter_flags() {
        let cli = Cli::try_parse_from([
            "devstack",
            "show",
            "--run",
            "run-123",
            "--service",
            "api",
            "--search",
            "timeout",
            "--level",
            "warn",
            "--stream",
            "stderr",
            "--since",
            "15m",
            "--last",
            "25",
        ])
        .unwrap();

        match cli.command {
            Commands::Show {
                run_id,
                service,
                q,
                level,
                stream,
                since,
                tail,
            } => {
                assert_eq!(run_id.as_deref(), Some("run-123"));
                assert_eq!(service.as_deref(), Some("api"));
                assert_eq!(q.as_deref(), Some("timeout"));
                assert_eq!(level.as_deref(), Some("warn"));
                assert_eq!(stream.as_deref(), Some("stderr"));
                assert_eq!(since.as_deref(), Some("15m"));
                assert_eq!(tail, Some(25));
            }
            other => panic!("expected show command, got {other:?}"),
        }
    }

    #[test]
    fn show_help_lists_navigation_filters() {
        let mut help = Vec::new();
        let mut command = Cli::command();
        let show = command
            .find_subcommand_mut("show")
            .expect("show subcommand should exist");
        show.write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();

        assert!(help.contains("--service"));
        assert!(help.contains("--search"));
        assert!(help.contains("--level"));
        assert!(help.contains("--stream"));
        assert!(help.contains("--since"));
        assert!(help.contains("--last"));
    }

    #[test]
    fn agent_command_parses_auto_share_watch_and_no_auto_share_flags() {
        let cli = Cli::try_parse_from([
            "devstack",
            "agent",
            "--auto-share",
            "warn",
            "--watch",
            "api,worker",
            "--",
            "claude",
            "Debug this",
        ])
        .unwrap();

        match cli.command {
            Commands::Agent {
                auto_share,
                no_auto_share,
                watch,
                command,
                ..
            } => {
                assert_eq!(auto_share.as_deref(), Some("warn"));
                assert!(!no_auto_share);
                assert_eq!(watch, Some(vec!["api".to_string(), "worker".to_string()]));
                assert_eq!(
                    command,
                    vec!["claude".to_string(), "Debug this".to_string()]
                );
            }
            other => panic!("expected agent command, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "devstack",
            "agent",
            "--no-auto-share",
            "--",
            "pi",
            "Inspect",
        ])
        .unwrap();

        match cli.command {
            Commands::Agent {
                auto_share,
                no_auto_share,
                ..
            } => {
                assert_eq!(auto_share, None);
                assert!(no_auto_share);
            }
            other => panic!("expected agent command, got {other:?}"),
        }
    }

    #[test]
    fn visible_subcommands_and_flags_have_help_text() {
        fn assert_command_help(command: &clap::Command) {
            if command.is_hide_set() {
                return;
            }

            let about = command.get_about().and_then(|value| {
                let trimmed = value.to_string();
                if trimmed.trim().is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            });
            assert!(
                about.is_some(),
                "command '{}' is missing help text",
                command.get_name()
            );

            for arg in command.get_arguments() {
                if arg.is_hide_set() {
                    continue;
                }
                let help = arg
                    .get_help()
                    .or_else(|| arg.get_long_help())
                    .map(|value| value.to_string())
                    .filter(|value| !value.trim().is_empty());
                assert!(
                    help.is_some(),
                    "flag '{}' on command '{}' is missing help text",
                    arg.get_id(),
                    command.get_name()
                );
            }

            for sub in command.get_subcommands() {
                assert_command_help(sub);
            }
        }

        let command = Cli::command();
        assert_command_help(&command);
    }

    #[test]
    fn logs_facets_and_follow_are_mutually_exclusive() {
        let err = Cli::try_parse_from([
            "devstack",
            "logs",
            "--service",
            "api",
            "--facets",
            "--follow",
        ])
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--facets"));
        assert!(msg.contains("--follow"));
    }

    #[test]
    fn format_log_facets_pretty_output() {
        let response = LogFacetsResponse {
            total: 1756,
            filters: vec![
                crate::api::FacetFilter {
                    field: "service".to_string(),
                    kind: "select".to_string(),
                    values: vec![
                        FacetValueCount {
                            value: "pi-agent-2026-03-03".to_string(),
                            count: 1247,
                        },
                        FacetValueCount {
                            value: "cron-2026-03-03".to_string(),
                            count: 89,
                        },
                        FacetValueCount {
                            value: "pi-extension-2026-03-02".to_string(),
                            count: 412,
                        },
                    ],
                },
                crate::api::FacetFilter {
                    field: "level".to_string(),
                    kind: "toggle".to_string(),
                    values: vec![
                        FacetValueCount {
                            value: "info".to_string(),
                            count: 1583,
                        },
                        FacetValueCount {
                            value: "error".to_string(),
                            count: 42,
                        },
                        FacetValueCount {
                            value: "warn".to_string(),
                            count: 123,
                        },
                    ],
                },
                crate::api::FacetFilter {
                    field: "stream".to_string(),
                    kind: "toggle".to_string(),
                    values: vec![
                        FacetValueCount {
                            value: "stdout".to_string(),
                            count: 1700,
                        },
                        FacetValueCount {
                            value: "stderr".to_string(),
                            count: 48,
                        },
                    ],
                },
            ],
        };

        let output = format_log_facets("Source: pi", &response);
        assert!(output.starts_with("Source: pi\n\nservice [select] (3):\n"));
        assert!(output.contains("1,247"));
        assert!(output.contains("level [toggle] (3):\n"));
        assert!(output.contains("info"));
        assert!(output.contains("stream [toggle] (2):\n"));
        assert!(output.contains("1,700"));
    }

    #[test]
    fn is_service_healthy_uses_health_check_stats() {
        let ready = crate::api::ServiceStatus {
            desired: "running".to_string(),
            systemd: None,
            ready: true,
            state: ServiceState::Ready,
            last_failure: None,
            health: None,
            health_check_stats: Some(crate::api::HealthCheckStats {
                passes: 10,
                failures: 1,
                consecutive_failures: 1,
                last_check_at: None,
                last_ok: Some(true),
            }),
            uptime_seconds: Some(42),
            recent_errors: Vec::new(),
            url: Some("http://localhost:3000".to_string()),
            auto_restart: false,
            watch_paused: false,
            watch_active: false,
        };
        assert!(is_service_healthy(&ready));

        let unhealthy = crate::api::ServiceStatus {
            health_check_stats: Some(crate::api::HealthCheckStats {
                last_ok: Some(false),
                ..crate::api::HealthCheckStats {
                    passes: 10,
                    failures: 2,
                    consecutive_failures: 2,
                    last_check_at: None,
                    last_ok: Some(true),
                }
            }),
            ..ready
        };
        assert!(!is_service_healthy(&unhealthy));
    }

    #[test]
    fn source_query_ingests_json_and_preserves_structured_fields() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("external.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","level":"info","msg":"ready"}
{"time":"2025-01-01T00:00:01Z","stream":"stderr","level":"error","msg":"boom"}
"#,
        )
        .unwrap();

        let mut ledger = SourcesLedger::default();
        ledger.sources.insert(
            "ext".to_string(),
            crate::sources::SourceEntry {
                name: "ext".to_string(),
                paths: vec![log_path.to_string_lossy().to_string()],
                created_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );

        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let response = search_source_logs(
            &index,
            &ledger,
            "ext",
            LogSearchQuery {
                last: Some(10),
                since: None,
                search: None,
                level: None,
                stream: None,
                service: None,
            },
        )
        .unwrap();

        assert_eq!(ledger.list().len(), 1);
        assert_eq!(response.entries.len(), 2);
        assert_eq!(response.entries[0].ts, "2025-01-01T00:00:00Z");
        assert_eq!(response.entries[0].level, "info");
        assert_eq!(response.entries[1].ts, "2025-01-01T00:00:01Z");
        assert_eq!(response.entries[1].level, "error");
    }

    #[test]
    fn source_remove_cleans_up_index_entries() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("external.log");
        std::fs::write(
            &log_path,
            r#"{"time":"2025-01-01T00:00:00Z","stream":"stdout","msg":"ready"}
"#,
        )
        .unwrap();

        let mut ledger = SourcesLedger::default();
        ledger.sources.insert(
            "ext".to_string(),
            crate::sources::SourceEntry {
                name: "ext".to_string(),
                paths: vec![log_path.to_string_lossy().to_string()],
                created_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );

        let index = LogIndex::open_or_create_in(dir.path()).unwrap();
        let _ = search_source_logs(
            &index,
            &ledger,
            "ext",
            LogSearchQuery {
                last: Some(10),
                since: None,
                search: None,
                level: None,
                stream: None,
                service: None,
            },
        )
        .unwrap();

        let run_id = source_run_id("ext");
        let before = index
            .search_run(
                &run_id,
                &[],
                LogSearchQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                    service: None,
                },
            )
            .unwrap();
        assert!(before.total > 0);

        ledger.sources.remove("ext");
        index.delete_run(&run_id).unwrap();

        let after = index
            .search_run(
                &run_id,
                &[],
                LogSearchQuery {
                    last: Some(10),
                    since: None,
                    search: None,
                    level: None,
                    stream: None,
                    service: None,
                },
            )
            .unwrap();
        assert_eq!(after.total, 0);
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

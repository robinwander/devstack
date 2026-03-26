use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

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
        #[arg(long, conflicts_with_all = ["run_id", "all", "task"], help = "Query a registered external source")]
        source: Option<String>,
        /// Show available facet values for discoverability.
        #[arg(long, conflicts_with_all = ["follow", "tail", "task"], help = "Show available facet values for discoverability")]
        facets: bool,
        /// Search all services in the run (cannot be combined with --follow).
        #[arg(long, conflicts_with_all = ["service", "task", "source"], help = "Search all services in the run (cannot be combined with --follow)")]
        all: bool,
        /// Filter to a specific service.
        #[arg(long, required_unless_present_any = ["all", "task", "source", "facets"], conflicts_with_all = ["all", "task"], help = "Filter to a specific service")]
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
        /// Hand the task to the daemon and return immediately with an execution id.
        #[arg(long, conflicts_with_all = ["init", "status", "verbose"], help = "Hand the task to the daemon and return immediately with an execution id")]
        detach: bool,
        /// Query a detached task execution by id.
        #[arg(long, value_name = "TASK_ID", conflicts_with_all = ["name", "init", "stack", "project", "file", "verbose", "detach", "args"], help = "Query a detached task execution by id")]
        status: Option<String>,
        /// Stream task stdout/stderr directly to the terminal.
        #[arg(long, help = "Stream task stdout/stderr directly to the terminal")]
        verbose: bool,
        /// Output machine-readable JSON.
        #[arg(long, help = "Output machine-readable JSON")]
        json: bool,
        /// Extra arguments passed to the task command (after --).
        #[arg(last = true, allow_hyphen_values = true)]
        args: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

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
    fn logs_facets_accepts_search_and_source_service() {
        let cli = Cli::try_parse_from([
            "devstack",
            "logs",
            "--source",
            "ext",
            "--service",
            "api",
            "--facets",
            "--search",
            "timeout",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                source,
                service,
                facets,
                q,
                ..
            } => {
                assert_eq!(source.as_deref(), Some("ext"));
                assert_eq!(service.as_deref(), Some("api"));
                assert!(facets);
                assert_eq!(q.as_deref(), Some("timeout"));
            }
            other => panic!("expected logs command, got {other:?}"),
        }
    }
}

mod completions;
mod lifecycle;
mod logs;
mod projects;
mod setup;
mod sources;
mod tasks;

use anyhow::Result;

use crate::cli::args::Commands;
use crate::cli::context::CliContext;

pub(crate) async fn run(command: Commands, context: &CliContext) -> Result<()> {
    match command {
        Commands::Install => setup::install().await,
        Commands::Init { project, file } => setup::init(project, file).await,
        Commands::Daemon => crate::daemon::run_daemon().await,
        Commands::Up {
            targets,
            stack_flag,
            new,
            force,
            all,
            project,
            run_id,
            file,
            no_wait,
        } => {
            lifecycle::up(
                context,
                targets,
                stack_flag,
                new,
                force,
                all,
                project,
                run_id,
                file,
                no_wait,
            )
            .await
        }
        Commands::Status { run_id, json } => lifecycle::status(context, run_id, json).await,
        Commands::Watch { action } => lifecycle::watch(context, action).await,
        Commands::Diagnose { run_id, service } => lifecycle::diagnose(context, run_id, service).await,
        Commands::Ls { all } => lifecycle::list_runs(context, all).await,
        Commands::Logs {
            target,
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
            logs::run(
                context, run_id, source, facets, all, target, service, task, tail, q, level,
                errors, stream, since, no_health, follow, follow_for, json,
            )
            .await
        }
        Commands::Show {
            run_id,
            service,
            q,
            level,
            stream,
            since,
            tail,
        } => lifecycle::show(context, run_id, service, q, level, stream, since, tail).await,
        Commands::Agent {
            auto_share,
            no_auto_share,
            watch,
            run_id,
            command,
        } => lifecycle::agent(auto_share, no_auto_share, watch, run_id, command).await,
        Commands::Down { run_id, purge } => lifecycle::down(context, run_id, purge).await,
        Commands::Kill { run_id } => lifecycle::kill(context, run_id).await,
        Commands::Exec { run_id, command } => lifecycle::exec(context, run_id, command).await,
        Commands::Lint { project, file } => setup::lint(context, project, file),
        Commands::Doctor => setup::doctor(context).await,
        Commands::Completions { shell } => completions::print(&shell),
        Commands::Gc { older_than, all } => lifecycle::gc(context, older_than, all).await,
        Commands::Ui => lifecycle::ui(),
        Commands::Projects { action } => projects::run(context, action).await,
        Commands::Sources { action } => sources::run(context, action).await,
        Commands::Run {
            name,
            init,
            stack,
            project,
            file,
            detach,
            status,
            verbose,
            json,
            args,
        } => {
            tasks::run(
                context, name, init, stack, project, file, detach, status, verbose, json, args,
            )
            .await
        }
        Commands::Openapi { out, watch } => setup::openapi(out, watch),
        Commands::Complete { cword, words } => completions::complete(context, cword, words).await,
        Commands::Shim {
            run_id,
            service,
            cmd,
            cwd,
            log_file,
        } => {
            let args = crate::shim::ShimArgs {
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

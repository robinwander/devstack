mod args;
mod commands;
mod context;
mod output;

use anyhow::Result;
use clap::Parser;

pub use args::{Cli, Commands, ProjectsAction, SourcesAction, WatchAction};

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let interactive = context::is_interactive();
    let context = context::CliContext::new(interactive);
    commands::run(cli.command, &context).await
}

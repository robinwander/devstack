pub mod agent;
pub mod api;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod diagnose;
pub mod ids;
pub mod log_index;
pub mod logfmt;
pub mod logs;
pub mod manifest;
pub mod openapi;
pub mod paths;
pub mod port;
pub mod projects;
pub mod readiness;
pub mod shim;
pub mod sources;
pub mod systemd;
pub mod tasks;
pub mod util;
pub mod watch;

pub async fn run() -> anyhow::Result<()> {
    cli::run().await
}

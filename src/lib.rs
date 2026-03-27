pub mod agent;
pub mod api;
pub mod app;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod diagnose;
pub mod ids;
pub mod infra;
pub mod logfmt;
pub mod logs;
pub mod model;
pub mod openapi;
pub mod paths;
pub mod persistence;
pub mod port;
pub mod projects;
pub mod services;
pub mod shim;
pub mod sources;
pub mod stores;
pub mod systemd;
pub mod util;
pub mod watch;

pub async fn run() -> anyhow::Result<()> {
    cli::run().await
}

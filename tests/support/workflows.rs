#![allow(dead_code)]

use anyhow::Result;

use crate::support::fixtures::FixtureSpec;
use crate::support::{DaemonHandle, ProjectHandle, RunHandle, TestHarness, UpOptions};

pub async fn start_fixture_run<F: FixtureSpec>(
    harness: &TestHarness,
    fixture: F,
) -> Result<(DaemonHandle, ProjectHandle, RunHandle)> {
    start_fixture_run_with(harness, fixture, UpOptions::default()).await
}

pub async fn start_fixture_run_with<F: FixtureSpec>(
    harness: &TestHarness,
    fixture: F,
    options: UpOptions,
) -> Result<(DaemonHandle, ProjectHandle, RunHandle)> {
    let project = harness.fixture(fixture).create().await?;
    let daemon = harness.daemon().start().await?;
    let run = harness.cli().up_with(&project, options).await?;
    Ok((daemon, project, run))
}

pub async fn latest_run_for_project(
    harness: &TestHarness,
    project: &ProjectHandle,
) -> Result<RunHandle> {
    let runs = harness.api().list_runs().await?;
    let summary = runs
        .runs
        .into_iter()
        .filter(|run| run.project_dir == project.path_string())
        .max_by(|left, right| left.created_at.cmp(&right.created_at))
        .ok_or_else(|| anyhow::anyhow!("no run found for {}", project.path().display()))?;
    Ok(harness.run_handle(project, summary.run_id))
}

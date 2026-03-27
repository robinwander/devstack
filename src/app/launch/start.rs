use std::time::Duration;

use anyhow::{Context, Result};

use crate::model::{InstanceScope, ReadinessKind};
use crate::systemd::{ExecStart, UnitProperties};

use super::prepare::PreparedService;
use crate::app::context::AppContext;

pub async fn start_prepared_service(
    app: &AppContext,
    scope: &InstanceScope,
    prepared: &PreparedService,
    restart_existing: bool,
) -> Result<()> {
    if restart_existing {
        let _ = app.systemd.stop_unit(&prepared.unit_name).await;
        for _ in 0..50 {
            if let Ok(Some(status)) = app.systemd.unit_status(&prepared.unit_name).await {
                let active_state = status.active_state.as_str();
                if matches!(
                    active_state,
                    "active" | "activating" | "deactivating" | "reloading" | "maintenance"
                ) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            }
            break;
        }
    }

    let binary = app.binary_path.to_string_lossy().to_string();
    let exec = ExecStart {
        path: binary.clone(),
        argv: vec![
            binary.clone(),
            "__shim".to_string(),
            "--run-id".to_string(),
            scope_identifier(scope).to_string(),
            "--service".to_string(),
            prepared.name.clone(),
            "--cmd".to_string(),
            prepared.cmd.clone(),
            "--cwd".to_string(),
            prepared.cwd.to_string_lossy().to_string(),
            "--log-file".to_string(),
            prepared.log_path.to_string_lossy().to_string(),
        ],
        ignore_failure: false,
    };

    let properties = UnitProperties::new(
        format!("devstack {} {}", scope_label(scope), prepared.name),
        &prepared.cwd,
        prepared
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect(),
        exec,
    )
    .with_restart("no")
    .with_remain_after_exit(matches!(prepared.readiness.kind, ReadinessKind::Exit));

    app.systemd
        .start_transient_service(&prepared.unit_name, properties)
        .await
        .with_context(|| format!("start unit {}", prepared.unit_name))?;
    Ok(())
}

fn scope_identifier(scope: &InstanceScope) -> &str {
    match scope {
        InstanceScope::Run { run_id, .. } => run_id.as_str(),
        InstanceScope::Global { key, .. } => key,
    }
}

fn scope_label(scope: &InstanceScope) -> String {
    match scope {
        InstanceScope::Run { run_id, .. } => run_id.as_str().to_string(),
        InstanceScope::Global { key, .. } => format!("global:{key}"),
    }
}

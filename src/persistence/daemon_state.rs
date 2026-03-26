use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::RunLifecycle;
use crate::model::{RunRecord, ServiceRecord, ServiceSpec, ServiceLaunchPlan, ServiceRuntimeState};
use crate::paths;
use crate::services::readiness::ReadinessSpec;
use crate::util::atomic_write;
use crate::ids::RunId;

/// Daemon state file format for persistence
#[derive(Serialize, Deserialize)]
pub struct DaemonStateFile {
    pub runs: Vec<String>,
    pub updated_at: String,
}

/// Load daemon state from disk, reconstructing run records from manifests
pub fn load_state_from_disk() -> Result<BTreeMap<String, RunRecord>> {
    let mut runs = BTreeMap::new();
    let runs_dir = paths::runs_dir()?;
    if !runs_dir.exists() {
        return Ok(runs);
    }

    for entry in std::fs::read_dir(runs_dir)? {
        let entry = entry?;
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        
        if let Ok(manifest) = crate::manifest::RunManifest::load_from_path(&manifest_path) {
            // Skip stopped runs during daemon startup
            if manifest.state == RunLifecycle::Stopped || manifest.stopped_at.is_some() {
                continue;
            }
            
            let run_record = convert_manifest_to_run_record(manifest, &entry.path())?;
            runs.insert(run_record.run_id.as_str().to_string(), run_record);
        }
    }
    
    Ok(runs)
}

/// Convert a RunManifest back to a RunRecord for in-memory state
fn convert_manifest_to_run_record(
    manifest: crate::manifest::RunManifest,
    run_dir: &std::path::Path,
) -> Result<RunRecord> {
    let run_id = RunId::new(manifest.run_id.clone());
    let mut run_record = RunRecord::new(
        run_id.clone(),
        manifest.stack,
        PathBuf::from(manifest.project_dir),
        manifest.env,
    );
    
    run_record.state = manifest.state;
    run_record.created_at = manifest.created_at;
    run_record.stopped_at = manifest.stopped_at;
    
    // Convert service manifests to service records
    for (name, svc) in manifest.services {
        let spec = ServiceSpec {
            name: name.clone(),
            deps: Vec::new(),  // Not stored in manifest, will be recomputed
            readiness: ReadinessSpec::new(crate::services::readiness::ReadinessKind::None),
            auto_restart: false,
            watch_patterns: Vec::new(),
            ignore_patterns: Vec::new(),
        };
        
        let launch = ServiceLaunchPlan {
            unit_name: unit_name_for_run(run_id.as_str(), &name),
            cwd: PathBuf::from(&run_record.project_dir),
            env: run_record.base_env.clone(),
            cmd: String::new(),  // Not stored in manifest
            log_path: run_dir.join("logs").join(format!("{name}.log")),
            port: svc.port,
            scheme: "http".to_string(),
            url: svc.url,
            watch_hash: svc.watch_hash.unwrap_or_default(),
            watch_fingerprint: Vec::new(),
            watch_extra_files: Vec::new(),
        };
        
        let runtime = ServiceRuntimeState {
            state: svc.state,
            last_failure: None,
            last_started_at: Some(run_record.created_at.clone()),
            watch_paused: false,
        };
        
        let service_record = ServiceRecord {
            spec,
            launch,
            runtime,
        };
        
        run_record.insert_service(name, service_record);
    }
    
    Ok(run_record)
}

/// Generate systemd unit name for a service in a run
fn unit_name_for_run(run_id: &str, service: &str) -> String {
    format!("devstack-{}-{}.service", run_id, service)
}

/// Write daemon state file 
pub fn write_daemon_state_file(runs: &BTreeMap<String, RunRecord>) -> Result<()> {
    let daemon_state = DaemonStateFile {
        runs: runs.keys().cloned().collect(),
        updated_at: crate::util::now_rfc3339(),
    };
    
    let state_path = paths::daemon_state_path()?;
    let json = serde_json::to_vec_pretty(&daemon_state).context("serialize daemon state")?;
    atomic_write(&state_path, &json).context("write daemon state")
}
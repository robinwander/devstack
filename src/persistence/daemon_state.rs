use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ids::RunId;
use crate::manifest::RunLifecycle;
use crate::model::{RunRecord, ServiceLaunchPlan, ServiceRecord, ServiceSpec};
use crate::paths;
use crate::services::readiness::{ReadinessKind, ReadinessSpec};
use crate::util::atomic_write;

#[derive(Serialize, Deserialize)]
pub struct DaemonStateFile {
    pub runs: Vec<String>,
    pub updated_at: String,
}

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
            if manifest.state == RunLifecycle::Stopped || manifest.stopped_at.is_some() {
                continue;
            }

            let record = convert_manifest_to_run_record(manifest, &entry.path());
            runs.insert(record.run_id.as_str().to_string(), record);
        }
    }

    Ok(runs)
}

fn convert_manifest_to_run_record(
    manifest: crate::manifest::RunManifest,
    run_dir: &std::path::Path,
) -> RunRecord {
    let run_id = RunId::new(manifest.run_id.clone());
    let mut record = RunRecord::new(
        run_id.clone(),
        manifest.stack,
        PathBuf::from(manifest.project_dir),
        manifest.env,
    );
    record.state = manifest.state;
    record.created_at = manifest.created_at;
    record.stopped_at = manifest.stopped_at;

    for (name, service) in manifest.services {
        let spec = ServiceSpec {
            name: name.clone(),
            deps: Vec::new(),
            readiness: ReadinessSpec::new(ReadinessKind::None),
            auto_restart: false,
            watch_patterns: Vec::new(),
            ignore_patterns: Vec::new(),
        };
        let launch = ServiceLaunchPlan {
            unit_name: unit_name_for_run(run_id.as_str(), &name),
            cwd: record.project_dir.clone(),
            env: record.base_env.clone(),
            cmd: String::new(),
            log_path: run_dir.join("logs").join(format!("{name}.log")),
            port: service.port,
            scheme: "http".to_string(),
            url: service.url,
            watch_hash: service.watch_hash.unwrap_or_default(),
            watch_fingerprint: Vec::new(),
            watch_extra_files: Vec::new(),
        };
        let mut service_record = ServiceRecord::new(spec, launch);
        service_record.runtime.state = service.state;
        service_record.runtime.last_started_at = Some(record.created_at.clone());
        record.insert_service(name, service_record);
    }

    record
}

fn unit_name_for_run(run_id: &str, service: &str) -> String {
    format!("devstack-{}-{}.service", run_id, service)
}

pub fn write_daemon_state_file(runs: &BTreeMap<String, RunRecord>) -> Result<()> {
    let daemon_state = DaemonStateFile {
        runs: runs.keys().cloned().collect(),
        updated_at: crate::util::now_rfc3339(),
    };

    let state_path = paths::daemon_state_path()?;
    let json = serde_json::to_vec_pretty(&daemon_state).context("serialize daemon state")?;
    atomic_write(&state_path, &json).context("write daemon state")
}

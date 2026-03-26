use anyhow::Result;

use crate::api::{GlobalSummary, GlobalsResponse};
use crate::app::context::AppContext;
use crate::manifest::RunManifest;
use crate::paths;

pub async fn list_globals(_app: &AppContext) -> Result<GlobalsResponse> {
    Ok(GlobalsResponse {
        globals: list_globals_from_disk()?,
    })
}

pub fn list_globals_from_disk() -> Result<Vec<GlobalSummary>> {
    let mut globals = Vec::new();
    let root = paths::globals_root()?;
    if !root.exists() {
        return Ok(globals);
    }

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let key = entry.file_name().to_string_lossy().to_string();
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest = RunManifest::load_from_path(&manifest_path)?;
        let Some((name, service)) = manifest.services.iter().next() else {
            continue;
        };
        globals.push(GlobalSummary {
            key,
            name: name.clone(),
            project_dir: manifest.project_dir.clone(),
            state: manifest.state,
            port: service.port,
            url: service.url.clone(),
        });
    }

    Ok(globals)
}

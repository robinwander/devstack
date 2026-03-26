use anyhow::Result;

use crate::api::{GlobalSummary, GlobalsResponse};
use crate::app::context::AppContext;
use crate::paths;
use crate::persistence::PersistedGlobal;

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
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest = PersistedGlobal::load_from_path(&manifest_path)?;
        globals.push(GlobalSummary {
            key: manifest.key,
            name: manifest.name,
            project_dir: manifest.project_dir,
            state: manifest.state,
            port: manifest.service.port,
            url: manifest.service.url,
        });
    }

    Ok(globals)
}

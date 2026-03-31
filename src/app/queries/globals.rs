use anyhow::Result;

use crate::api::{GlobalSummary, GlobalsResponse};
use crate::app::context::AppContext;

pub async fn list_globals(app: &AppContext) -> Result<GlobalsResponse> {
    Ok(GlobalsResponse {
        globals: app
            .globals
            .list_globals()
            .await
            .into_iter()
            .map(|global| GlobalSummary {
                key: global.key,
                name: global.name,
                project_dir: global.project_dir.to_string_lossy().to_string(),
                state: global.state,
                port: global.service.launch.port,
                url: global.service.launch.url,
            })
            .collect(),
    })
}

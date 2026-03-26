use crate::api::{RunListResponse, RunSummary};
use crate::app::context::AppContext;

pub async fn list_runs(app: &AppContext) -> RunListResponse {
    let runs = app.runs.list_runs().await;
    RunListResponse {
        runs: runs
            .into_iter()
            .map(|run| RunSummary {
                run_id: run.run_id.as_str().to_string(),
                stack: run.stack,
                project_dir: run.project_dir.to_string_lossy().to_string(),
                state: run.state,
                created_at: run.created_at,
                stopped_at: run.stopped_at,
            })
            .collect(),
    }
}

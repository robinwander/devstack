use crate::api::{RunWatchResponse, WatchServiceStatus};
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;

pub async fn build_watch_status(app: &AppContext, run_id: &str) -> AppResult<RunWatchResponse> {
    app.runs
        .with_run(run_id, |run| RunWatchResponse {
            run_id: run_id.to_string(),
            services: run
                .services
                .iter()
                .map(|(name, service)| {
                    (
                        name.clone(),
                        WatchServiceStatus {
                            auto_restart: service.spec.auto_restart,
                            active: service.watch_active(),
                            paused: service.runtime.watch_paused,
                        },
                    )
                })
                .collect(),
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))
}

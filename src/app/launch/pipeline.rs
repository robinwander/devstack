use anyhow::{Result, anyhow};

use crate::app::commands::tasks::run_post_init_tasks_blocking;
use crate::app::context::AppContext;
use crate::services::readiness::ReadinessContext;

use super::prepare::PreparedService;
use super::readiness::PostInitContext;

pub async fn wait_for_prepared_service(
    app: &AppContext,
    service_name: &str,
    prepared: &PreparedService,
    post_init: Option<PostInitContext>,
) -> Result<()> {
    let context = ReadinessContext {
        port: prepared.port,
        scheme: prepared.scheme.clone(),
        log_path: prepared.log_path.clone(),
        cwd: prepared.cwd.clone(),
        env: prepared.env.clone(),
        unit_name: Some(prepared.unit_name.clone()),
        systemd: Some(app.systemd.clone()),
    };

    crate::readiness::wait_for_ready(&prepared.readiness, &context).await?;

    if let Some(post_init) = post_init {
        run_post_init_tasks_blocking(
            post_init.tasks_map,
            post_init.post_init_tasks,
            post_init.project_dir,
            post_init.run_id,
        )
        .await
        .map_err(|err| anyhow!("{service_name} post_init task failed: {err}"))?;
    }

    Ok(())
}

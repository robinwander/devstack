use crate::api::LatestAgentSessionResponse;
use crate::app::context::AppContext;

pub async fn latest_agent_session(
    app: &AppContext,
    project_dir: &str,
) -> LatestAgentSessionResponse {
    LatestAgentSessionResponse {
        session: app
            .agent_sessions
            .find_latest_for_project(project_dir)
            .await,
    }
}

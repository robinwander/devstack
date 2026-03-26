use std::sync::Arc;

use axum::{Router, routing::{delete, get, post}};
use tokio::sync::Mutex;

use crate::app::AppContext;

use super::handlers;
use super::log_tailing::RunLogTailRegistry;

#[derive(Clone)]
pub struct DaemonState {
    pub app: AppContext,
    pub log_tails: Arc<Mutex<RunLogTailRegistry>>,
    pub _lock: Arc<std::fs::File>,
}

pub fn build_router(state: DaemonState) -> Router {
    Router::new()
        .route("/v1/ping", get(handlers::ping::ping))
        .route("/v1/agent/sessions", post(handlers::agent::register_agent_session))
        .route(
            "/v1/agent/sessions/{agent_id}",
            delete(handlers::agent::unregister_agent_session),
        )
        .route(
            "/v1/agent/sessions/{agent_id}/messages",
            post(handlers::agent::post_agent_message),
        )
        .route(
            "/v1/agent/sessions/{agent_id}/messages/poll",
            get(handlers::agent::poll_agent_messages),
        )
        .route(
            "/v1/agent/sessions/latest",
            get(handlers::agent::get_latest_agent_session),
        )
        .route("/v1/agent/share", post(handlers::agent::share_agent_message))
        .route("/v1/events", get(handlers::events::events))
        .route("/v1/runs/up", post(handlers::runs::up))
        .route("/v1/runs/down", post(handlers::runs::down))
        .route("/v1/runs/kill", post(handlers::runs::kill))
        .route(
            "/v1/runs/{run_id}/restart-service",
            post(handlers::runs::restart_service),
        )
        .route("/v1/runs/{run_id}/status", get(handlers::runs::status))
        .route("/v1/runs", get(handlers::runs::list_runs))
        .route("/v1/tasks/run", post(handlers::tasks::start_task))
        .route("/v1/tasks/{execution_id}", get(handlers::tasks::task_status))
        .route("/v1/runs/{run_id}/tasks", get(handlers::tasks::run_tasks))
        .route("/v1/runs/{run_id}/watch", get(handlers::watch::watch_status))
        .route(
            "/v1/runs/{run_id}/watch/pause",
            post(handlers::watch::watch_pause),
        )
        .route(
            "/v1/runs/{run_id}/watch/resume",
            post(handlers::watch::watch_resume),
        )
        .route("/v1/runs/{run_id}/logs/{service}", get(handlers::logs::logs))
        .route("/v1/runs/{run_id}/logs", get(handlers::logs::logs_view))
        .route("/v1/globals", get(handlers::globals::list_globals))
        .route("/v1/projects", get(handlers::projects::list_projects))
        .route("/v1/projects/register", post(handlers::projects::register_project))
        .route("/v1/projects/{project_id}", delete(handlers::projects::remove_project))
        .route("/v1/sources", get(handlers::sources::list_sources).post(handlers::sources::add_source))
        .route("/v1/sources/{name}", delete(handlers::sources::remove_source))
        .route("/v1/sources/{name}/logs", get(handlers::sources::source_logs_view))
        .route(
            "/v1/navigation/intent",
            get(handlers::navigation::get_navigation_intent)
                .post(handlers::navigation::set_navigation_intent)
                .delete(handlers::navigation::clear_navigation_intent),
        )
        .route("/v1/gc", post(handlers::gc::gc))
        .with_state(state)
}

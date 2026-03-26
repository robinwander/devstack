use crate::api::{
    AgentSession, AgentSessionMessageRequest, AgentSessionMessageResponse,
    AgentSessionPollResponse, AgentSessionRegisterRequest, ShareAgentMessageRequest,
    ShareAgentMessageResponse,
};
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;

pub async fn register_agent_session(
    app: &AppContext,
    request: AgentSessionRegisterRequest,
) -> AgentSession {
    app.agent_sessions
        .register_session(
            request.agent_id,
            request.project_dir,
            request.stack,
            request.command,
            request.pid,
        )
        .await
}

pub async fn unregister_agent_session(app: &AppContext, agent_id: &str) -> AppResult<()> {
    let removed = app.agent_sessions.unregister_session(agent_id).await;
    if removed {
        Ok(())
    } else {
        Err(AppError::not_found(format!(
            "agent session {agent_id} not found"
        )))
    }
}

pub async fn post_agent_message(
    app: &AppContext,
    agent_id: &str,
    request: AgentSessionMessageRequest,
) -> AppResult<AgentSessionMessageResponse> {
    let queued = app
        .agent_sessions
        .queue_message(agent_id, request.message)
        .await
        .map_err(AppError::from)?;
    Ok(AgentSessionMessageResponse { queued })
}

pub async fn poll_agent_messages(
    app: &AppContext,
    agent_id: &str,
) -> AppResult<AgentSessionPollResponse> {
    let messages = app
        .agent_sessions
        .poll_messages(agent_id)
        .await
        .map_err(AppError::from)?;
    Ok(AgentSessionPollResponse { messages })
}

pub async fn share_agent_message(
    app: &AppContext,
    request: ShareAgentMessageRequest,
) -> AppResult<ShareAgentMessageResponse> {
    let session = app
        .agent_sessions
        .find_latest_for_project(&request.project_dir)
        .await
        .ok_or_else(|| {
            AppError::not_found(format!(
                "no active agent session found for project {}",
                request.project_dir
            ))
        })?;

    let message = match request.command {
        Some(command) if !command.is_empty() => format!("{}\nRun `{command}`", request.message),
        _ => request.message,
    };
    let queued = app
        .agent_sessions
        .queue_message(&session.agent_id, message)
        .await
        .map_err(AppError::from)?;

    Ok(ShareAgentMessageResponse {
        agent_id: session.agent_id,
        queued,
    })
}

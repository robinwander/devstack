use std::collections::BTreeMap;
use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use crate::model::AgentSessionState;
use crate::api::AgentSession;

/// Store for managing agent sessions
pub struct AgentSessionStore {
    inner: Mutex<BTreeMap<String, AgentSessionState>>,
}

impl AgentSessionStore {
    /// Create a new agent session store
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    /// Register or update an agent session
    pub async fn register_session(
        &self,
        agent_id: String,
        project_dir: String,
        stack: Option<String>,
        command: String,
        pid: u32,
    ) -> AgentSession {
        let mut guard = self.inner.lock().await;
        
        // Clean up stale sessions before registering
        self.cleanup_stale_sessions_internal(&mut guard);
        
        let _now = crate::util::now_rfc3339();
        let entry = guard
            .entry(agent_id.clone())
            .or_insert_with(|| AgentSessionState::new(
                agent_id.clone(),
                project_dir.clone(),
                stack.clone(),
                command.clone(),
                pid,
            ));

        // Update the session details
        entry.update(project_dir, stack, command, pid);
        
        convert_to_api_session(entry)
    }

    /// Unregister an agent session
    pub async fn unregister_session(&self, agent_id: &str) -> bool {
        let mut guard = self.inner.lock().await;
        self.cleanup_stale_sessions_internal(&mut guard);
        guard.remove(agent_id).is_some()
    }

    /// Queue a message for an agent session
    pub async fn queue_message(&self, agent_id: &str, message: String) -> Result<usize> {
        let mut guard = self.inner.lock().await;
        self.cleanup_stale_sessions_internal(&mut guard);
        
        let session = guard
            .get_mut(agent_id)
            .ok_or_else(|| anyhow!("agent session {} not found", agent_id))?;
        
        Ok(session.queue_message(message))
    }

    /// Poll messages from an agent session
    pub async fn poll_messages(&self, agent_id: &str) -> Result<Vec<String>> {
        let mut guard = self.inner.lock().await;
        self.cleanup_stale_sessions_internal(&mut guard);
        
        let session = guard
            .get_mut(agent_id)
            .ok_or_else(|| anyhow!("agent session {} not found", agent_id))?;
        
        Ok(session.drain_messages())
    }

    /// Find the latest agent session for a project
    pub async fn find_latest_for_project(&self, project_dir: &str) -> Option<AgentSession> {
        let mut guard = self.inner.lock().await;
        self.cleanup_stale_sessions_internal(&mut guard);
        
        guard
            .values()
            .filter(|session| session.project_dir == project_dir)
            .max_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then(a.agent_id.cmp(&b.agent_id))
            })
            .map(convert_to_api_session)
    }

    /// Get all active sessions
    pub async fn list_sessions(&self) -> Vec<AgentSession> {
        let mut guard = self.inner.lock().await;
        self.cleanup_stale_sessions_internal(&mut guard);
        
        guard
            .values()
            .map(convert_to_api_session)
            .collect()
    }

    /// Cleanup stale sessions (public interface)
    pub async fn cleanup_stale_sessions(&self) -> usize {
        let mut guard = self.inner.lock().await;
        let before_count = guard.len();
        self.cleanup_stale_sessions_internal(&mut guard);
        before_count - guard.len()
    }

    /// Internal cleanup that requires the lock to be held
    fn cleanup_stale_sessions_internal(&self, sessions: &mut BTreeMap<String, AgentSessionState>) {
        sessions.retain(|_, session| is_pid_alive(session.pid));
    }
}

impl Default for AgentSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert internal state to API response
fn convert_to_api_session(session: &AgentSessionState) -> AgentSession {
    AgentSession {
        agent_id: session.agent_id.clone(),
        project_dir: session.project_dir.clone(),
        stack: session.stack.clone(),
        command: session.command.clone(),
        pid: session.pid,
        created_at: session.created_at.clone(),
    }
}

/// Check if a process ID is alive (extracted from daemon.rs)
fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }

    let err = std::io::Error::last_os_error();
    err.raw_os_error() == Some(libc::EPERM)
}
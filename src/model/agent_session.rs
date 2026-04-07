use std::collections::VecDeque;

/// Agent session state in memory
#[derive(Clone, Debug)]
pub struct AgentSessionState {
    pub agent_id: String,
    pub project_dir: String,
    pub stack: Option<String>,
    pub command: String,
    pub pid: u32,
    pub created_at: String,
    pub pending_messages: VecDeque<String>,
}

impl AgentSessionState {
    /// Create a new agent session
    pub fn new(
        agent_id: String,
        project_dir: String,
        stack: Option<String>,
        command: String,
        pid: u32,
    ) -> Self {
        Self {
            agent_id,
            project_dir,
            stack,
            command,
            pid,
            created_at: crate::util::now_rfc3339(),
            pending_messages: VecDeque::new(),
        }
    }

    /// Add a message to the pending queue
    pub fn queue_message(&mut self, message: String) -> usize {
        self.pending_messages.push_back(message);
        self.pending_messages.len()
    }

    /// Drain all pending messages
    pub fn drain_messages(&mut self) -> Vec<String> {
        self.pending_messages.drain(..).collect()
    }

    /// Check if there are pending messages
    pub fn has_messages(&self) -> bool {
        !self.pending_messages.is_empty()
    }

    /// Update session details
    pub fn update(
        &mut self,
        project_dir: String,
        stack: Option<String>,
        command: String,
        pid: u32,
    ) {
        self.project_dir = project_dir;
        self.stack = stack;
        self.command = command;
        self.pid = pid;
    }
}

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::api::DaemonEvent;
use crate::infra::logs::index::LogIndex;
use crate::stores::{AgentSessionStore, GlobalStore, NavigationStore, RunStore, TaskStore};
use crate::systemd::SystemdManager;

use super::error::AppError;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Clone)]
pub struct AppContext {
    pub(crate) systemd: Arc<dyn SystemdManager>,
    pub(crate) runs: Arc<RunStore>,
    pub(crate) globals: Arc<GlobalStore>,
    pub(crate) tasks: Arc<TaskStore>,
    pub(crate) agent_sessions: Arc<AgentSessionStore>,
    pub(crate) navigation: Arc<NavigationStore>,
    pub(crate) binary_path: PathBuf,
    pub(crate) log_index: Arc<LogIndex>,
    pub(crate) event_tx: broadcast::Sender<DaemonEvent>,
}

impl AppContext {
    pub fn emit_event(&self, event: DaemonEvent) {
        let _ = self.event_tx.send(event);
    }

    pub fn emit_events<I>(&self, events: I)
    where
        I: IntoIterator<Item = DaemonEvent>,
    {
        for event in events {
            self.emit_event(event);
        }
    }
}

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use devstack::api::{
    DaemonEvent, DaemonGlobalEvent, DaemonLogEvent, DaemonRunEvent, DaemonServiceEvent,
    DaemonTaskEvent,
};
use devstack::model::{RunLifecycle, ServiceState};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::{DAEMON_TIMEOUT, EVENT_TIMEOUT, TestHarness, collect_events};

#[derive(Clone)]
pub struct EventsHandle {
    pub(super) harness: TestHarness,
}

impl EventsHandle {
    pub async fn subscribe(&self) -> Result<EventRecorder> {
        EventRecorder::subscribe(self.harness.clone(), None).await
    }

    pub async fn subscribe_run(&self, run_id: &str) -> Result<EventRecorder> {
        EventRecorder::subscribe(self.harness.clone(), Some(run_id.to_string())).await
    }
}

pub struct EventRecorder {
    harness: TestHarness,
    filter: Option<String>,
    events: Arc<Mutex<Vec<DaemonEvent>>>,
    task: JoinHandle<()>,
}

impl EventRecorder {
    async fn subscribe(harness: TestHarness, filter: Option<String>) -> Result<Self> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_for_task = events.clone();
        let harness_for_task = harness.clone();
        let filter_for_task = filter.clone();
        let (ready_tx, ready_rx) = oneshot::channel();

        let task = tokio::spawn(async move {
            if let Err(err) =
                collect_events(harness_for_task, filter_for_task, events_for_task, ready_tx).await
            {
                let message = err.to_string();
                if !message.contains("unexpected EOF during chunk size line")
                    && !message.contains("connection closed")
                    && !message.contains("broken pipe")
                {
                    eprintln!("e2e-event-recorder: {err:#}");
                }
            }
        });

        timeout(DAEMON_TIMEOUT, ready_rx)
            .await
            .context("wait for event subscription readiness")?
            .context("event subscription closed before becoming ready")?;

        Ok(Self {
            harness,
            filter,
            events,
            task,
        })
    }

    pub fn snapshot(&self) -> Vec<DaemonEvent> {
        self.events
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    pub async fn assert_run_created(&self, run_id: &str) -> Result<()> {
        let run_id_string = run_id.to_string();
        self.wait_for_event(
            format!("run {run_id} created event"),
            move |event| {
                matches!(
                    event,
                    DaemonEvent::Run(DaemonRunEvent {
                        run_id: current,
                        kind: devstack::api::DaemonRunEventKind::Created,
                        ..
                    }) if current == &run_id_string
                )
            },
            Some(run_id),
            None,
        )
        .await
    }

    pub async fn assert_service_state(
        &self,
        run_id: &str,
        service: &str,
        state: ServiceState,
    ) -> Result<()> {
        let run_id_string = run_id.to_string();
        let service_string = service.to_string();
        self.wait_for_event(
            format!("service {service} in run {run_id} to emit state {state:?}"),
            move |event| {
                matches!(
                    event,
                    DaemonEvent::Service(DaemonServiceEvent {
                        run_id: current_run,
                        service: current_service,
                        state: current_state,
                        ..
                    }) if current_run == &run_id_string && current_service == &service_string && current_state == &state
                )
            },
            Some(run_id),
            Some(service),
        )
        .await
    }

    pub async fn assert_task_started(&self, execution_id: &str) -> Result<()> {
        let execution_id_string = execution_id.to_string();
        self.wait_for_event(
            format!("task {execution_id} started event"),
            move |event| {
                matches!(
                    event,
                    DaemonEvent::Task(DaemonTaskEvent {
                        execution_id: current,
                        kind: devstack::api::DaemonTaskEventKind::Started,
                        ..
                    }) if current == &execution_id_string
                )
            },
            None,
            None,
        )
        .await
    }

    pub async fn assert_task_completed(&self, execution_id: &str) -> Result<()> {
        let execution_id_string = execution_id.to_string();
        self.wait_for_event(
            format!("task {execution_id} completed event"),
            move |event| {
                matches!(
                    event,
                    DaemonEvent::Task(DaemonTaskEvent {
                        execution_id: current,
                        kind: devstack::api::DaemonTaskEventKind::Completed,
                        ..
                    }) if current == &execution_id_string
                )
            },
            None,
            None,
        )
        .await
    }

    pub async fn assert_global_state(&self, key: &str, state: RunLifecycle) -> Result<()> {
        let key_string = key.to_string();
        self.wait_for_event(
            format!("global {key} to emit state {state:?}"),
            move |event| {
                matches!(
                    event,
                    DaemonEvent::Global(DaemonGlobalEvent {
                        key: current_key,
                        state: current_state,
                        ..
                    }) if current_key == &key_string && current_state == &state
                )
            },
            None,
            None,
        )
        .await
    }

    pub async fn assert_log_contains(
        &self,
        run_id: &str,
        service: &str,
        needle: &str,
    ) -> Result<()> {
        let run_id_string = run_id.to_string();
        let service_string = service.to_string();
        let needle_string = needle.to_string();
        self.wait_for_event(
            format!("log event for {service} in run {run_id} containing {needle:?}"),
            move |event| {
                matches!(
                    event,
                    DaemonEvent::Log(DaemonLogEvent {
                        run_id: current_run,
                        service: current_service,
                        message,
                        ..
                    }) if current_run == &run_id_string && current_service == &service_string && message.contains(&needle_string)
                )
            },
            Some(run_id),
            Some(service),
        )
        .await
    }

    async fn wait_for_event(
        &self,
        description: String,
        predicate: impl Fn(&DaemonEvent) -> bool + Send + Sync + 'static,
        run_id: Option<&str>,
        service: Option<&str>,
    ) -> Result<()> {
        let predicate = Arc::new(predicate);
        match self
            .harness
            .wait_until(EVENT_TIMEOUT, description, || {
                let snapshot = self.snapshot();
                let predicate = predicate.clone();
                async move {
                    if snapshot.iter().any(|event| predicate(event)) {
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
            })
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let mut details = String::new();
                if let Ok(json) = serde_json::to_string_pretty(&self.snapshot()) {
                    let _ = writeln!(details, "events:\n{json}");
                }
                details.push_str(&self.harness.diagnostics(run_id, service).await);
                Err(err.context(details))
            }
        }
    }
}

impl Drop for EventRecorder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

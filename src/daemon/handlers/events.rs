use std::convert::Infallible;

use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use tokio_stream::wrappers::ReceiverStream;

use crate::daemon::error::AppError;
use crate::daemon::log_tailing::{release_run_log_tail, retain_run_log_tail};
use crate::daemon::router::DaemonState;

#[derive(Debug, serde::Deserialize)]
pub struct EventStreamQuery {
    run_id: Option<String>,
}

struct LogTailSubscription {
    state: DaemonState,
    run_id: Option<String>,
}

impl Drop for LogTailSubscription {
    fn drop(&mut self) {
        let Some(run_id) = self.run_id.take() else {
            return;
        };
        let state = self.state.clone();
        tokio::spawn(async move {
            release_run_log_tail(&state, &run_id).await;
        });
    }
}

pub async fn events(
    State(state): State<DaemonState>,
    Query(query): Query<EventStreamQuery>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, AppError> {
    if let Some(run_id) = query.run_id.as_deref() {
        let exists = state.app.runs.contains_run(run_id).await;
        if !exists {
            return Err(AppError::not_found(format!("run {run_id} not found")));
        }
        retain_run_log_tail(&state, run_id).await.map_err(AppError::from)?;
    }

    let mut event_rx = state.app.event_tx.subscribe();
    let run_filter = query.run_id.clone();
    let subscription = LogTailSubscription {
        state: state.clone(),
        run_id: run_filter.clone(),
    };
    let (stream_tx, stream_rx) =
        tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);

    tokio::spawn(async move {
        let _subscription = subscription;
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if !event.should_deliver(run_filter.as_deref()) {
                        continue;
                    }
                    let payload = match event.payload_json() {
                        Ok(payload) => payload,
                        Err(err) => {
                            eprintln!("devstack: failed to serialize SSE event: {err}");
                            continue;
                        }
                    };
                    let sse_event = Event::default().event(event.event_name()).data(payload);
                    if stream_tx.send(Ok(sse_event)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(stream_rx)).keep_alive(KeepAlive::default()))
}

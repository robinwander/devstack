use crate::app::context::{AppContext, AppResult};
use crate::app::queries::status::build_status;
use crate::app::runtime::{run_state_changed_event, service_state_changed_event};
use crate::manifest::RunLifecycle;

pub async fn reconcile_run(app: &AppContext, run_id: &str) -> AppResult<()> {
    let status = build_status(app, run_id).await?;
    let events = app
        .runs
        .with_run_mut(run_id, |run| {
            let previous_run_state = run.state.clone();
            let mut events = Vec::new();

            for (name, service_status) in &status.services {
                if let Some(service) = run.services.get_mut(name) {
                    if service.runtime.state != service_status.state {
                        service.runtime.state = service_status.state.clone();
                        events.push(service_state_changed_event(
                            run_id,
                            name,
                            service_status.state.clone(),
                        ));
                    }
                    service.runtime.last_failure = service_status.last_failure.clone();
                }
            }

            run.state = status.state.clone();
            if run.state != previous_run_state && run.state != RunLifecycle::Stopped {
                events.push(run_state_changed_event(run));
            }
            events
        })
        .await?;
    app.emit_events(events);
    Ok(())
}

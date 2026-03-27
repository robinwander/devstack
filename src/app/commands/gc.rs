use std::time::{Duration, SystemTime};

use crate::api::{GcRequest, GcResponse};
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::runtime::{run_removed_event, write_daemon_state};
use crate::ids::RunId;
use crate::model::RunLifecycle;
use crate::paths;
use crate::persistence::PersistedGlobal;

pub async fn run_gc(app: &AppContext, request: GcRequest) -> AppResult<GcResponse> {
    let older_than = request
        .older_than
        .as_deref()
        .map(humantime::parse_duration)
        .transpose()
        .map_err(|err| AppError::bad_request(format!("invalid older_than duration: {err}")))?
        .unwrap_or_else(|| Duration::from_secs(7 * 24 * 3600));
    let threshold = SystemTime::now()
        .checked_sub(older_than)
        .ok_or_else(|| AppError::bad_request("older_than duration is too large".to_string()))?;

    let removed_runs = app
        .runs
        .with_runs_mut(|runs| {
            let run_ids = runs.keys().cloned().collect::<Vec<_>>();
            let mut removed = Vec::new();
            for run_id in run_ids {
                let Some(run) = runs.get(&run_id) else {
                    continue;
                };
                if run.state != RunLifecycle::Stopped {
                    continue;
                }
                if let Some(stopped_at) = &run.stopped_at {
                    if let Ok(stopped_time) = time::OffsetDateTime::parse(
                        stopped_at,
                        &time::format_description::well_known::Rfc3339,
                    ) {
                        if stopped_time > time::OffsetDateTime::from(threshold) {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                let run_dir = paths::run_dir(&RunId::new(&run_id)).unwrap();
                let _ = std::fs::remove_dir_all(run_dir);
                runs.remove(&run_id);
                removed.push(run_id);
            }
            removed
        })
        .await;

    let mut removed_globals = Vec::new();
    if request.all {
        let globals_root = paths::globals_root().map_err(AppError::from)?;
        if globals_root.exists() {
            for entry in std::fs::read_dir(globals_root).map_err(AppError::from)? {
                let entry = entry.map_err(AppError::from)?;
                let manifest_path = entry.path().join("manifest.json");
                if !manifest_path.exists() {
                    continue;
                }
                if let Ok(manifest) = PersistedGlobal::load_from_path(&manifest_path) {
                    if manifest.state != RunLifecycle::Stopped {
                        continue;
                    }
                    if let Some(stopped_at) = &manifest.stopped_at {
                        if let Ok(stopped_time) = time::OffsetDateTime::parse(
                            stopped_at,
                            &time::format_description::well_known::Rfc3339,
                        ) {
                            if stopped_time > time::OffsetDateTime::from(threshold) {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }
                    let _ = std::fs::remove_dir_all(entry.path());
                    removed_globals.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
    }

    if !removed_runs.is_empty() {
        for run_id in &removed_runs {
            app.emit_event(run_removed_event(run_id.clone()));
        }
        let index = app.log_index.clone();
        let removed = removed_runs.clone();
        tokio::task::spawn_blocking(move || {
            for run_id in removed {
                let _ = index.delete_run(&run_id);
            }
        })
        .await
        .ok();
    }

    let _ = write_daemon_state(app).await;
    Ok(GcResponse {
        removed_runs,
        removed_globals,
    })
}

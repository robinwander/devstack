use anyhow::{Result, anyhow};

use crate::api::{AddSourceRequest, SourceSummary, SourcesResponse};
use crate::cli::args::SourcesAction;
use crate::cli::commands::logs::{absolutize_source_patterns, refresh_source_index};
use crate::cli::context::{CliContext, DAEMON_LONG_TIMEOUT};
use crate::cli::output::print_toon;
use crate::daemon::bootstrap::log_index_max_age;
use crate::sources::{SourceEntry, SourcesLedger, source_retention_duration};

fn parse_retention_seconds(retention: Option<&str>) -> Result<Option<u64>> {
    let Some(retention) = retention else {
        return Ok(None);
    };
    let retention = retention.trim();
    if retention.is_empty() || retention.eq_ignore_ascii_case("default") {
        return Ok(None);
    }
    Ok(Some(humantime::parse_duration(retention)?.as_secs()))
}

fn source_summary(entry: &SourceEntry, default_retention: std::time::Duration) -> SourceSummary {
    SourceSummary {
        name: entry.name.clone(),
        paths: entry.paths.clone(),
        created_at: entry.created_at.clone(),
        retention_seconds: entry.retention_seconds,
        effective_retention_seconds: source_retention_duration(entry, default_retention).as_secs(),
    }
}

pub(crate) async fn run(context: &CliContext, action: Option<SourcesAction>) -> Result<()> {
    let action = action.unwrap_or(SourcesAction::Ls);

    match action {
        SourcesAction::Ls => {
            let ledger = SourcesLedger::load()?;
            let sources = ledger.list();
            if context.interactive {
                if sources.is_empty() {
                    println!("No sources registered.");
                } else {
                    for source in &sources {
                        println!("{}", source.name);
                        println!("  created: {}", source.created_at);
                        if let Some(retention_seconds) = source.retention_seconds {
                            println!("  retention: {}s", retention_seconds);
                        } else {
                            println!("  retention: default");
                        }
                        for path in &source.paths {
                            println!("  - {}", path);
                        }
                    }
                }
            } else {
                let default_retention = log_index_max_age();
                let sources = sources
                    .iter()
                    .map(|source| source_summary(source, default_retention))
                    .collect();
                print_toon(&SourcesResponse { sources });
            }
        }
        SourcesAction::Add {
            name,
            paths,
            retention,
        } => {
            let patterns = absolutize_source_patterns(paths)?;
            let retention_seconds = parse_retention_seconds(retention.as_deref())?;
            if context.daemon_is_running() {
                let req = AddSourceRequest {
                    name: name.clone(),
                    paths: patterns,
                    retention,
                };
                context
                    .daemon_request("POST", "/v1/sources", Some(req), Some(DAEMON_LONG_TIMEOUT))
                    .await?;
            } else {
                let mut ledger = SourcesLedger::load()?;
                ledger.add_with_retention(&name, patterns, retention_seconds)?;
                refresh_source_index(&name).await?;
            }
            if context.interactive {
                println!("Added source: {name}");
            } else {
                print_toon(&serde_json::json!({ "ok": true, "name": name }));
            }
        }
        SourcesAction::Rm { name } => {
            if context.daemon_is_running() {
                context
                    .daemon_request::<()>(
                        "DELETE",
                        &format!("/v1/sources/{name}"),
                        None,
                        Some(DAEMON_LONG_TIMEOUT),
                    )
                    .await?;
            } else {
                let mut ledger = SourcesLedger::load()?;
                let removed = ledger.remove(&name)?;
                if !removed {
                    return Err(anyhow!("source not found: {name}"));
                }
                refresh_source_index(&name).await?;
            }
            if context.interactive {
                println!("Removed source: {name}");
            } else {
                print_toon(&serde_json::json!({ "ok": true, "name": name }));
            }
        }
    }

    Ok(())
}

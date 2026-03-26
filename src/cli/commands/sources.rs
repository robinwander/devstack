use anyhow::{Result, anyhow};

use crate::api::AddSourceRequest;
use crate::cli::args::SourcesAction;
use crate::cli::context::{CliContext, DAEMON_LONG_TIMEOUT};
use crate::cli::commands::logs::{absolutize_source_patterns, refresh_source_index};
use crate::cli::output::print_json;
use crate::sources::SourcesLedger;

pub(crate) async fn run(context: &CliContext, action: Option<SourcesAction>) -> Result<()> {
    let action = action.unwrap_or(SourcesAction::Ls);

    match action {
        SourcesAction::Ls => {
            let ledger = SourcesLedger::load()?;
            let sources = ledger.list();
            if context.pretty {
                if sources.is_empty() {
                    println!("No sources registered.");
                } else {
                    for source in &sources {
                        println!("{}", source.name);
                        println!("  created: {}", source.created_at);
                        for path in &source.paths {
                            println!("  - {}", path);
                        }
                    }
                }
            } else {
                print_json(serde_json::json!({ "sources": sources }), false);
            }
        }
        SourcesAction::Add { name, paths } => {
            let patterns = absolutize_source_patterns(paths)?;
            if context.daemon_is_running() {
                let req = AddSourceRequest {
                    name: name.clone(),
                    paths: patterns,
                };
                context
                    .daemon_request(
                        "POST",
                        "/v1/sources",
                        Some(req),
                        Some(DAEMON_LONG_TIMEOUT),
                    )
                    .await?;
            } else {
                let mut ledger = SourcesLedger::load()?;
                ledger.add(&name, patterns)?;
                refresh_source_index(&name).await?;
            }
            if context.pretty {
                println!("Added source: {name}");
            } else {
                print_json(serde_json::json!({ "ok": true, "name": name }), false);
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
            if context.pretty {
                println!("Removed source: {name}");
            } else {
                print_json(serde_json::json!({ "ok": true, "name": name }), false);
            }
        }
    }

    Ok(())
}

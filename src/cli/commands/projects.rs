use anyhow::{Context, Result, anyhow};

use crate::api::{ProjectsResponse, RegisterProjectResponse};
use crate::cli::args::ProjectsAction;
use crate::cli::context::{CliContext, DAEMON_TIMEOUT};

pub(crate) async fn run(context: &CliContext, action: Option<ProjectsAction>) -> Result<()> {
    let action = action.unwrap_or(ProjectsAction::Ls);

    match action {
        ProjectsAction::Ls => {
            let projects: ProjectsResponse = context
                .daemon_request_json("GET", "/v1/projects", None::<()>, Some(DAEMON_TIMEOUT))
                .await?;

            if context.interactive {
                if projects.projects.is_empty() {
                    println!("No projects registered.");
                    println!(
                        "Run 'devstack up' in a project or 'devstack projects add <path>' to register."
                    );
                } else {
                    for project in &projects.projects {
                        let status = if project.config_exists {
                            format!("{} stacks", project.stacks.len())
                        } else {
                            "no config".to_string()
                        };
                        println!("{} ({})", project.name, status);
                        println!("  path: {}", project.path);
                        println!("  id:   {}", project.id);
                        if !project.stacks.is_empty() {
                            println!("  stacks: {}", project.stacks.join(", "));
                        }
                        if let Some(last_used) = &project.last_used {
                            println!("  last used: {}", last_used);
                        }
                        println!();
                    }
                }
            } else {
                crate::cli::output::print_toon(&projects);
            }
        }
        ProjectsAction::Add { path } => {
            let abs_path = std::fs::canonicalize(&path)
                .with_context(|| format!("path does not exist: {}", path.display()))?;

            let body = serde_json::json!({ "path": abs_path.to_string_lossy() });
            let registered: RegisterProjectResponse = context
                .daemon_request_json(
                    "POST",
                    "/v1/projects/register",
                    Some(body),
                    Some(DAEMON_TIMEOUT),
                )
                .await?;

            if context.interactive {
                println!("Registered project: {}", registered.project.name);
                println!("  path: {}", registered.project.path);
                println!("  id:   {}", registered.project.id);
            } else {
                crate::cli::output::print_toon(&registered);
            }
        }
        ProjectsAction::Remove { project } => {
            let projects: ProjectsResponse = context
                .daemon_request_json("GET", "/v1/projects", None::<()>, Some(DAEMON_TIMEOUT))
                .await?;

            let project_id =
                if let Some(found) = projects.projects.iter().find(|item| item.id == project) {
                    found.id.clone()
                } else if let Some(found) = projects
                    .projects
                    .iter()
                    .find(|item| item.path == project || item.name == project)
                {
                    found.id.clone()
                } else {
                    return Err(anyhow!("project not found: {}", project));
                };

            let _ = context
                .daemon_request::<()>(
                    "DELETE",
                    &format!("/v1/projects/{}", project_id),
                    None,
                    Some(DAEMON_TIMEOUT),
                )
                .await?;

            if context.interactive {
                println!("Removed project: {}", project);
            } else {
                crate::cli::output::print_toon(
                    &serde_json::json!({ "ok": true, "removed": project_id }),
                );
            }
        }
    }

    Ok(())
}

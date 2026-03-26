use anyhow::{Result, anyhow};

use crate::cli::context::{
    CliContext, fetch_runs, resolve_project_context, resolve_project_dir_from_cwd,
    sort_runs_for_project,
};
use crate::config::ConfigFile;
use crate::paths;
use crate::persistence::PersistedRun;

pub(crate) async fn complete(
    context: &CliContext,
    cword: usize,
    mut words: Vec<String>,
) -> Result<()> {
    if words.is_empty() {
        return Ok(());
    }
    if words[0] != "devstack"
        && let Some((idx, _)) = words
            .iter()
            .enumerate()
            .find(|(_, word)| *word == "devstack")
    {
        words = words[idx..].to_vec();
    }

    let cur = words.get(cword).cloned().unwrap_or_default();
    let prev = if cword > 0 {
        words.get(cword - 1).cloned().unwrap_or_default()
    } else {
        String::new()
    };
    let subcommand = find_subcommand(&words);

    let mut candidates: Vec<String> = Vec::new();
    if subcommand.is_none() {
        if cur.starts_with('-') {
            candidates.extend(global_options());
        } else {
            candidates.extend(subcommands());
        }
        return print_completions_filtered(candidates, &cur);
    }

    let sub = subcommand.unwrap();
    if is_option_value(&prev, &cur, "--run") || is_option_value(&prev, &cur, "--run-id") {
        if let Ok(project_dir) = resolve_project_dir_from_cwd()
            && let Ok(runs) = fetch_runs(context).await.map(|mut response| {
                sort_runs_for_project(&mut response.runs, &project_dir);
                response
            })
        {
            candidates = runs.runs.into_iter().map(|run| run.run_id).collect();
        }

        let option = if prev == "--run-id" || cur.starts_with("--run-id=") {
            "--run-id"
        } else {
            "--run"
        };
        let value_prefix = option_value_prefix(&cur, option);
        if cur.starts_with(&format!("{option}=")) {
            candidates = candidates
                .into_iter()
                .map(|id| format!("{option}={id}"))
                .collect();
            return print_completions_filtered(candidates, &format!("{option}={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--service") {
        if let Some(services) = completion_services(context, &words).await? {
            candidates = services;
        }
        let value_prefix = option_value_prefix(&cur, "--service");
        if cur.starts_with("--service=") {
            candidates = candidates
                .into_iter()
                .map(|service| format!("--service={service}"))
                .collect();
            return print_completions_filtered(candidates, &format!("--service={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--task") {
        if let Ok(tasks) = completion_tasks() {
            candidates = tasks;
        }
        let value_prefix = option_value_prefix(&cur, "--task");
        if cur.starts_with("--task=") {
            candidates = candidates
                .into_iter()
                .map(|task| format!("--task={task}"))
                .collect();
            return print_completions_filtered(candidates, &format!("--task={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--stack") {
        if let Ok(stacks) = completion_stacks() {
            candidates = stacks;
        }
        let value_prefix = option_value_prefix(&cur, "--stack");
        if cur.starts_with("--stack=") {
            candidates = candidates
                .into_iter()
                .map(|stack| format!("--stack={stack}"))
                .collect();
            return print_completions_filtered(candidates, &format!("--stack={value_prefix}"));
        }
        return print_completions_filtered(candidates, &value_prefix);
    }

    if is_option_value(&prev, &cur, "--file") || is_option_value(&prev, &cur, "--project") {
        return Ok(());
    }

    if sub == "up" && is_positional_stack(&words, cword, &cur) {
        if let Ok(stacks) = completion_stacks() {
            candidates = stacks;
        }
        return print_completions_filtered(candidates, &cur);
    }

    if cur.starts_with('-') {
        candidates = options_for_subcommand(&sub);
        return print_completions_filtered(candidates, &cur);
    }

    Ok(())
}

pub(crate) fn print(shell: &str) -> Result<()> {
    match shell {
        "bash" => {
            print!(
                "{}",
                include_str!("../../../scripts/completions/devstack.bash")
            );
            Ok(())
        }
        "zsh" => {
            print!(
                "{}",
                include_str!("../../../scripts/completions/devstack.zsh")
            );
            Ok(())
        }
        "fish" => {
            print!(
                "{}",
                include_str!("../../../scripts/completions/devstack.fish")
            );
            Ok(())
        }
        _ => Err(anyhow!(
            "unsupported shell {shell} (use bash, zsh, or fish)"
        )),
    }
}

fn find_subcommand(words: &[String]) -> Option<String> {
    for word in words.iter().skip(1) {
        if word == "--pretty" {
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        return Some(word.clone());
    }
    None
}

fn subcommands() -> Vec<String> {
    vec![
        "install".to_string(),
        "init".to_string(),
        "daemon".to_string(),
        "up".to_string(),
        "status".to_string(),
        "watch".to_string(),
        "diagnose".to_string(),
        "ls".to_string(),
        "logs".to_string(),
        "show".to_string(),
        "down".to_string(),
        "kill".to_string(),
        "exec".to_string(),
        "lint".to_string(),
        "doctor".to_string(),
        "gc".to_string(),
        "ui".to_string(),
        "run".to_string(),
        "projects".to_string(),
        "sources".to_string(),
        "openapi".to_string(),
        "completions".to_string(),
    ]
}

fn global_options() -> Vec<String> {
    vec!["--pretty".to_string()]
}

fn options_for_subcommand(sub: &str) -> Vec<String> {
    match sub {
        "up" => vec![
            "--stack".to_string(),
            "--project".to_string(),
            "--run".to_string(),
            "--file".to_string(),
            "--no-wait".to_string(),
            "--all".to_string(),
            "--new".to_string(),
            "--force".to_string(),
        ],
        "status" => vec!["--run".to_string(), "--json".to_string()],
        "watch" => vec!["--service".to_string()],
        "diagnose" => vec!["--run".to_string(), "--service".to_string()],
        "ls" => vec!["--all".to_string()],
        "logs" => vec![
            "--run".to_string(),
            "--source".to_string(),
            "--facets".to_string(),
            "--all".to_string(),
            "--service".to_string(),
            "--task".to_string(),
            "--last".to_string(),
            "--search".to_string(),
            "--level".to_string(),
            "--stream".to_string(),
            "--since".to_string(),
            "--no-noise".to_string(),
            "--follow".to_string(),
            "--follow-for".to_string(),
            "--json".to_string(),
        ],
        "show" => vec![
            "--run".to_string(),
            "--service".to_string(),
            "--search".to_string(),
            "--level".to_string(),
            "--stream".to_string(),
            "--since".to_string(),
            "--last".to_string(),
        ],
        "down" => vec!["--run".to_string(), "--purge".to_string()],
        "kill" => vec!["--run".to_string()],
        "exec" => vec!["--run".to_string()],
        "gc" => vec!["--older-than".to_string(), "--all".to_string()],
        "init" => vec!["--project".to_string(), "--file".to_string()],
        "lint" => vec!["--project".to_string(), "--file".to_string()],
        "run" => vec![
            "--init".to_string(),
            "--stack".to_string(),
            "--project".to_string(),
            "--file".to_string(),
            "--detach".to_string(),
            "--status".to_string(),
            "--verbose".to_string(),
            "--json".to_string(),
        ],
        "openapi" => vec!["--out".to_string(), "--watch".to_string()],
        _ => Vec::new(),
    }
}

fn is_option_value(prev: &str, cur: &str, option: &str) -> bool {
    prev == option || cur.starts_with(&format!("{option}="))
}

fn option_value_prefix(cur: &str, option: &str) -> String {
    if let Some(rest) = cur.strip_prefix(&format!("{option}=")) {
        rest.to_string()
    } else {
        cur.to_string()
    }
}

fn is_positional_stack(words: &[String], cword: usize, cur: &str) -> bool {
    if cur.starts_with('-') {
        return false;
    }
    let sub_idx = match words.iter().position(|word| word == "up") {
        Some(idx) => idx,
        None => return false,
    };
    if cword <= sub_idx {
        return false;
    }

    let mut index = sub_idx + 1;
    let mut stack_set = false;
    let mut all_set = false;
    while index < words.len() && index < cword {
        let word = &words[index];
        if word == "--" {
            break;
        }
        if word == "--all" {
            all_set = true;
            index += 1;
            continue;
        }
        if word == "--new" || word == "--force" {
            index += 1;
            continue;
        }
        if word == "--stack" {
            stack_set = true;
            index += 2;
            continue;
        }
        if word.starts_with("--stack=") {
            stack_set = true;
            index += 1;
            continue;
        }
        if let Some(step) = up_option_value_step(word) {
            index += step;
            continue;
        }
        if word.starts_with('-') {
            index += 1;
            continue;
        }
        return false;
    }

    if stack_set || all_set {
        return false;
    }

    if cword > 0
        && let Some(step) = up_option_value_step(&words[cword - 1])
        && step > 1
    {
        return false;
    }

    true
}

fn up_option_value_step(word: &str) -> Option<usize> {
    match word {
        "--stack" | "--project" | "--run" | "--run-id" | "--file" => Some(2),
        "--no-wait" | "--new" => Some(1),
        _ => {
            if word.starts_with("--stack=")
                || word.starts_with("--project=")
                || word.starts_with("--run=")
                || word.starts_with("--run-id=")
                || word.starts_with("--file=")
            {
                Some(1)
            } else {
                None
            }
        }
    }
}

fn completion_stacks() -> Result<Vec<String>> {
    let resolved_context = resolve_project_context(None, None)?;
    let config_path = match resolved_context.config_path {
        Some(path) if path.is_file() => path,
        _ => return Ok(Vec::new()),
    };
    let config = ConfigFile::load_from_path(&config_path)?;
    let mut stacks: Vec<String> = config.stacks.as_map().keys().cloned().collect();
    stacks.sort();
    Ok(stacks)
}

fn completion_tasks() -> Result<Vec<String>> {
    let resolved_context = resolve_project_context(None, None)?;
    let config_path = match resolved_context.config_path {
        Some(path) if path.is_file() => path,
        _ => return Ok(Vec::new()),
    };
    let config = ConfigFile::load_from_path(&config_path)?;
    let mut tasks: Vec<String> = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().keys().cloned().collect())
        .unwrap_or_default();
    tasks.sort();
    Ok(tasks)
}

async fn completion_services(
    context: &CliContext,
    words: &[String],
) -> Result<Option<Vec<String>>> {
    let run_id = completion_run_id(context, words).await?;
    let run_id = match run_id {
        Some(run_id) => run_id,
        None => return Ok(None),
    };
    let manifest_path = paths::run_manifest_path(&crate::ids::RunId::new(&run_id))?;
    let manifest = match PersistedRun::load_from_path(&manifest_path) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(None),
    };
    let mut services: Vec<String> = manifest.services.keys().cloned().collect();
    services.sort();
    Ok(Some(services))
}

async fn completion_run_id(context: &CliContext, words: &[String]) -> Result<Option<String>> {
    if let Some(value) = extract_arg_value(words, "--run") {
        return Ok(Some(value));
    }
    if let Some(value) = extract_arg_value(words, "--run-id") {
        return Ok(Some(value));
    }
    let project_dir = match resolve_project_dir_from_cwd() {
        Ok(dir) => dir,
        Err(_) => return Ok(None),
    };
    let runs = match fetch_runs(context).await {
        Ok(runs) => runs,
        Err(_) => return Ok(None),
    };
    Ok(
        crate::cli::context::select_latest_run(&runs.runs, &project_dir)
            .map(|run| run.run_id.clone()),
    )
}

fn extract_arg_value(words: &[String], option: &str) -> Option<String> {
    let mut iter = words.iter().peekable();
    while let Some(word) = iter.next() {
        if word == "--" {
            break;
        }
        if word == option
            && let Some(value) = iter.next()
        {
            return Some(value.clone());
        }
        if let Some(value) = word.strip_prefix(&format!("{option}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn print_completions_filtered(mut candidates: Vec<String>, prefix: &str) -> Result<()> {
    if !prefix.is_empty() {
        candidates.retain(|item| item.starts_with(prefix));
    }
    for item in candidates {
        println!("{item}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_stack_allowed_after_project_option() {
        let words = vec![
            "devstack".to_string(),
            "up".to_string(),
            "--project".to_string(),
            "/tmp/project".to_string(),
            "".to_string(),
        ];
        assert!(is_positional_stack(&words, 4, ""));
    }
}

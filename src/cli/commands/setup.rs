use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::cli::context::{CliContext, resolve_project_context};
use crate::cli::output::print_json;
use crate::config::ConfigFile;
use crate::openapi;

pub(crate) async fn install() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().context("current_exe")?;
        let home = std::env::var("HOME").context("HOME not set")?;
        let launch_agents = Path::new(&home).join("Library/LaunchAgents");
        std::fs::create_dir_all(&launch_agents)?;
        let plist_path = launch_agents.join("devstack.plist");

        let stdout_path = Path::new(&home).join("Library/Logs/devstack-daemon.log");
        let stderr_path = Path::new(&home).join("Library/Logs/devstack-daemon.err.log");
        let plist_contents = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.devstack.daemon</string>
    <key>ProgramArguments</key>
    <array>
      <string>{}</string>
      <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
  </dict>
</plist>
"#,
            exe.to_string_lossy(),
            stdout_path.to_string_lossy(),
            stderr_path.to_string_lossy()
        );
        std::fs::write(&plist_path, plist_contents)?;

        let _ = Command::new("launchctl")
            .arg("unload")
            .arg(&plist_path)
            .status();
        let status = Command::new("launchctl")
            .arg("load")
            .arg("-w")
            .arg(&plist_path)
            .status()?;
        if !status.success() {
            return Err(anyhow!("launchctl load -w failed"));
        }

        println!("Installed LaunchAgent at {}", plist_path.to_string_lossy());
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let exe = std::env::current_exe().context("current_exe")?;
        let service_dir = std::env::var("HOME")?;
        let service_dir = Path::new(&service_dir).join(".config/systemd/user");
        std::fs::create_dir_all(&service_dir)?;
        let unit_path = service_dir.join("devstack.service");
        let unit_contents = format!(
            "[Unit]\nDescription=devstack daemon\n\n[Service]\nType=notify\nExecStart={} daemon\nRestart=on-failure\nNotifyAccess=main\n\n[Install]\nWantedBy=default.target\n",
            exe.to_string_lossy()
        );
        std::fs::write(&unit_path, unit_contents)?;

        let status = Command::new("systemctl")
            .arg("--user")
            .arg("daemon-reload")
            .status()?;
        if !status.success() {
            return Err(anyhow!("systemctl --user daemon-reload failed"));
        }

        let status = Command::new("systemctl")
            .arg("--user")
            .arg("enable")
            .arg("--now")
            .arg("devstack.service")
            .status()?;
        if !status.success() {
            return Err(anyhow!("systemctl --user enable --now failed"));
        }

        println!(
            "Installed systemd user service at {}",
            unit_path.to_string_lossy()
        );
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        println!("devstack install is only supported on Linux or macOS.");
        println!("Run `devstack daemon` in a terminal.");
        Ok(())
    }
}

pub(crate) async fn init(project: Option<PathBuf>, file: Option<PathBuf>) -> Result<()> {
    let project_dir = project.unwrap_or(std::env::current_dir()?);
    let config_path = file.unwrap_or_else(|| crate::config::ConfigFile::default_path(&project_dir));
    if config_path.exists() {
        return Err(anyhow!(
            "config already exists at {}",
            config_path.to_string_lossy()
        ));
    }
    let template = r#"# devstack config
version = 1

[stacks.app.services.api]
cmd = "python3 -m http.server {{ services.api.port }}"
readiness = { tcp = {} }

[stacks.app.services.web]
cmd = "python3 -m http.server {{ services.web.port }}"
deps = ["api"]
readiness = { tcp = {} }
env = { API_URL = "{{ services.api.url }}" }
"#;
    std::fs::create_dir_all(&project_dir)?;
    std::fs::write(&config_path, template)?;
    println!("Wrote {}", config_path.to_string_lossy());
    Ok(())
}

pub(crate) fn lint(
    context: &CliContext,
    project: Option<PathBuf>,
    file: Option<PathBuf>,
) -> Result<()> {
    let resolved_context = resolve_project_context(project, file)?;
    let config_path = resolved_context
        .config_path
        .ok_or_else(|| anyhow!("no devstack config found; run devstack init or pass --file"))?;
    if !config_path.is_file() {
        return Err(anyhow!(
            "config not found at {}; run devstack init or pass --file",
            config_path.to_string_lossy()
        ));
    }
    let config = ConfigFile::load_from_path(&config_path)?;
    let mut stacks: Vec<String> = config.stacks.as_map().keys().cloned().collect();
    stacks.sort();
    let mut globals: Vec<String> = config
        .globals
        .as_ref()
        .map(|globals| globals.as_map().keys().cloned().collect())
        .unwrap_or_default();
    globals.sort();

    let response = serde_json::json!({
        "ok": true,
        "path": config_path.to_string_lossy(),
        "default_stack": config.default_stack,
        "stacks": stacks,
        "globals": globals,
    });
    print_json(response, context.pretty);
    Ok(())
}

pub(crate) async fn doctor(context: &CliContext) -> Result<()> {
    let response = crate::daemon::doctor().await?;
    print_json(serde_json::to_value(response)?, context.pretty);
    Ok(())
}

pub(crate) fn openapi(out: Option<PathBuf>, watch: bool) -> Result<()> {
    if watch {
        watch_openapi(out)?;
    } else {
        write_openapi(out)?;
    }
    Ok(())
}

fn write_openapi(out: Option<PathBuf>) -> Result<()> {
    let out = resolve_openapi_output(out)?;
    let spec = openapi::openapi();
    let json = serde_json::to_string_pretty(&spec)?;
    std::fs::write(&out, json)?;
    println!("Wrote OpenAPI spec to {}", out.to_string_lossy());
    Ok(())
}

fn watch_openapi(out: Option<PathBuf>) -> Result<()> {
    let root = find_repo_root()?;
    let out = resolve_openapi_output(out)?;
    let watch_paths = vec![
        root.join("src/api.rs"),
        root.join("src/model/lifecycle.rs"),
        root.join("src/daemon.rs"),
        root.join("src/openapi.rs"),
        root.join("Cargo.toml"),
    ];
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    let mut watched_any = false;
    for path in &watch_paths {
        if path.exists() {
            watcher.watch(path, RecursiveMode::NonRecursive)?;
            watched_any = true;
        }
    }
    if !watched_any {
        return Err(anyhow!(
            "no watch paths found; run from the repo root so src/*.rs is available"
        ));
    }
    write_openapi(Some(out.clone()))?;
    println!("Watching for API changes...");
    let mut last_write = Instant::now() - Duration::from_secs(1);
    for event in rx {
        match event {
            Ok(event) => {
                if matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) && last_write.elapsed() >= Duration::from_millis(200)
                {
                    if let Err(err) = write_openapi(Some(out.clone())) {
                        eprintln!("warning: failed to write OpenAPI: {err}");
                    }
                    last_write = Instant::now();
                }
            }
            Err(err) => {
                eprintln!("warning: watcher error: {err}");
            }
        }
    }
    Ok(())
}

fn resolve_openapi_output(out: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(out) = out {
        return Ok(out);
    }
    Ok(find_repo_root()?.join("openapi.json"))
}

fn find_repo_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("Cargo.toml").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(anyhow!("could not find Cargo.toml (run from repo root)"))
}

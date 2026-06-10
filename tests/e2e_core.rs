mod support;

use anyhow::Result;
use devstack::api::{LogFilterQuery, LogViewQuery};
use devstack::model::{RunLifecycle, ServiceState};
use devstack::persistence::PersistedRun;
use serde_json::Value;
use support::fixtures;
use support::workflows::start_fixture_run;
use support::{TestHarness, UpOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn http_post_json(url: &str, path: &str, body: &str) -> Result<String> {
    http_request(
        url,
        "POST",
        path,
        &[("Content-Type", "application/json")],
        Some(body),
    )
    .await
}

async fn http_request(
    url: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<String> {
    let prefix = "http://localhost:";
    let port = url
        .strip_prefix(prefix)
        .and_then(|rest| rest.split('/').next())
        .ok_or_else(|| anyhow::anyhow!("unsupported service url {url}"))?
        .parse::<u16>()?;

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let body = body.unwrap_or("");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n");
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    Ok(String::from_utf8_lossy(&response).to_string())
}

#[tokio::test]
async fn up_starts_simple_stack_and_status_reports_ready() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.service("api")
        .assert_log_contains("service-started name=api")
        .await?;
    t.fs(&project).assert_exists("state/api-starts.log")?;
    t.fs(&project)
        .assert_file_contains("state/api-starts.log", "started")?;
    assert!(run.manifest_path().exists());

    let status = t.cli().status_json(&project, run.id()).await?;
    assert_eq!(status.state, RunLifecycle::Running);
    assert_eq!(status.services["api"].state, ServiceState::Ready);

    run.down().await?;
    run.assert_stopped().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn api_capture_records_request_and_response_json() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;
    let response = http_post_json(
        &run.service("api").url().await?,
        "/debug",
        r#"{"hello":"world"}"#,
    )
    .await?;
    assert!(response.contains("201 Created"));
    assert!(response.contains(r#""received": {"hello": "world"}"#));

    let capture = t
        .wait_until(
            std::time::Duration::from_secs(5),
            "API capture log entry",
            || {
                let api = t.api();
                let run_id = run.id().to_string();
                async move {
                    let response = api
                        .logs_view(
                            &run_id,
                            &LogViewQuery {
                                filter: LogFilterQuery {
                                    last: Some(50),
                                    since: None,
                                    search: None,
                                    level: None,
                                    stream: None,
                                },
                                service: Some("api".to_string()),
                                include_entries: true,
                                include_facets: false,
                                include_total: true,
                            },
                        )
                        .await?;
                    Ok(response.entries.into_iter().find(|entry| {
                        entry.attributes.get("event").map(String::as_str) == Some("api_capture")
                    }))
                }
            },
        )
        .await?;

    assert!(capture.message.starts_with("POST /debug -> 201"));
    let json = capture
        .json
        .expect("capture entry should preserve raw JSON");
    assert_eq!(json["method"], "POST");
    assert_eq!(json["target"], "/debug");
    assert_eq!(json["status"], 201);
    assert_eq!(json["request"]["body"]["json"]["hello"], "world");
    assert_eq!(
        json["response"]["body"]["json"]["received"]["hello"],
        "world"
    );

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn api_capture_ignores_browser_non_xhr_requests() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;
    let url = run.service("api").url().await?;
    http_request(
        &url,
        "GET",
        "/",
        &[
            ("Accept", "text/html,application/xhtml+xml"),
            ("Sec-Fetch-Dest", "document"),
        ],
        None,
    )
    .await?;
    http_request(
        &url,
        "GET",
        "/asset.js",
        &[("Accept", "*/*"), ("Sec-Fetch-Dest", "script")],
        None,
    )
    .await?;
    http_request(
        &url,
        "POST",
        "/xhr",
        &[
            ("Content-Type", "application/json"),
            ("Sec-Fetch-Dest", "empty"),
        ],
        Some(r#"{"xhr":true}"#),
    )
    .await?;

    let entries = t
        .wait_until(
            std::time::Duration::from_secs(5),
            "XHR API capture log entry",
            || {
                let api = t.api();
                let run_id = run.id().to_string();
                async move {
                    let response = api
                        .logs_view(
                            &run_id,
                            &LogViewQuery {
                                filter: LogFilterQuery {
                                    last: Some(100),
                                    since: None,
                                    search: None,
                                    level: None,
                                    stream: Some("api".to_string()),
                                },
                                service: Some("api".to_string()),
                                include_entries: true,
                                include_facets: false,
                                include_total: true,
                            },
                        )
                        .await?;
                    let found_xhr = response.entries.iter().any(|entry| {
                        entry.attributes.get("event").map(String::as_str) == Some("api_capture")
                            && entry.attributes.get("target").map(String::as_str) == Some("/xhr")
                    });
                    Ok(found_xhr.then_some(response.entries))
                }
            },
        )
        .await?;

    assert!(!entries.iter().any(|entry| {
        entry.attributes.get("event").map(String::as_str) == Some("api_capture")
            && matches!(
                entry.attributes.get("target").map(String::as_str),
                Some("/" | "/asset.js")
            )
    }));

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn api_capture_caps_large_payloads_and_ignores_configured_paths() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            let mut api = config.service("dev", "api")?;
            api.capture_api(true)
                .capture_api_body_limit("1kb")
                .capture_api_ignore(&["/health", "/assets/*"]);
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_ready().await?;
    let url = run.service("api").url().await?;
    http_post_json(&url, "/health", r#"{"ignored":true}"#).await?;
    http_post_json(&url, "/assets/app.js", r#"{"ignored":true}"#).await?;

    let body = format!(r#"{{"blob":"{}"}}"#, "x".repeat(1024 * 1024));
    let response = http_post_json(&url, "/large", &body).await?;
    assert!(response.contains("201 Created"));
    assert!(response.contains(r#""blob""#));

    let entries = t
        .wait_until(
            std::time::Duration::from_secs(5),
            "large API capture log entry",
            || {
                let api = t.api();
                let run_id = run.id().to_string();
                async move {
                    let response = api
                        .logs_view(
                            &run_id,
                            &LogViewQuery {
                                filter: LogFilterQuery {
                                    last: Some(100),
                                    since: None,
                                    search: None,
                                    level: None,
                                    stream: Some("api".to_string()),
                                },
                                service: Some("api".to_string()),
                                include_entries: true,
                                include_facets: false,
                                include_total: true,
                            },
                        )
                        .await?;
                    let found_large = response.entries.iter().any(|entry| {
                        entry.attributes.get("event").map(String::as_str) == Some("api_capture")
                            && entry.attributes.get("target").map(String::as_str) == Some("/large")
                    });
                    Ok(found_large.then_some(response.entries))
                }
            },
        )
        .await?;

    assert!(!entries.iter().any(|entry| {
        entry.attributes.get("event").map(String::as_str) == Some("api_capture")
            && matches!(
                entry.attributes.get("target").map(String::as_str),
                Some("/health" | "/assets/app.js")
            )
    }));

    let large = entries
        .iter()
        .find(|entry| {
            entry.attributes.get("event").map(String::as_str) == Some("api_capture")
                && entry.attributes.get("target").map(String::as_str) == Some("/large")
        })
        .expect("large capture entry should exist");
    let json = large
        .json
        .as_ref()
        .expect("large capture entry should preserve raw JSON");
    assert_eq!(json["request"]["body"]["bytes"], body.len());
    assert_eq!(json["request"]["body"]["captured_bytes"], 1024);
    assert_eq!(json["request"]["body"]["truncated"], true);
    assert_eq!(json["response"]["body"]["truncated"], true);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_without_new_run_refreshes_existing_run() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let refreshed = t.cli().up(&project).await?;
    refreshed.assert_ready().await?;

    assert_eq!(refreshed.id(), run.id());

    refreshed.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn up_with_new_run_creates_distinct_run() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let second = t
        .cli()
        .up_with(
            run.project(),
            UpOptions {
                new_run: true,
                ..UpOptions::default()
            },
        )
        .await?;
    second.assert_ready().await?;

    assert_ne!(second.id(), run.id());

    run.down().await?;
    second.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn down_stops_run_and_marks_manifest_stopped() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.down().await?;
    run.assert_stopped().await?;

    let manifest: PersistedRun =
        serde_json::from_str(&std::fs::read_to_string(run.manifest_path())?)?;
    assert_eq!(manifest.state, RunLifecycle::Stopped);
    assert!(manifest.stopped_at.is_some());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn kill_force_stops_run_and_marks_manifest_stopped() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.kill().await?;
    run.assert_stopped().await?;

    let manifest: PersistedRun =
        serde_json::from_str(&std::fs::read_to_string(run.manifest_path())?)?;
    assert_eq!(manifest.state, RunLifecycle::Stopped);
    assert!(manifest.stopped_at.is_some());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn no_wait_returns_early_and_background_readiness_converges() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.readiness_delay_ms(1_500);
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t
        .cli()
        .up_with(
            &project,
            UpOptions {
                no_wait: true,
                ..UpOptions::default()
            },
        )
        .await?;

    let initial = run.status().await?;
    assert_ne!(initial.services["api"].state, ServiceState::Ready);

    run.assert_service_ready("api").await?;
    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_restart_preserves_visible_run_state() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;

    let daemon = daemon.restart().await?;
    daemon.assert_ping().await?;
    run.assert_service_ready("api").await?;

    let status = run.status().await?;
    assert_eq!(status.state, RunLifecycle::Running);
    assert_eq!(status.services["api"].state, ServiceState::Ready);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_stop_cleans_local_process_manager_units_including_globals() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::globals_fixture()).await?;

    run.assert_ready().await?;
    assert!(t.local_units_path().exists());

    run.down().await?;
    daemon.stop().await?;

    assert!(!t.local_units_path().exists());
    Ok(())
}

#[tokio::test]
async fn daemon_restart_restores_legacy_run_manifest_without_config_dir() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;

    let manifest_path = run.manifest_path().to_path_buf();
    let mut manifest: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    manifest
        .as_object_mut()
        .expect("run manifest object")
        .remove("config_dir");
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    let daemon = daemon.restart().await?;
    daemon.assert_ping().await?;
    run.assert_service_ready("api").await?;

    let status = run.status().await?;
    assert_eq!(status.state, RunLifecycle::Running);
    assert_eq!(status.services["api"].state, ServiceState::Ready);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn list_runs_reconciles_exited_service_state_and_persists_manifest() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .cmd("bash -lc 'echo api-ready; sleep 1; exit 17'")
                .port_none()
                .readiness_log_regex("api-ready");
            Ok(())
        })?
        .create()
        .await?;
    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;

    run.assert_service_ready("api").await?;

    t.wait_until(
        std::time::Duration::from_secs(10),
        "run list to reconcile exited service state",
        || {
            let api = t.api();
            let run_id = run.id().to_string();
            async move {
                let runs = api.list_runs().await?;
                let state = runs
                    .runs
                    .iter()
                    .find(|candidate| candidate.run_id == run_id)
                    .map(|candidate| candidate.state.clone());
                if state == Some(RunLifecycle::Degraded) {
                    Ok(Some(()))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await?;

    let manifest: PersistedRun =
        serde_json::from_str(&std::fs::read_to_string(run.manifest_path())?)?;
    assert_eq!(manifest.state, RunLifecycle::Degraded);
    assert_eq!(manifest.services["api"].state, ServiceState::Degraded);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

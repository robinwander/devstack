mod support;

use anyhow::{Result, anyhow};
use devstack::model::ServiceState;
use devstack::persistence::PersistedRun;
use support::TestHarness;
use support::fixtures;

fn service_port(url: &str) -> Result<u16> {
    url.strip_prefix("http://localhost:")
        .and_then(|value| value.split('/').next())
        .ok_or_else(|| anyhow!("unsupported service url {url}"))?
        .parse::<u16>()
        .map_err(Into::into)
}

fn available_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

async fn http_get(url: &str, path: &str) -> Result<String> {
    let port = service_port(url)?;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes()).await?;
    let mut response = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut response).await?;
    Ok(String::from_utf8_lossy(&response).to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_daemons_allocate_distinct_ports_for_same_stack() -> Result<()> {
    let t1 = TestHarness::new().await?;
    let t2 = TestHarness::new().await?;
    let project1 = t1.fixture(fixtures::simple_http()).create().await?;
    let project2 = t2.fixture(fixtures::simple_http()).create().await?;

    let daemon_controller1 = t1.daemon();
    let daemon_controller2 = t2.daemon();
    let (daemon1, daemon2) =
        tokio::try_join!(daemon_controller1.start(), daemon_controller2.start())?;
    let cli1 = t1.cli();
    let cli2 = t2.cli();
    let (run1, run2) = tokio::try_join!(cli1.up(&project1), cli2.up(&project2))?;

    tokio::try_join!(run1.assert_ready(), run2.assert_ready())?;

    let api1 = run1.service("api");
    let api2 = run2.service("api");
    let (url1, url2) = tokio::try_join!(api1.url(), api2.url())?;
    assert_ne!(service_port(&url1)?, service_port(&url2)?);

    tokio::try_join!(run1.down(), run2.down())?;
    tokio::try_join!(daemon1.stop(), daemon2.stop())?;
    Ok(())
}

#[tokio::test]
async fn fixed_port_is_reserved_across_daemons_until_run_stops() -> Result<()> {
    const PORT: u16 = 43101;

    let t1 = TestHarness::new().await?;
    let t2 = TestHarness::new().await?;
    let project1 = t1
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.port_fixed(PORT);
            Ok(())
        })?
        .create()
        .await?;
    let project2 = t2
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.port_fixed(PORT);
            Ok(())
        })?
        .create()
        .await?;

    let daemon1 = t1.daemon().start().await?;
    let daemon2 = t2.daemon().start().await?;
    let run1 = t1.cli().up(&project1).await?;
    run1.assert_ready().await?;

    let conflict = t2
        .cli()
        .run_in(
            &project2,
            &["up", "--project", &project2.path_string(), "--stack", "dev"],
        )
        .await?
        .failure()?;
    conflict.assert_stderr_contains("reserved by another devstack service")?;

    run1.down().await?;

    let run2 = t2.cli().up(&project2).await?;
    run2.assert_ready().await?;
    assert_eq!(service_port(&run2.service("api").url().await?)?, PORT);

    run2.down().await?;
    daemon1.stop().await?;
    daemon2.stop().await?;
    Ok(())
}

#[tokio::test]
async fn api_capture_fixed_port_remains_public_callback_port() -> Result<()> {
    let port = available_port()?;
    let t = TestHarness::new().await?;
    let project = t
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .port_fixed(port)
                .capture_api(true);
            Ok(())
        })?
        .create()
        .await?;

    let daemon = t.daemon().start().await?;
    let run = t.cli().up(&project).await?;
    run.assert_ready().await?;

    let url = run.service("api").url().await?;
    assert_eq!(service_port(&url)?, port);
    let manifest = PersistedRun::load_from_path(&run.manifest_path())?;
    let listen_port = manifest.services["api"]
        .listen_port
        .expect("captured service should have a backend listen port");
    assert_ne!(listen_port, port);
    run.service("api")
        .assert_log_contains(&format!("service-started name=api port={listen_port}"))
        .await?;

    let response = http_get(&url, "/callback").await?;
    assert!(response.contains("200 OK"));
    assert!(response.contains(&format!(r#""host": "localhost:{port}""#)));
    assert!(response.contains(&format!(r#""x_forwarded_host": "localhost:{port}""#)));
    assert!(response.contains(&format!(r#""x_forwarded_port": "{port}""#)));
    assert!(response.contains(r#""x_forwarded_proto": "http""#));
    assert!(!response.contains(&format!("localhost:{listen_port}")));
    run.service("api")
        .assert_log_contains(r#""target":"/callback""#)
        .await?;

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_restart_rehydrates_failed_run_port_reservations() -> Result<()> {
    const PORT: u16 = 43102;
    let run_id = "failed-port-reservation";

    let t1 = TestHarness::new().await?;
    let t2 = TestHarness::new().await?;
    let failing = t1
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config
                .service("dev", "api")?
                .port_fixed(PORT)
                .cmd("bash -lc 'echo startup-broke; exit 17'");
            Ok(())
        })?
        .create()
        .await?;
    let competing = t2
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.port_fixed(PORT);
            Ok(())
        })?
        .create()
        .await?;

    let daemon1 = t1.daemon().start().await?;
    let daemon2 = t2.daemon().start().await?;

    let failed_up = t1
        .cli()
        .run_in(
            &failing,
            &[
                "up",
                "--project",
                &failing.path_string(),
                "--stack",
                "dev",
                "--run-id",
                run_id,
            ],
        )
        .await?
        .failure()?;
    failed_up.assert_stderr_contains("service exited before readiness")?;

    let failed_run = t1.run_handle(&failing, run_id);
    failed_run.service("api").assert_failed().await?;
    let daemon1 = daemon1.restart().await?;
    daemon1.assert_ping().await?;

    let conflict = t2
        .cli()
        .run_in(
            &competing,
            &[
                "up",
                "--project",
                &competing.path_string(),
                "--stack",
                "dev",
            ],
        )
        .await?
        .failure()?;
    conflict.assert_stderr_contains("reserved by another devstack service")?;

    failed_run.down().await?;

    let run2 = t2.cli().up(&competing).await?;
    run2.assert_ready().await?;
    assert_eq!(
        run2.status().await?.services["api"].state,
        ServiceState::Ready
    );
    assert_eq!(service_port(&run2.service("api").url().await?)?, PORT);

    run2.down().await?;
    daemon1.stop().await?;
    daemon2.stop().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_restart_rehydrates_global_port_reservations() -> Result<()> {
    let port = available_port()?;

    let t1 = TestHarness::new().await?;
    let t2 = TestHarness::new().await?;
    let globals_project = t1
        .fixture(fixtures::globals_fixture())
        .with_config_patch(|config| {
            config.global_service("cache")?.port_fixed(port);
            Ok(())
        })?
        .create()
        .await?;
    let competing = t2
        .fixture(fixtures::simple_http())
        .with_config_patch(|config| {
            config.service("dev", "api")?.port_fixed(port);
            Ok(())
        })?
        .create()
        .await?;

    let daemon1 = t1.daemon().start().await?;
    let daemon2 = t2.daemon().start().await?;
    let run = t1.cli().up(&globals_project).await?;
    run.assert_ready().await?;

    let daemon1 = daemon1.restart().await?;
    daemon1.assert_ping().await?;

    let conflict = t2
        .cli()
        .run_in(
            &competing,
            &[
                "up",
                "--project",
                &competing.path_string(),
                "--stack",
                "dev",
            ],
        )
        .await?
        .failure()?;
    conflict.assert_stderr_contains("reserved by another devstack service")?;

    run.down().await?;
    daemon1.stop().await?;
    daemon2.stop().await?;
    Ok(())
}

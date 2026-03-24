mod support;

use std::time::Duration;

use anyhow::Result;
use devstack::api::SetNavigationIntentRequest;
use devstack::manifest::RunLifecycle;
use support::fixtures;
use support::workflows::start_fixture_run;
use support::TestHarness;

#[tokio::test]
async fn navigation_intent_round_trip() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;

    let initial = t.api().get_navigation_intent().await?;
    assert!(initial.intent.is_none());

    let set = t
        .api()
        .set_navigation_intent(&SetNavigationIntentRequest {
            run_id: Some("run-1".to_string()),
            service: Some("api".to_string()),
            search: Some("panic".to_string()),
            level: Some("error".to_string()),
            stream: Some("stderr".to_string()),
            since: Some("1h".to_string()),
            last: Some(25),
        })
        .await?;
    let intent = set.intent.expect("navigation intent stored");
    assert_eq!(intent.run_id.as_deref(), Some("run-1"));
    assert_eq!(intent.service.as_deref(), Some("api"));
    assert_eq!(intent.search.as_deref(), Some("panic"));

    let fetched = t.api().get_navigation_intent().await?;
    assert_eq!(fetched.intent.as_ref().and_then(|it| it.last), Some(25));

    let cleared = t.api().clear_navigation_intent().await?;
    assert_eq!(cleared["ok"], true);
    let after_clear = t.api().get_navigation_intent().await?;
    assert!(after_clear.intent.is_none());

    let _ = project;
    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn agent_session_register_poll_share_unregister_round_trip() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;
    let agent_id = "agent-round-trip";

    let registered = t
        .api()
        .register_agent_session(&devstack::api::AgentSessionRegisterRequest {
            agent_id: agent_id.to_string(),
            project_dir: project.path().to_string_lossy().to_string(),
            stack: Some("dev".to_string()),
            command: "claude".to_string(),
            pid: std::process::id(),
        })
        .await?;
    assert_eq!(registered.agent_id, agent_id);

    let queued = t.api().send_agent_message(agent_id, "first").await?;
    assert_eq!(queued.queued, 1);
    let queued = t.api().send_agent_message(agent_id, "second").await?;
    assert_eq!(queued.queued, 2);

    let polled = t.api().poll_agent_messages(agent_id).await?;
    assert_eq!(polled.messages, vec!["first".to_string(), "second".to_string()]);
    let polled = t.api().poll_agent_messages(agent_id).await?;
    assert!(polled.messages.is_empty());

    let shared = t
        .api()
        .share_agent_message(
            &project,
            "investigate this",
            Some("devstack logs --run run-1 --service api --level error".to_string()),
        )
        .await?;
    assert_eq!(shared.agent_id, agent_id);
    assert_eq!(shared.queued, 1);

    let shared_messages = t.api().poll_agent_messages(agent_id).await?;
    assert_eq!(
        shared_messages.messages,
        vec!["investigate this\nRun `devstack logs --run run-1 --service api --level error`".to_string()]
    );

    t.api().unregister_agent_session(agent_id).await?;
    let latest = t.api().latest_agent_session(&project).await?;
    assert!(latest.session.is_none());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn share_targets_latest_session_for_project() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let other = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;

    t.api()
        .register_agent_session(&devstack::api::AgentSessionRegisterRequest {
            agent_id: "agent-old".to_string(),
            project_dir: project.path().to_string_lossy().to_string(),
            stack: None,
            command: "claude".to_string(),
            pid: std::process::id(),
        })
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    t.api()
        .register_agent_session(&devstack::api::AgentSessionRegisterRequest {
            agent_id: "agent-new".to_string(),
            project_dir: project.path().to_string_lossy().to_string(),
            stack: None,
            command: "claude".to_string(),
            pid: std::process::id(),
        })
        .await?;
    t.api()
        .register_agent_session(&devstack::api::AgentSessionRegisterRequest {
            agent_id: "agent-other".to_string(),
            project_dir: other.path().to_string_lossy().to_string(),
            stack: None,
            command: "claude".to_string(),
            pid: std::process::id(),
        })
        .await?;

    let latest = t.api().latest_agent_session(&project).await?;
    assert_eq!(latest.session.as_ref().map(|session| session.agent_id.as_str()), Some("agent-new"));

    let shared = t
        .api()
        .share_agent_message(&project, "check latest", None)
        .await?;
    assert_eq!(shared.agent_id, "agent-new");

    let old_messages = t.api().poll_agent_messages("agent-old").await?;
    let new_messages = t.api().poll_agent_messages("agent-new").await?;
    assert!(old_messages.messages.is_empty());
    assert_eq!(new_messages.messages, vec!["check latest".to_string()]);

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn gc_removes_old_stopped_runs() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    run.down().await?;
    let response = t.api().gc(Some("0s".to_string()), false).await?;
    assert!(response.removed_runs.contains(&run.id().to_string()));
    assert!(!t.run_dir(run.id()).exists());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn stale_agent_sessions_are_eventually_cleaned_up() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start().await?;

    t.api()
        .register_agent_session(&devstack::api::AgentSessionRegisterRequest {
            agent_id: "agent-dead".to_string(),
            project_dir: project.path().to_string_lossy().to_string(),
            stack: None,
            command: "claude".to_string(),
            pid: u32::MAX,
        })
        .await?;

    let latest = t.api().latest_agent_session(&project).await?;
    assert!(latest.session.is_none());

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn gc_invalid_duration_fails_clearly() -> Result<()> {
    let t = TestHarness::new().await?;
    let daemon = t.daemon().start().await?;

    let err = t
        .api()
        .gc(Some("not-a-duration".to_string()), false)
        .await
        .expect_err("invalid gc duration should fail");
    assert!(err.to_string().contains("invalid older_than duration"));

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn gc_does_not_remove_running_runs() -> Result<()> {
    let t = TestHarness::new().await?;
    let (daemon, _project, run) = start_fixture_run(&t, fixtures::simple_http()).await?;

    run.assert_ready().await?;
    let response = t.api().gc(Some("0s".to_string()), false).await?;
    assert!(response.removed_runs.is_empty());
    assert!(t.run_dir(run.id()).exists());
    assert_eq!(run.status().await?.state, RunLifecycle::Running);

    run.down().await?;
    daemon.stop().await?;
    Ok(())
}

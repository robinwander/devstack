mod support;

use std::time::Duration;

use anyhow::Result;
use devstack::api::{LogFilterQuery, LogViewQuery};
use support::fixtures;
use support::TestHarness;

fn view_query(last: usize) -> LogViewQuery {
    LogViewQuery {
        filter: LogFilterQuery {
            last: Some(last),
            since: None,
            search: None,
            level: None,
            stream: None,
        },
        service: None,
        include_entries: true,
        include_facets: false,
    }
}

fn jsonl_line(ts: &str, stream: &str, level: &str, msg: &str) -> String {
    format!(
        "{{\"time\":\"{ts}\",\"stream\":\"{stream}\",\"level\":\"{level}\",\"msg\":\"{msg}\"}}\n"
    )
}

fn ts_rfc3339(dt: time::OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

const EVICTION_ENV: &[(&str, &str)] = &[
    ("DEVSTACK_LOG_INDEX_MAINTENANCE_INTERVAL_SECS", "2"),
    ("DEVSTACK_LOG_INDEX_MAX_AGE_SECS", "3600"),
];

#[tokio::test]
async fn daemon_evicts_old_source_logs_by_age() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start_with_env(EVICTION_ENV).await?;

    let now = time::OffsetDateTime::now_utc();
    let old_ts = ts_rfc3339(now - time::Duration::days(30));
    let recent_ts = ts_rfc3339(now - time::Duration::minutes(5));

    let source_path = project.path().join("state/evict-age.jsonl");
    std::fs::create_dir_all(source_path.parent().unwrap())?;
    std::fs::write(
        &source_path,
        format!(
            "{}{}{}",
            jsonl_line(&old_ts, "stdout", "info", "ancient-entry"),
            jsonl_line(
                &ts_rfc3339(now - time::Duration::days(29)),
                "stderr",
                "error",
                "also-ancient"
            ),
            jsonl_line(&recent_ts, "stdout", "info", "recent-entry"),
        ),
    )?;

    t.api()
        .add_source("evict-age", vec![source_path.to_string_lossy().to_string()])
        .await?;

    let before = t.api().source_logs("evict-age", &view_query(50)).await?;
    assert_eq!(before.total, 3);

    // Daemon maintenance runs every 2s with max_age=3600s in test harness.
    // Wait for at least one maintenance cycle.
    t.wait_until(
        Duration::from_secs(15),
        "old source logs to be evicted",
        || {
            let api = t.api();
            let query = view_query(50);
            async move {
                let logs = api.source_logs("evict-age", &query).await?;
                if logs.total == 1 {
                    Ok(Some(logs))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await?;

    let after = t.api().source_logs("evict-age", &view_query(50)).await?;
    assert_eq!(after.entries.len(), 1);
    assert_eq!(after.entries[0].message, "recent-entry");

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_preserves_recent_logs_across_eviction_cycles() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start_with_env(EVICTION_ENV).await?;

    let now = time::OffsetDateTime::now_utc();
    let source_path = project.path().join("state/evict-preserve.jsonl");
    std::fs::create_dir_all(source_path.parent().unwrap())?;
    std::fs::write(
        &source_path,
        format!(
            "{}{}",
            jsonl_line(
                &ts_rfc3339(now - time::Duration::minutes(10)),
                "stdout",
                "info",
                "ten-min-ago"
            ),
            jsonl_line(
                &ts_rfc3339(now - time::Duration::seconds(30)),
                "stderr",
                "error",
                "thirty-sec-ago"
            ),
        ),
    )?;

    t.api()
        .add_source(
            "evict-preserve",
            vec![source_path.to_string_lossy().to_string()],
        )
        .await?;

    let before = t
        .api()
        .source_logs("evict-preserve", &view_query(50))
        .await?;
    assert_eq!(before.total, 2);

    // Let several maintenance cycles pass — both entries are recent, both should survive.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let after = t
        .api()
        .source_logs("evict-preserve", &view_query(50))
        .await?;
    assert_eq!(after.total, 2);

    daemon.stop().await?;
    Ok(())
}

#[tokio::test]
async fn eviction_does_not_break_subsequent_source_ingestion() -> Result<()> {
    let t = TestHarness::new().await?;
    let project = t.fixture(fixtures::simple_http()).create().await?;
    let daemon = t.daemon().start_with_env(EVICTION_ENV).await?;

    let now = time::OffsetDateTime::now_utc();
    let old_path = project.path().join("state/evict-old.jsonl");
    std::fs::create_dir_all(old_path.parent().unwrap())?;
    std::fs::write(
        &old_path,
        jsonl_line(
            &ts_rfc3339(now - time::Duration::days(30)),
            "stdout",
            "info",
            "old-entry",
        ),
    )?;

    t.api()
        .add_source("evict-old", vec![old_path.to_string_lossy().to_string()])
        .await?;

    // Wait for eviction to clear old entries
    t.wait_until(
        Duration::from_secs(15),
        "old source to be evicted",
        || {
            let api = t.api();
            let query = view_query(50);
            async move {
                let logs = api.source_logs("evict-old", &query).await?;
                if logs.total == 0 {
                    Ok(Some(()))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await?;

    // Add a new source with fresh logs — should ingest cleanly
    let fresh_path = project.path().join("state/fresh.jsonl");
    std::fs::write(
        &fresh_path,
        format!(
            "{}{}",
            jsonl_line(
                &ts_rfc3339(now - time::Duration::minutes(2)),
                "stdout",
                "info",
                "fresh-alpha",
            ),
            jsonl_line(
                &ts_rfc3339(now - time::Duration::minutes(1)),
                "stderr",
                "error",
                "fresh-beta",
            ),
        ),
    )?;

    t.api()
        .add_source("fresh", vec![fresh_path.to_string_lossy().to_string()])
        .await?;

    let fresh_logs = t.api().source_logs("fresh", &view_query(50)).await?;
    assert_eq!(fresh_logs.total, 2);
    let messages: Vec<&str> = fresh_logs.entries.iter().map(|e| e.message.as_str()).collect();
    assert!(messages.contains(&"fresh-alpha"));
    assert!(messages.contains(&"fresh-beta"));

    daemon.stop().await?;
    Ok(())
}

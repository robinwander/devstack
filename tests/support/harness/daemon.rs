use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;

use anyhow::{Context, Result};
use tokio::process::Child;

use super::{DAEMON_TIMEOUT, NEXT_ID, TestHarness, tail_lines};

pub struct DaemonController {
    pub(super) harness: TestHarness,
}

impl DaemonController {
    pub async fn start(&self) -> Result<DaemonHandle> {
        self.start_with_env(&[]).await
    }

    pub async fn start_with_env(&self, extra_env: &[(&str, &str)]) -> Result<DaemonHandle> {
        let log_path = self.harness.inner.workspace.join(format!(
            "daemon-{}.log",
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        let stdout = std::fs::File::create(&log_path)?;
        let stderr = stdout.try_clone()?;

        let mut cmd = tokio::process::Command::new(&self.harness.inner.bin);
        cmd.current_dir(&self.harness.inner.workspace);
        self.harness.apply_child_env_tokio(&mut cmd);
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        cmd.arg("daemon");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::from(stdout));
        cmd.stderr(Stdio::from(stderr));

        let child = cmd.spawn().context("spawn daemon")?;
        *self
            .harness
            .inner
            .daemon_log_path
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(log_path.clone());

        let handle = DaemonHandle {
            harness: self.harness.clone(),
            child: Some(child),
            log_path,
        };
        handle
            .assert_ping()
            .await
            .with_context(|| handle.failure_context())?;
        Ok(handle)
    }
}

pub struct DaemonHandle {
    harness: TestHarness,
    child: Option<Child>,
    log_path: PathBuf,
}

impl DaemonHandle {
    pub async fn assert_ping(&self) -> Result<()> {
        self.harness
            .wait_until(DAEMON_TIMEOUT, "daemon ping", || {
                let api = self.harness.api();
                async move {
                    if api.ping().await.unwrap_or(false) {
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
            })
            .await
    }

    pub async fn stop(mut self) -> Result<()> {
        if self.harness.api().ping().await.unwrap_or(false)
            && let Ok(runs) = self.harness.api().list_runs().await
        {
            for run in runs.runs {
                let _ = self.harness.api().down(&run.run_id).await;
            }
        }

        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(())
    }

    pub async fn restart(mut self) -> Result<Self> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.harness.daemon().start().await
    }

    fn failure_context(&self) -> String {
        let log = std::fs::read_to_string(&self.log_path).unwrap_or_default();
        format!(
            "daemon failed to become healthy\nlog_path: {}\nlog_tail:\n{}",
            self.log_path.display(),
            tail_lines(&log, 80)
        )
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

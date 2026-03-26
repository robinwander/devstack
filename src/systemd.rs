use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;

#[derive(Clone, Debug)]
pub struct ExecStart {
    pub path: String,
    pub argv: Vec<String>,
    pub ignore_failure: bool,
}

#[derive(Clone, Debug)]
pub struct UnitProperties {
    pub description: String,
    pub working_directory: String,
    pub environment: Vec<String>,
    pub exec_start: ExecStart,
    pub kill_mode: String,
    pub kill_signal: i32,
    pub timeout_stop_usec: u64,
    pub send_sigkill: bool,
    pub restart: String,
    pub restart_usec: u64,
    pub start_limit_interval_usec: u64,
    pub start_limit_burst: u32,
    pub remain_after_exit: bool,
}

impl UnitProperties {
    pub fn new(
        description: String,
        working_directory: &Path,
        environment: Vec<String>,
        exec_start: ExecStart,
    ) -> Self {
        Self {
            description,
            working_directory: working_directory.to_string_lossy().to_string(),
            environment,
            exec_start,
            kill_mode: "control-group".to_string(),
            kill_signal: 2,
            timeout_stop_usec: 2_000_000,
            send_sigkill: true,
            restart: "on-failure".to_string(),
            restart_usec: 250_000,
            start_limit_interval_usec: 30_000_000,
            start_limit_burst: 20,
            remain_after_exit: false,
        }
    }

    pub fn with_restart(mut self, restart: &str) -> Self {
        self.restart = restart.to_string();
        self
    }

    pub fn with_remain_after_exit(mut self, remain_after_exit: bool) -> Self {
        self.remain_after_exit = remain_after_exit;
        self
    }
}

#[derive(Clone, Debug)]
pub struct UnitStatus {
    pub active_state: String,
    pub sub_state: String,
    pub result: Option<String>,
}

#[async_trait]
pub trait SystemdManager: Send + Sync {
    async fn start_transient_service(&self, unit_name: &str, props: UnitProperties) -> Result<()>;
    async fn stop_unit(&self, unit_name: &str) -> Result<()>;
    async fn restart_unit(&self, unit_name: &str) -> Result<()>;
    async fn kill_unit(&self, unit_name: &str, signal: i32) -> Result<()>;
    async fn unit_status(&self, unit_name: &str) -> Result<Option<UnitStatus>>;
}

#[cfg(target_os = "linux")]
use systemd_zbus::zbus::{Connection, zvariant::Value};
#[cfg(target_os = "linux")]
use systemd_zbus::{ManagerProxy, Mode, ServiceProxy, UnitProxy};

#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct RealSystemd {
    conn: Connection,
}

#[cfg(target_os = "linux")]
impl RealSystemd {
    pub async fn connect() -> Result<Self> {
        let conn = Connection::session().await.context("connect session bus")?;
        Ok(Self { conn })
    }

    async fn manager(&self) -> Result<ManagerProxy<'_>> {
        ManagerProxy::new(&self.conn)
            .await
            .context("create ManagerProxy")
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl SystemdManager for RealSystemd {
    async fn start_transient_service(&self, unit_name: &str, props: UnitProperties) -> Result<()> {
        let manager = self.manager().await?;

        let exec = vec![(
            props.exec_start.path,
            props.exec_start.argv,
            props.exec_start.ignore_failure,
        )];

        let properties: Vec<(&str, Value)> = vec![
            ("Description", Value::new(props.description)),
            ("Type", Value::new("exec")),
            ("WorkingDirectory", Value::new(props.working_directory)),
            ("Environment", Value::new(props.environment)),
            ("ExecStart", Value::new(exec)),
            ("KillMode", Value::new(props.kill_mode)),
            ("KillSignal", Value::new(props.kill_signal)),
            ("TimeoutStopUSec", Value::new(props.timeout_stop_usec)),
            ("SendSIGKILL", Value::new(props.send_sigkill)),
            ("RemainAfterExit", Value::new(props.remain_after_exit)),
            ("Restart", Value::new(props.restart)),
            ("RestartUSec", Value::new(props.restart_usec)),
            (
                "StartLimitIntervalUSec",
                Value::new(props.start_limit_interval_usec),
            ),
            ("StartLimitBurst", Value::new(props.start_limit_burst)),
        ];

        manager
            .start_transient_unit(unit_name, Mode::Replace, &properties, &[])
            .await
            .context("start transient unit")?;
        Ok(())
    }

    async fn stop_unit(&self, unit_name: &str) -> Result<()> {
        let manager = self.manager().await?;
        manager
            .stop_unit(unit_name, Mode::Replace)
            .await
            .context("stop unit")?;
        Ok(())
    }

    async fn restart_unit(&self, unit_name: &str) -> Result<()> {
        let manager = self.manager().await?;
        manager
            .restart_unit(unit_name, Mode::Replace)
            .await
            .context("restart unit")?;
        Ok(())
    }

    async fn kill_unit(&self, unit_name: &str, signal: i32) -> Result<()> {
        let manager = self.manager().await?;
        manager
            .kill_unit(unit_name, "all", signal)
            .await
            .context("kill unit")?;
        Ok(())
    }

    async fn unit_status(&self, unit_name: &str) -> Result<Option<UnitStatus>> {
        let manager = self.manager().await?;
        let path = match manager.get_unit(unit_name).await {
            Ok(path) => path,
            Err(_) => return Ok(None),
        };

        let unit = UnitProxy::builder(&self.conn)
            .path(path.clone())?
            .build()
            .await
            .context("create UnitProxy")?;
        let active_state = unit
            .active_state()
            .await
            .unwrap_or(systemd_zbus::ActiveState::Inactive);
        let sub_state = unit.sub_state().await.unwrap_or_default();
        let result = match ServiceProxy::builder(&self.conn).path(path) {
            Ok(builder) => match builder.build().await {
                Ok(proxy) => proxy.result().await.ok(),
                Err(_) => None,
            },
            Err(_) => None,
        };

        Ok(Some(UnitStatus {
            active_state: format!("{:?}", active_state).to_lowercase(),
            sub_state,
            result,
        }))
    }
}

#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::process::ExitStatus;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use tokio::process::Command;
#[cfg(unix)]
use tokio::sync::Mutex;
#[cfg(unix)]
use tokio::time::{Duration, timeout};

#[cfg(unix)]
#[derive(Clone)]
pub struct LocalSystemd {
    units: Arc<Mutex<HashMap<String, LocalUnit>>>,
}

#[cfg(unix)]
struct LocalUnit {
    props: UnitProperties,
    child: tokio::process::Child,
    pgid: Option<i32>,
    last_status: Option<ExitStatus>,
}

#[cfg(unix)]
impl Default for LocalSystemd {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl LocalSystemd {
    pub fn new() -> Self {
        let units = Arc::new(Mutex::new(HashMap::new()));
        Self::spawn_reaper(units.clone());
        Self { units }
    }

    fn spawn_reaper(units: Arc<Mutex<HashMap<String, LocalUnit>>>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                let mut guard = units.lock().await;
                let mut to_remove = Vec::new();
                for (name, unit) in guard.iter_mut() {
                    match unit.child.try_wait() {
                        Ok(Some(status)) => {
                            unit.last_status = Some(status);
                            if !unit.props.remain_after_exit {
                                to_remove.push(name.clone());
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            to_remove.push(name.clone());
                        }
                    }
                }
                for name in to_remove {
                    guard.remove(&name);
                }
            }
        });
    }

    fn spawn_child(props: &UnitProperties) -> Result<tokio::process::Child> {
        let mut cmd = Command::new(&props.exec_start.path);
        if !props.exec_start.argv.is_empty() {
            if props.exec_start.argv[0] == props.exec_start.path {
                cmd.args(&props.exec_start.argv[1..]);
            } else {
                cmd.args(&props.exec_start.argv);
            }
        }
        cmd.current_dir(&props.working_directory);
        for item in &props.environment {
            if let Some((key, value)) = item.split_once('=') {
                cmd.env(key, value);
            }
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        // SAFETY: `pre_exec` runs in the child process after `fork` and before `exec`.
        // We only call the async-signal-safe libc `setpgid(0, 0)` to put the child in its
        // own process group, and propagate any OS error back to the caller.
        unsafe {
            cmd.pre_exec(|| {
                let rc = libc::setpgid(0, 0);
                if rc != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        cmd.spawn().context("spawn local service")
    }

    fn signal_unit(unit: &LocalUnit, signal: i32) {
        if let Some(pgid) = unit.pgid {
            // SAFETY: `kill` is called with a process-group id that we created for this unit.
            // Negative pid targets the whole group by POSIX contract; errors are ignored
            // intentionally because teardown is best-effort.
            unsafe {
                let _ = libc::kill(-pgid, signal);
            }
            return;
        }
        if let Some(pid) = unit.child.id() {
            // SAFETY: `pid` comes from the running child handle. Sending a signal to this pid is
            // safe; errors are ignored because the process may have already exited.
            unsafe {
                let _ = libc::kill(pid as i32, signal);
            }
        }
    }

    async fn stop_and_reap(unit: &mut LocalUnit, signal: i32) -> Result<()> {
        Self::signal_unit(unit, signal);

        match timeout(Duration::from_secs(3), unit.child.wait()).await {
            Ok(Ok(status)) => {
                unit.last_status = Some(status);
                Ok(())
            }
            Ok(Err(err)) => Err(err).context("wait for local service"),
            Err(_) => {
                Self::signal_unit(unit, libc::SIGKILL);
                if let Ok(Ok(status)) = timeout(Duration::from_secs(1), unit.child.wait()).await {
                    unit.last_status = Some(status);
                }
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
#[async_trait]
impl SystemdManager for LocalSystemd {
    async fn start_transient_service(&self, unit_name: &str, props: UnitProperties) -> Result<()> {
        let existing = {
            let mut units = self.units.lock().await;
            units.remove(unit_name)
        };
        if let Some(mut unit) = existing {
            Self::stop_and_reap(&mut unit, libc::SIGTERM).await?;
        }

        let child = Self::spawn_child(&props)?;
        let pgid = child.id().map(|pid| pid as i32);

        let mut units = self.units.lock().await;
        units.insert(
            unit_name.to_string(),
            LocalUnit {
                props,
                child,
                pgid,
                last_status: None,
            },
        );
        Ok(())
    }

    async fn stop_unit(&self, unit_name: &str) -> Result<()> {
        let unit = {
            let mut units = self.units.lock().await;
            units.remove(unit_name)
        };
        if let Some(mut unit) = unit {
            Self::stop_and_reap(&mut unit, libc::SIGTERM).await?;
        }
        Ok(())
    }

    async fn restart_unit(&self, unit_name: &str) -> Result<()> {
        let previous = {
            let mut units = self.units.lock().await;
            units.remove(unit_name)
        };

        let Some(mut unit) = previous else {
            return Ok(());
        };

        let props = unit.props.clone();
        Self::stop_and_reap(&mut unit, libc::SIGTERM).await?;

        let child = Self::spawn_child(&props)?;
        let pgid = child.id().map(|pid| pid as i32);
        let mut units = self.units.lock().await;
        units.insert(
            unit_name.to_string(),
            LocalUnit {
                props,
                child,
                pgid,
                last_status: None,
            },
        );
        Ok(())
    }

    async fn kill_unit(&self, unit_name: &str, signal: i32) -> Result<()> {
        let unit = {
            let mut units = self.units.lock().await;
            units.remove(unit_name)
        };
        if let Some(mut unit) = unit {
            Self::stop_and_reap(&mut unit, signal).await?;
        }
        Ok(())
    }

    async fn unit_status(&self, unit_name: &str) -> Result<Option<UnitStatus>> {
        let mut units = self.units.lock().await;
        let Some(unit) = units.get_mut(unit_name) else {
            return Ok(None);
        };

        match unit.child.try_wait()? {
            Some(status) => {
                unit.last_status = Some(status);
                let result = if status.success() {
                    Some("success".to_string())
                } else {
                    Some("exit-code".to_string())
                };

                if unit.props.remain_after_exit {
                    return Ok(Some(UnitStatus {
                        active_state: "active".to_string(),
                        sub_state: "exited".to_string(),
                        result,
                    }));
                }

                Ok(Some(UnitStatus {
                    active_state: "inactive".to_string(),
                    sub_state: "exited".to_string(),
                    result,
                }))
            }
            None => Ok(Some(UnitStatus {
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                result: None,
            })),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_systemd_keeps_exited_unit_when_remain_after_exit_enabled() {
        let systemd = LocalSystemd::new();
        let unit_name = format!(
            "devstack-local-remain-after-exit-{}.service",
            std::process::id()
        );
        let props = UnitProperties::new(
            "test".to_string(),
            Path::new("/"),
            vec![],
            ExecStart {
                path: "/usr/bin/true".to_string(),
                argv: vec!["/usr/bin/true".to_string()],
                ignore_failure: false,
            },
        )
        .with_restart("no")
        .with_remain_after_exit(true);

        systemd
            .start_transient_service(&unit_name, props)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;

        let status = systemd.unit_status(&unit_name).await.unwrap().unwrap();
        assert_eq!(status.active_state, "active");
        assert_eq!(status.sub_state, "exited");
        assert_eq!(status.result.as_deref(), Some("success"));

        systemd.stop_unit(&unit_name).await.unwrap();
    }
}

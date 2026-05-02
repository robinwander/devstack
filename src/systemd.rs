use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ExecStart {
    pub path: String,
    pub argv: Vec<String>,
    pub ignore_failure: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
use std::collections::{BTreeMap, HashMap};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::ExitStatus;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use tokio::process::{Child, Command};
#[cfg(unix)]
use tokio::sync::Mutex;
#[cfg(unix)]
use tokio::time::{Duration, timeout};

#[cfg(unix)]
#[derive(Clone)]
pub struct LocalSystemd {
    units: Arc<Mutex<HashMap<String, LocalUnit>>>,
    registry_path: Arc<PathBuf>,
}

#[cfg(unix)]
struct LocalUnit {
    props: UnitProperties,
    child: Option<Child>,
    pgid: Option<i32>,
    last_status: Option<LocalExitStatus>,
}

#[cfg(unix)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct LocalExitStatus {
    success: bool,
    code: Option<i32>,
}

#[cfg(unix)]
impl LocalExitStatus {
    fn result(&self) -> String {
        if self.success {
            "success".to_string()
        } else {
            "exit-code".to_string()
        }
    }
}

#[cfg(unix)]
impl From<ExitStatus> for LocalExitStatus {
    fn from(status: ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code(),
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedLocalUnit {
    props: UnitProperties,
    pgid: Option<i32>,
    last_status: Option<LocalExitStatus>,
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
        let registry_path = crate::paths::local_units_path()
            .unwrap_or_else(|_| std::env::temp_dir().join("devstack-local-units.json"));
        Self::with_registry_path(registry_path)
    }

    #[cfg(test)]
    fn new_with_registry_path(registry_path: PathBuf) -> Self {
        Self::with_registry_path(registry_path)
    }

    fn with_registry_path(registry_path: PathBuf) -> Self {
        let mut loaded = Self::load_units(&registry_path);
        loaded.retain(|_, unit| Self::should_keep_loaded_unit(unit));
        Self::persist_units_sync(&registry_path, &loaded);

        let units = Arc::new(Mutex::new(loaded));
        let registry_path = Arc::new(registry_path);
        Self::spawn_reaper(units.clone(), registry_path.clone());
        Self {
            units,
            registry_path,
        }
    }

    fn should_keep_loaded_unit(unit: &LocalUnit) -> bool {
        if unit.props.remain_after_exit && unit.last_status.is_some() {
            return true;
        }
        unit.pgid.is_some_and(process_group_exists)
    }

    fn load_units(registry_path: &Path) -> HashMap<String, LocalUnit> {
        let Ok(raw) = std::fs::read(registry_path) else {
            return HashMap::new();
        };
        let Ok(persisted) = serde_json::from_slice::<BTreeMap<String, PersistedLocalUnit>>(&raw)
        else {
            return HashMap::new();
        };
        persisted
            .into_iter()
            .map(|(name, unit)| {
                (
                    name,
                    LocalUnit {
                        props: unit.props,
                        child: None,
                        pgid: unit.pgid,
                        last_status: unit.last_status,
                    },
                )
            })
            .collect()
    }

    fn persist_units_sync(registry_path: &Path, units: &HashMap<String, LocalUnit>) {
        let persisted = units
            .iter()
            .map(|(name, unit)| {
                (
                    name.clone(),
                    PersistedLocalUnit {
                        props: unit.props.clone(),
                        pgid: unit.pgid,
                        last_status: unit.last_status.clone(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        if let Some(parent) = registry_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp_path = registry_path.with_extension("json.tmp");
        if persisted.is_empty() {
            let _ = std::fs::remove_file(registry_path);
            let _ = std::fs::remove_file(tmp_path);
            return;
        }
        let Ok(bytes) = serde_json::to_vec_pretty(&persisted) else {
            return;
        };
        if std::fs::write(&tmp_path, bytes).is_ok() {
            let _ = std::fs::rename(tmp_path, registry_path);
        }
    }

    fn persist_units(&self, units: &HashMap<String, LocalUnit>) {
        Self::persist_units_sync(&self.registry_path, units);
    }

    pub fn cleanup_registry(registry_path: &Path) {
        let units = Self::load_units(registry_path);
        for unit in units.values() {
            Self::signal_unit(unit, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline
            && units
                .values()
                .any(|unit| unit.pgid.is_some_and(|pgid| process_group_exists(pgid)))
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for unit in units.values() {
            if unit.pgid.is_some_and(|pgid| process_group_exists(pgid)) {
                Self::signal_unit(unit, libc::SIGKILL);
            }
        }
        let _ = std::fs::remove_file(registry_path);
    }

    fn spawn_reaper(units: Arc<Mutex<HashMap<String, LocalUnit>>>, registry_path: Arc<PathBuf>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                let mut guard = units.lock().await;
                let mut to_remove = Vec::new();
                let mut changed = false;
                for (name, unit) in guard.iter_mut() {
                    if let Some(child) = unit.child.as_mut() {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                unit.last_status = Some(status.into());
                                unit.child = None;
                                changed = true;
                                if !unit.props.remain_after_exit {
                                    to_remove.push(name.clone());
                                }
                            }
                            Ok(None) => {}
                            Err(_) => {
                                to_remove.push(name.clone());
                            }
                        }
                        continue;
                    }

                    if unit.props.remain_after_exit && unit.last_status.is_some() {
                        continue;
                    }
                    if unit.pgid.is_some_and(process_group_exists) {
                        continue;
                    }
                    to_remove.push(name.clone());
                }
                if !to_remove.is_empty() {
                    changed = true;
                }
                for name in to_remove {
                    guard.remove(&name);
                }
                if changed {
                    Self::persist_units_sync(&registry_path, &guard);
                }
            }
        });
    }

    fn spawn_child(props: &UnitProperties) -> Result<Child> {
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
        if let Some(pid) = unit.child.as_ref().and_then(Child::id) {
            // SAFETY: `pid` comes from the running child handle. Sending a signal to this pid is
            // safe; errors are ignored because the process may have already exited.
            unsafe {
                let _ = libc::kill(pid as i32, signal);
            }
        }
    }

    async fn stop_and_reap(unit: &mut LocalUnit, signal: i32) -> Result<()> {
        Self::signal_unit(unit, signal);
        let pgid = unit.pgid;

        if let Some(child) = unit.child.as_mut() {
            match timeout(Duration::from_secs(3), child.wait()).await {
                Ok(Ok(status)) => {
                    unit.last_status = Some(status.into());
                    return Ok(());
                }
                Ok(Err(err)) => return Err(err).context("wait for local service"),
                Err(_) => {
                    if let Some(pgid) = pgid {
                        // SAFETY: negative pid targets the process group created for this unit.
                        unsafe {
                            let _ = libc::kill(-pgid, libc::SIGKILL);
                        }
                    }
                    if let Ok(Ok(status)) = timeout(Duration::from_secs(1), child.wait()).await {
                        unit.last_status = Some(status.into());
                    }
                    return Ok(());
                }
            }
        }

        let Some(pgid) = pgid else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        if wait_for_process_group_exit(pgid, deadline).await {
            return Ok(());
        }
        Self::signal_unit(unit, libc::SIGKILL);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let _ = wait_for_process_group_exit(pgid, deadline).await;
        Ok(())
    }
}

#[cfg(unix)]
fn process_group_exists(pgid: i32) -> bool {
    // SAFETY: `kill(-pgid, 0)` performs a process-group existence probe without sending a signal.
    unsafe {
        if libc::kill(-pgid, 0) == 0 {
            return true;
        }
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(unix)]
async fn wait_for_process_group_exit(pgid: i32, deadline: tokio::time::Instant) -> bool {
    while tokio::time::Instant::now() < deadline {
        if !process_group_exists(pgid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    !process_group_exists(pgid)
}

#[cfg(unix)]
#[async_trait]
impl SystemdManager for LocalSystemd {
    async fn start_transient_service(&self, unit_name: &str, props: UnitProperties) -> Result<()> {
        let existing = {
            let mut units = self.units.lock().await;
            let existing = units.remove(unit_name);
            self.persist_units(&units);
            existing
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
                child: Some(child),
                pgid,
                last_status: None,
            },
        );
        self.persist_units(&units);
        Ok(())
    }

    async fn stop_unit(&self, unit_name: &str) -> Result<()> {
        let unit = {
            let mut units = self.units.lock().await;
            let unit = units.remove(unit_name);
            self.persist_units(&units);
            unit
        };
        if let Some(mut unit) = unit {
            Self::stop_and_reap(&mut unit, libc::SIGTERM).await?;
        }
        Ok(())
    }

    async fn restart_unit(&self, unit_name: &str) -> Result<()> {
        let previous = {
            let mut units = self.units.lock().await;
            let previous = units.remove(unit_name);
            self.persist_units(&units);
            previous
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
                child: Some(child),
                pgid,
                last_status: None,
            },
        );
        self.persist_units(&units);
        Ok(())
    }

    async fn kill_unit(&self, unit_name: &str, signal: i32) -> Result<()> {
        let unit = {
            let mut units = self.units.lock().await;
            let unit = units.remove(unit_name);
            self.persist_units(&units);
            unit
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

        if let Some(child) = unit.child.as_mut() {
            match child.try_wait()? {
                Some(status) => {
                    unit.last_status = Some(status.into());
                    unit.child = None;
                    let result = unit.last_status.as_ref().map(LocalExitStatus::result);
                    let active_state = if unit.props.remain_after_exit {
                        "active"
                    } else {
                        "inactive"
                    }
                    .to_string();
                    let response = UnitStatus {
                        active_state,
                        sub_state: "exited".to_string(),
                        result,
                    };
                    self.persist_units(&units);
                    return Ok(Some(response));
                }
                None => {
                    return Ok(Some(UnitStatus {
                        active_state: "active".to_string(),
                        sub_state: "running".to_string(),
                        result: None,
                    }));
                }
            }
        }

        if unit.props.remain_after_exit && unit.last_status.is_some() {
            return Ok(Some(UnitStatus {
                active_state: "active".to_string(),
                sub_state: "exited".to_string(),
                result: unit.last_status.as_ref().map(LocalExitStatus::result),
            }));
        }

        if unit.pgid.is_some_and(process_group_exists) {
            return Ok(Some(UnitStatus {
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                result: None,
            }));
        }

        let result = unit.last_status.as_ref().map(LocalExitStatus::result);
        units.remove(unit_name);
        self.persist_units(&units);
        Ok(Some(UnitStatus {
            active_state: "inactive".to_string(),
            sub_state: "exited".to_string(),
            result,
        }))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_systemd_keeps_exited_unit_when_remain_after_exit_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let systemd = LocalSystemd::new_with_registry_path(dir.path().join("local-units.json"));
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

    #[tokio::test]
    async fn local_systemd_restores_persisted_unit_after_manager_restart() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = dir.path().join("local-units.json");
        let unit_name = format!("devstack-local-restore-{}.service", std::process::id());
        let props = UnitProperties::new(
            "test".to_string(),
            Path::new("/"),
            vec![],
            ExecStart {
                path: "/bin/sh".to_string(),
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "trap 'exit 0' TERM; while true; do sleep 1; done".to_string(),
                ],
                ignore_failure: false,
            },
        )
        .with_restart("no");

        let systemd = LocalSystemd::new_with_registry_path(registry_path.clone());
        systemd
            .start_transient_service(&unit_name, props)
            .await
            .unwrap();
        assert!(registry_path.exists());
        drop(systemd);

        let restored = LocalSystemd::new_with_registry_path(registry_path.clone());
        let status = restored.unit_status(&unit_name).await.unwrap().unwrap();
        assert_eq!(status.active_state, "active");
        assert_eq!(status.sub_state, "running");

        restored.stop_unit(&unit_name).await.unwrap();
        assert!(!registry_path.exists());
    }
}

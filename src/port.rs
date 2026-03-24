use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpListener;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::config::{PortConfig, ServiceConfig};

#[derive(Debug, Default, Serialize, Deserialize)]
struct PortReservationRegistry {
    reservations: BTreeMap<u16, PortReservationEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortReservationEntry {
    owner: String,
    daemon_pid: u32,
}

pub fn allocate_ports(
    services: &BTreeMap<String, ServiceConfig>,
    owner_for: impl Fn(&str) -> String,
) -> Result<BTreeMap<String, Option<u16>>> {
    allocate_ports_in_registry(&registry_path(), services, owner_for)
}

pub fn reserve_port(port: u16, owner: &str) -> Result<()> {
    reserve_port_in_registry(&registry_path(), port, owner)
}

pub fn reserve_available_port(port: u16, owner: &str) -> Result<()> {
    reserve_available_port_in_registry(&registry_path(), port, owner)
}

pub fn release_port(port: u16, owner: &str) -> Result<()> {
    release_port_in_registry(&registry_path(), port, owner)
}

fn allocate_ports_in_registry(
    path: &Path,
    services: &BTreeMap<String, ServiceConfig>,
    owner_for: impl Fn(&str) -> String,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut allocated = BTreeMap::new();

    for (name, svc) in services {
        let owner = owner_for(name);
        let port = match &svc.port {
            Some(config) if config.is_none() => None,
            Some(PortConfig::Fixed(value)) => {
                reserve_available_port_in_registry(path, *value, &owner)?;
                Some(*value)
            }
            Some(PortConfig::None(_)) => None,
            None => Some(allocate_ephemeral_port_in_registry(path, &owner)?),
        };
        allocated.insert(name.clone(), port);
    }

    Ok(allocated)
}

fn release_port_in_registry(path: &Path, port: u16, owner: &str) -> Result<()> {
    with_registry(path, |registry| {
        match registry.reservations.get(&port) {
            Some(entry) if entry.owner == owner => {
                registry.reservations.remove(&port);
            }
            Some(_) => {
                return Err(anyhow!(
                    "port {port} is reserved by another devstack service"
                ));
            }
            None => {}
        }
        Ok(())
    })
}

fn allocate_ephemeral_port_in_registry(path: &Path, owner: &str) -> Result<u16> {
    with_registry(path, |registry| {
        loop {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let port = listener.local_addr()?.port();
            drop(listener);

            if registry.reservations.contains_key(&port) {
                continue;
            }

            registry.reservations.insert(
                port,
                PortReservationEntry {
                    owner: owner.to_string(),
                    daemon_pid: std::process::id(),
                },
            );
            return Ok(port);
        }
    })
}

fn reserve_port_in_registry(path: &Path, port: u16, owner: &str) -> Result<()> {
    reserve_specific_port_in_registry(path, port, owner, false)
}

fn reserve_available_port_in_registry(path: &Path, port: u16, owner: &str) -> Result<()> {
    reserve_specific_port_in_registry(path, port, owner, true)
}

fn reserve_specific_port_in_registry(
    path: &Path,
    port: u16,
    owner: &str,
    require_available: bool,
) -> Result<()> {
    with_registry(path, |registry| {
        match registry.reservations.get(&port) {
            Some(entry) if entry.owner == owner => return Ok(()),
            Some(_) => {
                return Err(anyhow!(
                    "port {port} is reserved by another devstack service"
                ));
            }
            None => {}
        }

        if require_available {
            ensure_available(port)?;
        }

        registry.reservations.insert(
            port,
            PortReservationEntry {
                owner: owner.to_string(),
                daemon_pid: std::process::id(),
            },
        );
        Ok(())
    })
}

fn with_registry<T>(
    path: &Path,
    action: impl FnOnce(&mut PortReservationRegistry) -> Result<T>,
) -> Result<T> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open port reservation registry {}", path.display()))?;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result != 0 {
        return Err(anyhow!(std::io::Error::last_os_error()));
    }

    let outcome = (|| {
        let mut registry = read_registry(&mut file)?;
        registry.cleanup_stale();
        let output = action(&mut registry)?;
        write_registry(&mut file, &registry)?;
        Ok(output)
    })();

    let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    outcome
}

fn read_registry(file: &mut std::fs::File) -> Result<PortReservationRegistry> {
    file.seek(SeekFrom::Start(0))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    if contents.trim().is_empty() {
        return Ok(PortReservationRegistry::default());
    }
    serde_json::from_str(&contents).context("parse port reservation registry")
}

fn write_registry(file: &mut std::fs::File, registry: &PortReservationRegistry) -> Result<()> {
    let contents = serde_json::to_vec_pretty(registry)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&contents)?;
    file.sync_data()?;
    Ok(())
}

fn registry_path() -> PathBuf {
    std::env::temp_dir().join("devstack-port-reservations.json")
}

impl PortReservationRegistry {
    fn cleanup_stale(&mut self) {
        self.reservations
            .retain(|_, entry| is_pid_alive(entry.daemon_pid));
    }
}

fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }

    let err = std::io::Error::last_os_error();
    err.raw_os_error() == Some(libc::EPERM)
}

pub fn ensure_available(port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    TcpListener::bind(addr)
        .map(drop)
        .with_context(|| format!("port {port} is unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    use crate::config::ServiceConfig;

    fn service_with_port(port: Option<PortConfig>) -> ServiceConfig {
        ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port,
            readiness: None,
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        }
    }

    #[test]
    fn allocate_ports_respects_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ports.json");
        let mut services = BTreeMap::new();
        services.insert(
            "a".to_string(),
            service_with_port(Some(PortConfig::None("none".to_string()))),
        );
        let ports =
            allocate_ports_in_registry(&path, &services, |name| format!("owner:{name}")).unwrap();
        assert_eq!(ports.get("a").cloned().unwrap(), None);
    }

    #[test]
    fn allocate_ports_allocates_ephemeral() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ports.json");
        let mut services = BTreeMap::new();
        services.insert("a".to_string(), service_with_port(None));
        let ports =
            allocate_ports_in_registry(&path, &services, |name| format!("owner:{name}")).unwrap();
        let port = ports.get("a").unwrap().unwrap();
        assert!(port > 0);
        release_port_in_registry(&path, port, "owner:a").unwrap();
    }

    #[test]
    fn reserve_available_port_rejects_live_reservation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ports.json");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let err = reserve_available_port_in_registry(&path, port, "owner:test").unwrap_err();
        assert!(err.to_string().contains("port"));
    }

    #[test]
    fn registry_cleanup_drops_dead_pids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ports.json");
        with_registry(&path, |registry| {
            registry.reservations.insert(
                43210,
                PortReservationEntry {
                    owner: "stale".to_string(),
                    daemon_pid: u32::MAX,
                },
            );
            Ok(())
        })
        .unwrap();

        with_registry(&path, |registry| {
            assert!(registry.reservations.is_empty());
            Ok(())
        })
        .unwrap();
    }
}

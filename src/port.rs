use std::collections::{BTreeMap, HashSet};
use std::net::TcpListener;

use anyhow::{Context, Result, anyhow};

use crate::config::{PortConfig, ServiceConfig};

pub fn allocate_ports(
    services: &BTreeMap<String, ServiceConfig>,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut allocated = BTreeMap::new();
    let mut used = HashSet::new();

    for (name, svc) in services {
        let port = match &svc.port {
            Some(config) if config.is_none() => None,
            Some(PortConfig::Fixed(value)) => {
                ensure_available(*value)?;
                Some(*value)
            }
            Some(PortConfig::None(_)) => None,
            None => Some(allocate_ephemeral(&mut used)?),
        };
        if let Some(port) = port {
            if used.contains(&port) {
                return Err(anyhow!("duplicate allocated port {port}"));
            }
            used.insert(port);
        }
        allocated.insert(name.clone(), port);
    }

    Ok(allocated)
}

fn allocate_ephemeral(used: &mut HashSet<u16>) -> Result<u16> {
    loop {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        if !used.contains(&port) {
            return Ok(port);
        }
    }
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
    use std::collections::BTreeMap;

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
        let mut services = BTreeMap::new();
        services.insert(
            "a".to_string(),
            service_with_port(Some(PortConfig::None("none".to_string()))),
        );
        let ports = allocate_ports(&services).unwrap();
        assert_eq!(ports.get("a").cloned().unwrap(), None);
    }

    #[test]
    fn allocate_ports_allocates_ephemeral() {
        let mut services = BTreeMap::new();
        services.insert("a".to_string(), service_with_port(None));
        let ports = allocate_ports(&services).unwrap();
        let port = ports.get("a").unwrap().unwrap();
        assert!(port > 0);
    }
}

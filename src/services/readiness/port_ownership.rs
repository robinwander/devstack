use anyhow::{Result, anyhow};
#[cfg(target_os = "linux")]
use std::collections::HashSet;

use super::model::ReadinessContext;

pub struct PortBindingInfo {
    pub probe_supported: bool,
    pub has_listener: bool,
    pub listening_pids: Vec<u32>,
    pub owned_by_unit: Option<bool>,
}

pub async fn verify_port_binding(port: u16, ctx: &ReadinessContext) -> Result<()> {
    // Only verify port binding on Linux for now
    #[cfg(target_os = "linux")]
    {
        let info = linux_port_binding_info(port, ctx.unit_name.as_deref())?;
        if !info.has_listener {
            return Err(anyhow!(
                "port {} is not bound by any process",
                port
            ));
        }

        if let Some(unit_name) = &ctx.unit_name {
            if let Some(false) = info.owned_by_unit {
                return Err(anyhow!(
                    "port {} is bound but not by unit '{}' (bound by PIDs: {:?})",
                    port,
                    unit_name,
                    info.listening_pids
                ));
            }
        }

        return Ok(());
    }

    #[cfg(not(target_os = "linux"))]
    {
        // For non-Linux platforms, just check if port is bound without ownership
        let _ = ctx;
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(_) => Ok(()),
            Err(_) => Err(anyhow!("port {} is not bound", port)),
        }
    }
}



pub async fn port_binding_info(port: u16, unit_name: Option<&str>) -> Result<PortBindingInfo> {
    #[cfg(target_os = "linux")]
    {
        linux_port_binding_info_impl(port, unit_name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (port, unit_name);
        Ok(PortBindingInfo {
            probe_supported: false,
            has_listener: false,
            listening_pids: vec![],
            owned_by_unit: None,
        })
    }
}

#[cfg(target_os = "linux")]
pub fn linux_port_binding_info(port: u16, unit_name: Option<&str>) -> Result<PortBindingInfo> {
    linux_port_binding_info_impl(port, unit_name)
}

#[cfg(target_os = "linux")]
fn linux_port_binding_info_impl(port: u16, unit_name: Option<&str>) -> Result<PortBindingInfo> {
    // Try ss first (faster and more reliable)
    if let Some((bound, pids)) = linux_port_binding_info_from_ss(port) {
        let owned_by_unit = if let Some(unit_name) = unit_name {
            let control_group = linux_unit_control_group(unit_name);
            Some(pids.iter().any(|&pid| {
                pid_in_unit_cgroup(pid, unit_name, control_group.as_deref())
            }))
        } else {
            None
        };

        return Ok(PortBindingInfo {
            probe_supported: true,
            has_listener: bound,
            listening_pids: pids,
            owned_by_unit,
        });
    }

    // Fallback to /proc parsing
    let inodes = linux_listen_inodes_for_port(port)?;
    let bound = !inodes.is_empty();
    let pids = linux_pids_for_inodes(&inodes);

    let owned_by_unit = if let Some(unit_name) = unit_name {
        let control_group = linux_unit_control_group(unit_name);
        Some(pids.iter().any(|&pid| {
            pid_in_unit_cgroup(pid, unit_name, control_group.as_deref())
        }))
    } else {
        None
    };

    Ok(PortBindingInfo {
        probe_supported: true,
        has_listener: bound,
        listening_pids: pids,
        owned_by_unit,
    })
}

#[cfg(target_os = "linux")]
fn linux_listen_inodes_for_port(port: u16) -> Result<HashSet<u64>> {
    let mut inodes = HashSet::new();
    linux_collect_inodes_from_proc_net("/proc/net/tcp", port, &mut inodes);
    linux_collect_inodes_from_proc_net("/proc/net/tcp6", port, &mut inodes);
    Ok(inodes)
}

#[cfg(target_os = "linux")]
fn linux_collect_inodes_from_proc_net(path: &str, port: u16, into: &mut HashSet<u64>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    for line in content.lines().skip(1) {
        // Format: sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }

        let local_address = fields[1];
        let state = fields[3];

        // Check if this is a listening socket (state 0A = TCP_LISTEN)
        if state != "0A" {
            continue;
        }

        // Parse local port from address (format: IP:PORT in hex)
        if let Some((_ip, port_hex)) = local_address.split_once(':') {
            if let Ok(parsed_port) = u16::from_str_radix(port_hex, 16) {
                if parsed_port == port {
                    if let Ok(inode) = fields[9].parse::<u64>() {
                        into.insert(inode);
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_pids_for_inodes(inodes: &HashSet<u64>) -> Vec<u32> {
    let mut pids = Vec::new();

    let Ok(proc_entries) = std::fs::read_dir("/proc") else {
        return pids;
    };

    for entry in proc_entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };

        let fd_dir = format!("/proc/{}/fd", pid);
        let Ok(fd_entries) = std::fs::read_dir(fd_dir) else {
            continue;
        };

        for fd_entry in fd_entries.flatten() {
            let Ok(link_target) = std::fs::read_link(fd_entry.path()) else {
                continue;
            };

            if let Some(target_str) = link_target.to_str() {
                if target_str.starts_with("socket:[") && target_str.ends_with(']') {
                    let inode_str = &target_str[8..target_str.len() - 1];
                    if let Ok(inode) = inode_str.parse::<u64>() {
                        if inodes.contains(&inode) {
                            pids.push(pid);
                            break; // Found one socket for this PID, no need to check more
                        }
                    }
                }
            }
        }
    }

    pids
}

#[cfg(target_os = "linux")]
fn linux_port_binding_info_from_ss(port: u16) -> Option<(bool, Vec<u32>)> {
    use std::process::Command;

    let output = Command::new("ss")
        .args(&["-tlnp", &format!("sport = :{}", port)])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(parse_ss_output_for_pids(&stdout))
}

#[cfg(target_os = "linux")]
fn parse_ss_output_for_pids(output: &str) -> (bool, Vec<u32>) {
    let mut pids = Vec::new();
    let mut found_any = false;

    for line in output.lines().skip(1) {
        // ss output format: State Recv-Q Send-Q Local Address:Port Peer Address:Port Process
        if line.trim().is_empty() {
            continue;
        }

        found_any = true;

        // Look for process info in the last field
        // Format is usually like: users:(("process_name",pid=12345,fd=3))
        if let Some(process_info) = line.split_whitespace().last() {
            if process_info.contains("pid=") {
                for part in process_info.split(',') {
                    if let Some(pid_part) = part.strip_prefix("pid=") {
                        if let Ok(pid) = pid_part.parse::<u32>() {
                            pids.push(pid);
                        }
                    }
                }
            }
        }
    }

    (found_any, pids)
}

#[cfg(target_os = "linux")]
fn linux_unit_control_group(unit_name: &str) -> Option<String> {
    use std::process::Command;

    let output = Command::new("systemctl")
        .args(&["show", "-p", "ControlGroup", unit_name])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    for line in stdout.lines() {
        if let Some(cgroup) = line.strip_prefix("ControlGroup=") {
            if !cgroup.is_empty() {
                return Some(cgroup.to_string());
            }
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn pid_in_unit_cgroup(pid: u32, unit_name: &str, control_group: Option<&str>) -> bool {
    let cgroup_path = format!("/proc/{}/cgroup", pid);
    let Ok(content) = std::fs::read_to_string(&cgroup_path) else {
        return false;
    };

    if cgroup_content_mentions_unit(&content, unit_name) {
        return true;
    }

    if let Some(target_path) = control_group {
        if cgroup_content_has_path(&content, target_path) {
            return true;
        }
    }

    false
}

#[cfg(target_os = "linux")]
fn cgroup_content_mentions_unit(content: &str, unit_name: &str) -> bool {
    content.contains(unit_name)
}

#[cfg(target_os = "linux")]
fn cgroup_content_has_path(content: &str, target: &str) -> bool {
    for line in content.lines() {
        if let Some(path) = cgroup_path_from_line(line) {
            if path == target {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn cgroup_path_from_line(line: &str) -> Option<&str> {
    // cgroup format: hierarchy-ID:controller-list:cgroup-path
    let parts: Vec<&str> = line.splitn(3, ':').collect();
    if parts.len() == 3 {
        Some(parts[2])
    } else {
        None
    }
}
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::infra::ipc::UnixDaemonClient;

use super::auto_share::{AutoShareLevel, configure_auto_share};
use super::pty_proxy::{AgentSessionClient, ProxyIo, UnixAgentSessionClient, run_proxy};

#[derive(Clone, Debug)]
pub struct AgentCommandArgs {
    pub auto_share: Option<String>,
    pub no_auto_share: bool,
    pub watch: Vec<String>,
    pub run_id: Option<String>,
    pub command: Vec<String>,
}

pub async fn run(args: AgentCommandArgs) -> Result<i32> {
    let AgentCommandArgs {
        auto_share,
        no_auto_share,
        watch,
        run_id,
        command,
    } = args;

    let auto_share_level = resolve_auto_share(auto_share.as_deref(), no_auto_share)?;
    let daemon = UnixDaemonClient::default();
    let auto_share_config = configure_auto_share(auto_share_level, run_id, watch, &daemon).await;
    let agent_id = generate_agent_id();
    let session_client: Arc<dyn AgentSessionClient> =
        Arc::new(UnixAgentSessionClient::new(daemon.clone()));

    run_proxy(
        command,
        agent_id,
        session_client,
        ProxyIo::stdio(),
        auto_share_config.map(|config| (config, daemon)),
    )
    .await
}

fn resolve_auto_share(
    auto_share: Option<&str>,
    no_auto_share: bool,
) -> Result<Option<AutoShareLevel>> {
    if no_auto_share {
        return Ok(None);
    }

    match auto_share {
        Some("error") => Ok(Some(AutoShareLevel::Error)),
        Some("warn") => Ok(Some(AutoShareLevel::Warn)),
        Some(other) => Err(anyhow!("invalid auto-share level: {other}")),
        None => Ok(None),
    }
}

fn generate_agent_id() -> String {
    let mut rng = rand::rng();
    let mut suffix = String::new();
    for _ in 0..16 {
        suffix.push_str(&format!("{:x}", rand::Rng::random_range(&mut rng, 0..16)));
    }
    format!("agent-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_share_off_by_default() {
        assert_eq!(resolve_auto_share(None, false).unwrap(), None);
    }

    #[test]
    fn no_auto_share_flag_disables_monitoring() {
        assert_eq!(resolve_auto_share(None, true).unwrap(), None);
    }

    #[test]
    fn auto_share_flag_enables_monitoring() {
        assert_eq!(
            resolve_auto_share(Some("error"), false).unwrap(),
            Some(AutoShareLevel::Error)
        );
        assert_eq!(
            resolve_auto_share(Some("warn"), false).unwrap(),
            Some(AutoShareLevel::Warn)
        );
    }
}

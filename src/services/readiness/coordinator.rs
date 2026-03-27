use anyhow::{Result, anyhow};
use std::time::Duration;
use tokio::time::{Instant, sleep};

use crate::model::{ReadinessKind, ReadinessSpec};

use super::model::ReadinessContext;
use super::probes::{
    cmd_ready, http_ready, tcp_ready, wait_for_delay, wait_for_exit_success, wait_for_log_regex,
};

pub async fn wait_for_ready(spec: &ReadinessSpec, ctx: &ReadinessContext) -> Result<()> {
    if let ReadinessKind::Delay { duration } = &spec.kind {
        return wait_for_delay(*duration, spec.timeout).await;
    }
    if let ReadinessKind::Exit = &spec.kind {
        return wait_for_exit_success(ctx, spec.timeout).await;
    }
    if let ReadinessKind::LogRegex { pattern } = &spec.kind {
        return wait_for_log_regex(&ctx.log_path, pattern, spec.timeout).await;
    }

    let deadline = Instant::now() + spec.timeout;
    let mut last_err: Option<anyhow::Error> = None;
    loop {
        if let Some(reason) = readiness_process_failure(ctx).await? {
            return Err(anyhow!(reason));
        }

        match check_ready_once(spec, ctx).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) => {
                last_err = Some(err);
            }
        }

        if let Some(reason) = readiness_process_failure(ctx).await? {
            return Err(anyhow!(reason));
        }

        if Instant::now() >= deadline {
            if let Some(err) = last_err {
                return Err(anyhow!("readiness timed out: {err}"));
            } else {
                return Err(anyhow!("readiness timed out"));
            }
        }

        sleep(Duration::from_millis(100)).await;
    }
}

pub async fn check_ready_once(spec: &ReadinessSpec, ctx: &ReadinessContext) -> Result<bool> {
    match &spec.kind {
        ReadinessKind::Tcp => {
            if let Some(port) = ctx.port {
                Ok(tcp_ready(port).await)
            } else {
                Err(anyhow!("TCP readiness check requires port"))
            }
        }
        ReadinessKind::Http {
            path,
            expect_min,
            expect_max,
        } => {
            if let Some(port) = ctx.port {
                http_ready(port, path, *expect_min, *expect_max).await
            } else {
                Err(anyhow!("HTTP readiness check requires port"))
            }
        }
        ReadinessKind::LogRegex { pattern } => {
            use super::probes::log_regex_ready;
            log_regex_ready(&ctx.log_path, pattern)
        }
        ReadinessKind::Cmd { command } => cmd_ready(command, ctx).await,
        ReadinessKind::Exit => {
            use super::probes::exit_ready_once;
            exit_ready_once(ctx).await
        }
        ReadinessKind::Delay { .. } => {
            // Delay checks are handled in wait_for_ready
            Ok(false)
        }
        ReadinessKind::None => Ok(true),
    }
}

pub(crate) async fn readiness_process_failure(ctx: &ReadinessContext) -> Result<Option<String>> {
    let Some(systemd) = ctx.systemd.as_ref() else {
        return Ok(None);
    };
    let Some(unit_name) = ctx.unit_name.as_deref() else {
        return Ok(None);
    };

    let Some(status) = systemd.unit_status(unit_name).await? else {
        return Ok(None);
    };

    if status.active_state == "failed" {
        let result = status.result.unwrap_or_else(|| "unknown".to_string());
        return Ok(Some(format!(
            "service exited before readiness (active_state=failed, sub_state={}, result={result})",
            status.sub_state
        )));
    }

    if status.active_state == "inactive" {
        if status.result.as_deref() == Some("success") {
            return Ok(None);
        }
        if let Some(result) = status.result {
            return Ok(Some(format!(
                "service exited before readiness (active_state=inactive, sub_state={}, result={result})",
                status.sub_state
            )));
        }
    }

    Ok(None)
}

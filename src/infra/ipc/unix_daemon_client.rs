use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::paths;

#[derive(Clone, Debug)]
pub struct UnixDaemonClient {
    require_existing_socket: bool,
    missing_socket_message: Option<String>,
    connect_context: Option<String>,
    failure_prefix: &'static str,
}

impl Default for UnixDaemonClient {
    fn default() -> Self {
        Self {
            require_existing_socket: false,
            missing_socket_message: None,
            connect_context: None,
            failure_prefix: "daemon request failed:",
        }
    }
}

impl UnixDaemonClient {
    pub fn for_cli() -> Self {
        Self::default()
            .require_existing_socket(true)
            .with_missing_socket_message(
                "daemon socket missing at {socket}; run `devstack daemon` (foreground) or `devstack install` (system service)",
            )
            .with_connect_context(
                "connect to daemon socket at {socket} (is the daemon running? try `devstack daemon` or `devstack install`)",
            )
            .with_failure_prefix("daemon error:")
    }

    pub fn require_existing_socket(mut self, require_existing_socket: bool) -> Self {
        self.require_existing_socket = require_existing_socket;
        self
    }

    pub fn with_missing_socket_message(mut self, message: impl Into<String>) -> Self {
        self.missing_socket_message = Some(message.into());
        self
    }

    pub fn with_connect_context(mut self, context: impl Into<String>) -> Self {
        self.connect_context = Some(context.into());
        self
    }

    pub fn with_failure_prefix(mut self, prefix: &'static str) -> Self {
        self.failure_prefix = prefix;
        self
    }

    pub fn socket_path(&self) -> Result<PathBuf> {
        paths::daemon_socket_path()
    }

    pub async fn request<T: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: Option<T>,
        timeout_duration: Option<Duration>,
    ) -> Result<serde_json::Value> {
        let fut = self.request_inner(method, path, body);
        if let Some(timeout_duration) = timeout_duration {
            match timeout(timeout_duration, fut).await {
                Ok(result) => result,
                Err(_) => Err(DaemonTimeout.into()),
            }
        } else {
            fut.await
        }
    }

    pub async fn request_json<T, R>(
        &self,
        method: &str,
        path: &str,
        body: Option<T>,
        timeout_duration: Option<Duration>,
    ) -> Result<R>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        let response = self.request(method, path, body, timeout_duration).await?;
        Ok(serde_json::from_value(response)?)
    }

    async fn request_inner<T: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: Option<T>,
    ) -> Result<serde_json::Value> {
        let socket_path = self.socket_path()?;
        if self.require_existing_socket && !socket_path.exists() {
            return Err(anyhow!(self.format_missing_socket_message(&socket_path)));
        }

        let stream = UnixStream::connect(&socket_path)
            .await
            .with_context(|| self.format_connect_context(&socket_path))?;
        let io = TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .context("handshake with daemon")?;
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let body_bytes = if let Some(payload) = body {
            serde_json::to_vec(&payload)?
        } else {
            Vec::new()
        };

        let request = Request::builder()
            .method(method)
            .uri(format!("http://localhost{path}"))
            .header("content-type", "application/json")
            .body(Full::new(hyper::body::Bytes::from(body_bytes)))?;

        let response = sender.send_request(request).await.context("send request")?;
        let status = response.status();
        let body = response.into_body().collect().await?.to_bytes();

        if !status.is_success() {
            return Err(anyhow!(
                "{} {status} {}",
                self.failure_prefix,
                String::from_utf8_lossy(&body)
            ));
        }

        if body.is_empty() {
            return Ok(serde_json::json!({}));
        }

        Ok(serde_json::from_slice(&body)?)
    }

    fn format_missing_socket_message(&self, socket_path: &std::path::Path) -> String {
        self.missing_socket_message
            .as_deref()
            .unwrap_or("daemon socket missing at {socket}")
            .replace("{socket}", &socket_path.display().to_string())
    }

    fn format_connect_context(&self, socket_path: &std::path::Path) -> String {
        self.connect_context
            .as_deref()
            .unwrap_or("connect to daemon socket {socket}")
            .replace("{socket}", &socket_path.display().to_string())
    }
}

#[derive(Debug)]
struct DaemonTimeout;

impl std::fmt::Display for DaemonTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "daemon request timed out")
    }
}

impl std::error::Error for DaemonTimeout {}

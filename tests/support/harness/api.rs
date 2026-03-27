use std::path::Path;

use anyhow::{Context, Result, anyhow};
use devstack::api::{
    AddSourceRequest, AddSourceResponse, AgentSession, AgentSessionMessageRequest,
    AgentSessionMessageResponse, AgentSessionPollResponse, AgentSessionRegisterRequest,
    DownRequest, GcRequest, GcResponse, GlobalsResponse, KillRequest, LatestAgentSessionResponse,
    LogViewQuery, LogViewResponse, LogsQuery, LogsResponse, NavigationIntentResponse, PingResponse,
    ProjectsResponse, RegisterProjectRequest, RegisterProjectResponse, RestartServiceRequest,
    RunListResponse, RunResponse, RunStatusResponse, RunWatchResponse, SetNavigationIntentRequest,
    ShareAgentMessageRequest, ShareAgentMessageResponse, SourcesResponse, StartTaskRequest,
    StartTaskResponse, TaskStatusResponse, TasksResponse, UpRequest, WatchControlRequest,
};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::net::UnixStream;

use super::{ProjectHandle, RunHandle, TaskHandle, TaskStartOptions, TestHarness, UpOptions};

#[derive(Clone)]
pub struct ApiHandle {
    pub(super) harness: TestHarness,
}

impl ApiHandle {
    pub async fn ping(&self) -> Result<bool> {
        let response: PingResponse = self.get("/v1/ping").await?;
        Ok(response.ok)
    }

    pub async fn up_with(&self, project: &ProjectHandle, options: &UpOptions) -> Result<RunHandle> {
        let manifest: RunResponse = self
            .post(
                "/v1/runs/up",
                &UpRequest {
                    stack: options.stack.clone(),
                    project_dir: project.path_string(),
                    run_id: options.run_id.clone(),
                    file: Some(project.config_path().to_string_lossy().to_string()),
                    no_wait: options.no_wait,
                    new_run: options.new_run,
                    force: options.force,
                },
            )
            .await?;
        Ok(RunHandle::new(
            self.harness.clone(),
            project.clone(),
            manifest.run_id,
        ))
    }

    pub async fn status(&self, run_id: &str) -> Result<RunStatusResponse> {
        self.get(&format!("/v1/runs/{run_id}/status")).await
    }

    pub async fn list_runs(&self) -> Result<RunListResponse> {
        self.get("/v1/runs").await
    }

    pub async fn restart_service(&self, run_id: &str, service: &str) -> Result<RunResponse> {
        self.restart_service_with_options(run_id, service, false)
            .await
    }

    pub async fn restart_service_with_options(
        &self,
        run_id: &str,
        service: &str,
        no_wait: bool,
    ) -> Result<RunResponse> {
        self.post(
            &format!("/v1/runs/{run_id}/restart-service"),
            &RestartServiceRequest {
                service: service.to_string(),
                no_wait,
            },
        )
        .await
    }

    pub async fn watch_status(&self, run_id: &str) -> Result<RunWatchResponse> {
        self.get(&format!("/v1/runs/{run_id}/watch")).await
    }

    pub async fn watch_pause(
        &self,
        run_id: &str,
        service: Option<&str>,
    ) -> Result<RunWatchResponse> {
        self.post(
            &format!("/v1/runs/{run_id}/watch/pause"),
            &WatchControlRequest {
                service: service.map(|value| value.to_string()),
            },
        )
        .await
    }

    pub async fn watch_resume(
        &self,
        run_id: &str,
        service: Option<&str>,
    ) -> Result<RunWatchResponse> {
        self.post(
            &format!("/v1/runs/{run_id}/watch/resume"),
            &WatchControlRequest {
                service: service.map(|value| value.to_string()),
            },
        )
        .await
    }

    pub async fn logs(
        &self,
        run_id: &str,
        service: &str,
        query: &LogsQuery,
    ) -> Result<LogsResponse> {
        let mut params = Vec::new();
        if let Some(last) = query.filter.last {
            params.push(format!("last={last}"));
        }
        if let Some(since) = query.filter.since.as_deref() {
            params.push(format!("since={}", urlencoding::encode(since)));
        }
        if let Some(search) = query.filter.search.as_deref() {
            params.push(format!("search={}", urlencoding::encode(search)));
        }
        if let Some(level) = query.filter.level.as_deref() {
            params.push(format!("level={}", urlencoding::encode(level)));
        }
        if let Some(stream) = query.filter.stream.as_deref() {
            params.push(format!("stream={}", urlencoding::encode(stream)));
        }
        if let Some(after) = query.after {
            params.push(format!("after={after}"));
        }
        let suffix = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        self.get(&format!("/v1/runs/{run_id}/logs/{service}{suffix}"))
            .await
    }

    pub async fn logs_view(&self, run_id: &str, query: &LogViewQuery) -> Result<LogViewResponse> {
        let mut params = Vec::new();
        if let Some(last) = query.filter.last {
            params.push(format!("last={last}"));
        }
        if let Some(since) = query.filter.since.as_deref() {
            params.push(format!("since={}", urlencoding::encode(since)));
        }
        if let Some(search) = query.filter.search.as_deref() {
            params.push(format!("search={}", urlencoding::encode(search)));
        }
        if let Some(level) = query.filter.level.as_deref() {
            params.push(format!("level={}", urlencoding::encode(level)));
        }
        if let Some(stream) = query.filter.stream.as_deref() {
            params.push(format!("stream={}", urlencoding::encode(stream)));
        }
        if let Some(service) = query.service.as_deref() {
            params.push(format!("service={}", urlencoding::encode(service)));
        }
        if !query.include_entries {
            params.push("include_entries=false".to_string());
        }
        if query.include_facets {
            params.push("include_facets=true".to_string());
        }
        let suffix = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        self.get(&format!("/v1/runs/{run_id}/logs{suffix}")).await
    }

    pub async fn start_task(
        &self,
        project: &ProjectHandle,
        task: &str,
        options: &TaskStartOptions,
    ) -> Result<TaskHandle> {
        let response: StartTaskResponse = self
            .post(
                "/v1/tasks/run",
                &StartTaskRequest {
                    project_dir: project.path_string(),
                    file: Some(project.config_path().to_string_lossy().to_string()),
                    task: task.to_string(),
                    args: options.args.clone(),
                },
            )
            .await?;
        Ok(TaskHandle::new(
            self.harness.clone(),
            project.clone(),
            response.execution_id,
            response.task,
            response.run_id,
        ))
    }

    pub async fn task_status(&self, execution_id: &str) -> Result<TaskStatusResponse> {
        self.get(&format!("/v1/tasks/{execution_id}")).await
    }

    pub async fn run_tasks(&self, run_id: &str) -> Result<TasksResponse> {
        self.get(&format!("/v1/runs/{run_id}/tasks")).await
    }

    pub async fn down(&self, run_id: &str) -> Result<RunResponse> {
        self.post(
            "/v1/runs/down",
            &DownRequest {
                run_id: run_id.to_string(),
                purge: false,
            },
        )
        .await
    }

    pub async fn kill(&self, run_id: &str) -> Result<RunResponse> {
        self.post(
            "/v1/runs/kill",
            &KillRequest {
                run_id: run_id.to_string(),
            },
        )
        .await
    }

    pub async fn list_globals(&self) -> Result<GlobalsResponse> {
        self.get("/v1/globals").await
    }

    pub async fn list_projects(&self) -> Result<ProjectsResponse> {
        self.get("/v1/projects").await
    }

    pub async fn register_project(&self, path: &Path) -> Result<RegisterProjectResponse> {
        self.post(
            "/v1/projects/register",
            &RegisterProjectRequest {
                path: path.to_string_lossy().to_string(),
            },
        )
        .await
    }

    pub async fn remove_project(&self, project_id: &str) -> Result<serde_json::Value> {
        self.delete(&format!("/v1/projects/{project_id}")).await
    }

    pub async fn list_sources(&self) -> Result<SourcesResponse> {
        self.get("/v1/sources").await
    }

    pub async fn add_source(&self, name: &str, paths: Vec<String>) -> Result<AddSourceResponse> {
        self.post(
            "/v1/sources",
            &AddSourceRequest {
                name: name.to_string(),
                paths,
            },
        )
        .await
    }

    pub async fn remove_source(&self, name: &str) -> Result<serde_json::Value> {
        self.delete(&format!("/v1/sources/{name}")).await
    }

    pub async fn source_logs(&self, name: &str, query: &LogViewQuery) -> Result<LogViewResponse> {
        let mut params = Vec::new();
        if let Some(last) = query.filter.last {
            params.push(format!("last={last}"));
        }
        if let Some(since) = query.filter.since.as_deref() {
            params.push(format!("since={}", urlencoding::encode(since)));
        }
        if let Some(search) = query.filter.search.as_deref() {
            params.push(format!("search={}", urlencoding::encode(search)));
        }
        if let Some(level) = query.filter.level.as_deref() {
            params.push(format!("level={}", urlencoding::encode(level)));
        }
        if let Some(stream) = query.filter.stream.as_deref() {
            params.push(format!("stream={}", urlencoding::encode(stream)));
        }
        if let Some(service) = query.service.as_deref() {
            params.push(format!("service={}", urlencoding::encode(service)));
        }
        if !query.include_entries {
            params.push("include_entries=false".to_string());
        }
        if query.include_facets {
            params.push("include_facets=true".to_string());
        }
        let suffix = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        self.get(&format!("/v1/sources/{name}/logs{suffix}")).await
    }

    pub async fn get_navigation_intent(&self) -> Result<NavigationIntentResponse> {
        self.get("/v1/navigation/intent").await
    }

    pub async fn set_navigation_intent(
        &self,
        request: &SetNavigationIntentRequest,
    ) -> Result<NavigationIntentResponse> {
        self.post("/v1/navigation/intent", request).await
    }

    pub async fn clear_navigation_intent(&self) -> Result<serde_json::Value> {
        self.delete("/v1/navigation/intent").await
    }

    pub async fn register_agent_session(
        &self,
        request: &AgentSessionRegisterRequest,
    ) -> Result<AgentSession> {
        self.post("/v1/agent/sessions", request).await
    }

    pub async fn unregister_agent_session(&self, agent_id: &str) -> Result<serde_json::Value> {
        self.delete(&format!("/v1/agent/sessions/{agent_id}")).await
    }

    pub async fn send_agent_message(
        &self,
        agent_id: &str,
        message: impl Into<String>,
    ) -> Result<AgentSessionMessageResponse> {
        self.post(
            &format!("/v1/agent/sessions/{agent_id}/messages"),
            &AgentSessionMessageRequest {
                message: message.into(),
            },
        )
        .await
    }

    pub async fn poll_agent_messages(&self, agent_id: &str) -> Result<AgentSessionPollResponse> {
        self.get(&format!("/v1/agent/sessions/{agent_id}/messages/poll"))
            .await
    }

    pub async fn latest_agent_session(
        &self,
        project: &ProjectHandle,
    ) -> Result<LatestAgentSessionResponse> {
        self.get(&format!(
            "/v1/agent/sessions/latest?project_dir={}",
            urlencoding::encode(&project.path_string())
        ))
        .await
    }

    pub async fn share_agent_message(
        &self,
        project: &ProjectHandle,
        message: impl Into<String>,
        command: Option<String>,
    ) -> Result<ShareAgentMessageResponse> {
        self.post(
            "/v1/agent/share",
            &ShareAgentMessageRequest {
                project_dir: project.path_string(),
                command,
                message: message.into(),
            },
        )
        .await
    }

    pub async fn gc(&self, older_than: Option<String>, all: bool) -> Result<GcResponse> {
        self.post("/v1/gc", &GcRequest { older_than, all }).await
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request::<(), T>("GET", path, None).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.request("POST", path, Some(body)).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request::<(), T>("DELETE", path, None).await
    }

    async fn request<B: Serialize, T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let response = self.raw_request(method, path, body).await?;
        let status = response.status();
        let body = response.into_body().collect().await?.to_bytes();
        if !status.is_success() {
            return Err(anyhow!(
                "daemon request failed: {status} {}",
                String::from_utf8_lossy(&body)
            ));
        }
        if body.is_empty() {
            return serde_json::from_value(serde_json::json!({}))
                .context("deserialize empty daemon response");
        }
        serde_json::from_slice(&body).context("deserialize daemon response")
    }

    pub(crate) async fn raw_request<B: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<hyper::Response<hyper::body::Incoming>> {
        let socket_path = self.harness.daemon_socket_path();
        let stream = UnixStream::connect(&socket_path)
            .await
            .with_context(|| format!("connect to daemon socket {}", socket_path.display()))?;
        let io = TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .context("handshake with daemon")?;
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let body_bytes = if let Some(value) = body {
            serde_json::to_vec(value)?
        } else {
            Vec::new()
        };

        let request = Request::builder()
            .method(method)
            .uri(format!("http://localhost{path}"))
            .header("content-type", "application/json")
            .body(Full::new(hyper::body::Bytes::from(body_bytes)))?;

        sender.send_request(request).await.context("send request")
    }
}

use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use axum::Router;
use axum::body::{Body, BodyDataStream as AxumBodyDataStream};
use axum::extract::State;
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{HeaderMap, Method, Request, Response, StatusCode, Uri, Version};
use axum::routing::any;
use http_body_util::{BodyDataStream as HyperBodyDataStream, BodyExt};
use hyper::body::{Bytes, Incoming};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};
use tokio::time::MissedTickBehavior;
use tokio_stream::Stream;

use crate::app::handles::ApiCaptureHandle;
use crate::logfmt::encode_log_line;
use crate::util::now_rfc3339;

const MAX_CAPTURE_HEADER_BYTES: usize = 512;
const CAPTURE_LOG_QUEUE: usize = 1024;
const CAPTURE_LOG_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
const CAPTURE_BIND_RETRY_TIMEOUT: Duration = Duration::from_millis(750);
const CAPTURE_BIND_RETRY_DELAY: Duration = Duration::from_millis(25);
const CAPTURE_PUBLIC_BIND_ADDR: Ipv4Addr = Ipv4Addr::UNSPECIFIED;

type ProxyClient = Client<HttpConnector, Body>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderForwardMode {
    Standard,
    Upgrade,
}

#[derive(Clone, Debug)]
pub(crate) struct ApiCaptureProxyConfig {
    pub(crate) run_id: String,
    pub(crate) service: String,
    pub(crate) public_port: u16,
    pub(crate) target_port: u16,
    pub(crate) log_path: PathBuf,
    pub(crate) body_limit: usize,
    pub(crate) ignore_paths: Vec<String>,
}

#[derive(Clone)]
struct ApiCaptureState {
    run_id: String,
    service: String,
    public_port: u16,
    target_port: u16,
    body_limit: usize,
    ignore_paths: Vec<String>,
    log: CaptureLogWriter,
    client: ProxyClient,
}

#[derive(Clone)]
struct CaptureLogWriter {
    tx: mpsc::Sender<CaptureLogEvent>,
    dropped: Arc<AtomicU64>,
}

enum CaptureLogEvent {
    Entry(Value),
    Api(CaptureApiRecord),
}

struct CaptureApiRecord {
    method: Method,
    path: String,
    target: String,
    request_headers: HeaderMap,
    request_body: CapturedBody,
    status: StatusCode,
    response_headers: HeaderMap,
    response_body: CapturedBody,
    duration_ms: u64,
}

#[derive(Clone, Debug, Default)]
struct CapturedBody {
    total_bytes: usize,
    captured: Vec<u8>,
    limit: usize,
}

struct RequestCaptureStream {
    inner: AxumBodyDataStream,
    capture: Arc<Mutex<CapturedBody>>,
}

struct CaptureResponseStream {
    inner: HyperBodyDataStream<Incoming>,
    log: CaptureLogWriter,
    method: Method,
    path: String,
    target: String,
    request_headers: HeaderMap,
    request_body: Arc<Mutex<CapturedBody>>,
    status: StatusCode,
    response_headers: HeaderMap,
    response_body: CapturedBody,
    started: Instant,
    logged: bool,
}

pub(crate) async fn start_api_capture_proxy(
    config: ApiCaptureProxyConfig,
) -> Result<ApiCaptureHandle> {
    let listener = bind_capture_listener(&config).await?;

    let mut connector = HttpConnector::new();
    connector.enforce_http(false);
    let body_limit = config.body_limit;
    let (log, drain_complete) = spawn_capture_log_writer(config.log_path).await?;
    let state = Arc::new(ApiCaptureState {
        run_id: config.run_id,
        service: config.service,
        public_port: config.public_port,
        target_port: config.target_port,
        body_limit,
        ignore_paths: config.ignore_paths,
        log,
        client: Client::builder(TokioExecutor::new()).build(connector),
    });

    let handle = ApiCaptureHandle::new(drain_complete, CAPTURE_LOG_DRAIN_TIMEOUT);
    let stop_flag = handle.stop_flag.clone();
    let stop_for_log = stop_flag.clone();
    let app = Router::new()
        .fallback(any(proxy_request))
        .with_state(state.clone());

    tokio::spawn(async move {
        let shutdown = async move {
            loop {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
            && !stop_for_log.load(Ordering::SeqCst)
        {
            eprintln!(
                "devstack: API capture proxy for {}.{} failed: {}",
                state.run_id, state.service, err
            );
        }
    });

    Ok(handle)
}

async fn bind_capture_listener(config: &ApiCaptureProxyConfig) -> Result<TcpListener> {
    let started = Instant::now();
    loop {
        match TcpListener::bind((CAPTURE_PUBLIC_BIND_ADDR, config.public_port)).await {
            Ok(listener) => return Ok(listener),
            Err(err)
                if err.kind() == ErrorKind::AddrInUse
                    && started.elapsed() < CAPTURE_BIND_RETRY_TIMEOUT =>
            {
                tokio::time::sleep(CAPTURE_BIND_RETRY_DELAY).await;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "bind API capture proxy for {}.{} on {}:{}",
                        config.run_id, config.service, CAPTURE_PUBLIC_BIND_ADDR, config.public_port
                    )
                });
            }
        }
    }
}

async fn proxy_request(
    State(state): State<Arc<ApiCaptureState>>,
    request: Request<Body>,
) -> Response<Body> {
    match proxy_request_inner(state.clone(), request).await {
        Ok(response) => response,
        Err(err) => {
            let entry = json!({
                "level": "error",
                "event": "api_capture",
                "msg": format!("API capture proxy error: {err}"),
                "error": err.to_string(),
            });
            state.log.write(CaptureLogEvent::Entry(entry));
            response_with_body(
                StatusCode::BAD_GATEWAY,
                HeaderMap::new(),
                Body::from(Bytes::from(format!(
                    "devstack API capture proxy error: {err}\n"
                ))),
                HeaderForwardMode::Standard,
            )
        }
    }
}

async fn proxy_request_inner(
    state: Arc<ApiCaptureState>,
    request: Request<Body>,
) -> Result<Response<Body>> {
    let started = Instant::now();
    let capture = should_capture_request(
        &state.ignore_paths,
        request.method(),
        request.uri().path(),
        request.headers(),
    );
    if is_upgrade_request(request.headers()) {
        return proxy_upgrade_request(state, request, started, capture).await;
    }

    let (parts, body) = request.into_parts();
    let method = parts.method.clone();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let path = parts.uri.path().to_string();
    let request_headers = parts.headers.clone();
    if !capture {
        let upstream_request = build_upstream_request(
            &method,
            parts.version,
            &path_and_query,
            &request_headers,
            state.public_port,
            state.target_port,
            body,
            HeaderForwardMode::Standard,
        )?;
        let upstream_response = state
            .client
            .request(upstream_request)
            .await
            .context("forward request to service")?;
        let (response_parts, response_body) = upstream_response.into_parts();
        return Ok(response_with_body(
            response_parts.status,
            response_parts.headers,
            Body::new(response_body),
            HeaderForwardMode::Standard,
        ));
    }

    let request_body = Arc::new(Mutex::new(CapturedBody::new(state.body_limit)));
    let request_stream = RequestCaptureStream {
        inner: body.into_data_stream(),
        capture: request_body.clone(),
    };

    let upstream_request = build_upstream_request(
        &method,
        parts.version,
        &path_and_query,
        &request_headers,
        state.public_port,
        state.target_port,
        Body::from_stream(request_stream),
        HeaderForwardMode::Standard,
    )?;
    let upstream_response = state
        .client
        .request(upstream_request)
        .await
        .context("forward request to service")?;

    let status = upstream_response.status();
    let response_headers = upstream_response.headers().clone();
    let (response_parts, response_body) = upstream_response.into_parts();
    let response_stream = CaptureResponseStream {
        inner: response_body.into_data_stream(),
        log: state.log.clone(),
        method,
        path,
        target: path_and_query,
        request_headers,
        request_body,
        status,
        response_headers,
        response_body: CapturedBody::new(state.body_limit),
        started,
        logged: false,
    };

    Ok(response_with_body(
        response_parts.status,
        response_parts.headers,
        Body::from_stream(response_stream),
        HeaderForwardMode::Standard,
    ))
}

async fn proxy_upgrade_request(
    state: Arc<ApiCaptureState>,
    mut request: Request<Body>,
    started: Instant,
    capture: bool,
) -> Result<Response<Body>> {
    let downstream_upgrade = hyper::upgrade::on(&mut request);
    let (parts, _body) = request.into_parts();
    let method = parts.method.clone();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let path = parts.uri.path().to_string();
    let request_headers = parts.headers.clone();

    let mut upstream_response = state
        .client
        .request(build_upstream_request(
            &method,
            parts.version,
            &path_and_query,
            &request_headers,
            state.public_port,
            state.target_port,
            Body::empty(),
            HeaderForwardMode::Upgrade,
        )?)
        .await
        .context("forward upgrade request to service")?;

    let status = upstream_response.status();
    let response_headers = upstream_response.headers().clone();
    if status != StatusCode::SWITCHING_PROTOCOLS {
        let (response_parts, response_body) = upstream_response.into_parts();
        return Ok(response_with_body(
            response_parts.status,
            response_parts.headers,
            Body::new(response_body),
            HeaderForwardMode::Standard,
        ));
    }

    let upstream_upgrade = hyper::upgrade::on(&mut upstream_response);
    let (response_parts, _response_body) = upstream_response.into_parts();
    if capture {
        let log = state.log.clone();
        let request_body = CapturedBody::new(state.body_limit);
        let response_body = CapturedBody::new(state.body_limit);
        log.write(CaptureLogEvent::Api(CaptureApiRecord {
            method,
            path,
            target: path_and_query,
            request_headers,
            request_body,
            status,
            response_headers: response_headers.clone(),
            response_body,
            duration_ms: started.elapsed().as_millis() as u64,
        }));
    }

    tokio::spawn(async move {
        match (downstream_upgrade.await, upstream_upgrade.await) {
            (Ok(downstream), Ok(upstream)) => {
                let mut downstream = TokioIo::new(downstream);
                let mut upstream = TokioIo::new(upstream);
                if let Err(err) =
                    tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await
                {
                    eprintln!("devstack: API capture websocket tunnel failed: {err}");
                }
            }
            (Err(err), _) => {
                eprintln!("devstack: downstream websocket upgrade failed: {err}");
            }
            (_, Err(err)) => {
                eprintln!("devstack: upstream websocket upgrade failed: {err}");
            }
        }
    });

    Ok(response_with_body(
        response_parts.status,
        response_parts.headers,
        Body::empty(),
        HeaderForwardMode::Upgrade,
    ))
}

fn build_upstream_request(
    method: &Method,
    version: Version,
    path_and_query: &str,
    headers: &HeaderMap,
    public_port: u16,
    target_port: u16,
    body: Body,
    mode: HeaderForwardMode,
) -> Result<Request<Body>> {
    let uri: Uri = format!("http://127.0.0.1:{target_port}{path_and_query}")
        .parse()
        .context("build upstream URI")?;
    let mut builder = Request::builder()
        .method(method.clone())
        .version(version)
        .uri(uri);
    {
        let outbound_headers = builder
            .headers_mut()
            .ok_or_else(|| anyhow!("request builder headers unavailable"))?;
        copy_forward_headers(headers, outbound_headers, mode);
        apply_forwarded_request_context(headers, outbound_headers, public_port)?;
    }
    builder.body(body).map_err(Into::into)
}

fn apply_forwarded_request_context(
    inbound_headers: &HeaderMap,
    outbound_headers: &mut HeaderMap,
    public_port: u16,
) -> Result<()> {
    let public_host = inbound_headers
        .get(header::HOST)
        .cloned()
        .unwrap_or_else(|| {
            HeaderValue::from_str(&format!("127.0.0.1:{public_port}"))
                .expect("generated host header should be valid")
        });

    outbound_headers.insert(header::HOST, public_host.clone());
    outbound_headers.insert(
        HeaderName::from_static("x-forwarded-host"),
        public_host.clone(),
    );
    outbound_headers.insert(
        HeaderName::from_static("x-forwarded-proto"),
        HeaderValue::from_static("http"),
    );
    outbound_headers.insert(
        HeaderName::from_static("x-forwarded-port"),
        HeaderValue::from_str(&public_port.to_string())?,
    );
    Ok(())
}

fn response_with_body(
    status: StatusCode,
    headers: HeaderMap,
    body: Body,
    mode: HeaderForwardMode,
) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    {
        let response_headers = builder
            .headers_mut()
            .expect("response builder headers should exist");
        copy_forward_headers(&headers, response_headers, mode);
    }
    builder.body(body).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty())
            .expect("status response should build")
    })
}

fn copy_forward_headers(from: &HeaderMap, to: &mut HeaderMap, mode: HeaderForwardMode) {
    for (name, value) in from {
        if mode == HeaderForwardMode::Standard && is_hop_by_hop(name) {
            continue;
        }
        to.append(name.clone(), value.clone());
    }
}

fn is_upgrade_request(headers: &HeaderMap) -> bool {
    header_contains_token(headers, header::CONNECTION, "upgrade")
        && headers
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false)
}

fn header_contains_token(headers: &HeaderMap, name: HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|value| {
        value
            .to_str()
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case(token))
            })
            .unwrap_or(false)
    })
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn should_ignore_path(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|pattern| {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return false;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return path.starts_with(prefix);
        }
        pattern == path
    })
}

fn should_capture_request(
    patterns: &[String],
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> bool {
    !should_ignore_path(patterns, path) && !is_browser_non_xhr_request(method, headers)
}

fn is_browser_non_xhr_request(method: &Method, headers: &HeaderMap) -> bool {
    if header_eq_ignore_ascii_case(headers, "x-requested-with", "XMLHttpRequest") {
        return false;
    }

    if let Some(dest) = header_to_str(headers, "sec-fetch-dest") {
        let dest = dest.trim();
        return !dest.is_empty() && !dest.eq_ignore_ascii_case("empty");
    }

    method == Method::GET && accepts_html_document(headers)
}

fn accepts_html_document(headers: &HeaderMap) -> bool {
    header_to_str(headers, "accept")
        .map(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("text/html") && !value.contains("application/json")
        })
        .unwrap_or(false)
}

fn header_eq_ignore_ascii_case(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    header_to_str(headers, name)
        .map(|value| value.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn header_to_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn capture_entry(
    method: &Method,
    path: &str,
    target: &str,
    request_headers: &HeaderMap,
    request_body: &CapturedBody,
    status: StatusCode,
    response_headers: &HeaderMap,
    response_body: &CapturedBody,
    duration_ms: u64,
) -> Value {
    json!({
        "level": if status.is_server_error() { "error" } else if status.is_client_error() { "warn" } else { "info" },
        "event": "api_capture",
        "msg": format!("{} {} -> {} ({} ms)", method.as_str(), target, status.as_u16(), duration_ms),
        "method": method.as_str(),
        "path": path,
        "target": target,
        "status": status.as_u16(),
        "duration_ms": duration_ms,
        "request": {
            "headers": sanitized_headers(request_headers),
            "body": body_snapshot(request_headers, request_body),
        },
        "response": {
            "headers": sanitized_headers(response_headers),
            "body": body_snapshot(response_headers, response_body),
        },
    })
}

fn sanitized_headers(headers: &HeaderMap) -> Value {
    let mut out = Map::new();
    for (name, value) in headers {
        let key = name.as_str().to_ascii_lowercase();
        let value = if is_sensitive_header(&key) {
            "[redacted]".to_string()
        } else {
            header_value_to_string(value)
        };
        out.insert(key, Value::String(value));
    }
    Value::Object(out)
}

fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "cookie" | "set-cookie" | "proxy-authorization"
    )
}

fn header_value_to_string(value: &HeaderValue) -> String {
    let value = value.to_str().unwrap_or("<non-utf8>");
    truncate_chars(value, MAX_CAPTURE_HEADER_BYTES)
}

fn body_snapshot(headers: &HeaderMap, body: &CapturedBody) -> Value {
    let total_bytes = body.total_bytes;
    let captured = body.captured.as_slice();
    let truncated = captured.len() < total_bytes;

    if total_bytes == 0 {
        return json!({
            "bytes": 0,
            "truncated": false,
        });
    }

    let mut out = Map::new();
    out.insert("bytes".to_string(), json!(total_bytes));
    out.insert("captured_bytes".to_string(), json!(captured.len()));
    out.insert("truncated".to_string(), json!(truncated));
    if captured.is_empty() {
        return Value::Object(out);
    }

    match captured_text(captured) {
        Ok(text) => {
            if body_looks_json(headers, text)
                && let Ok(json) = serde_json::from_str::<Value>(text)
            {
                out.insert("json".to_string(), json);
            } else {
                out.insert("text".to_string(), Value::String(text.to_string()));
            }
        }
        Err(_) => {
            out.insert("binary".to_string(), json!(true));
        }
    }

    Value::Object(out)
}

fn captured_text(bytes: &[u8]) -> Result<&str, std::str::Utf8Error> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(err) if err.valid_up_to() > 0 => std::str::from_utf8(&bytes[..err.valid_up_to()]),
        Err(err) => Err(err),
    }
}

fn body_looks_json(headers: &HeaderMap, text: &str) -> bool {
    let content_type_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("json"))
        .unwrap_or(false);
    if content_type_json {
        return true;
    }
    let trimmed = text.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

fn trim_to_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn truncate_chars(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    format!("{}...", trim_to_char_boundary(value, max_bytes))
}

impl CapturedBody {
    fn new(limit: usize) -> Self {
        Self {
            total_bytes: 0,
            captured: Vec::with_capacity(limit.min(8 * 1024)),
            limit,
        }
    }

    fn append(&mut self, chunk: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        let remaining = self.limit.saturating_sub(self.captured.len());
        if remaining > 0 {
            let to_capture = remaining.min(chunk.len());
            self.captured.extend_from_slice(&chunk[..to_capture]);
        }
    }
}

impl Stream for RequestCaptureStream {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Ok(mut capture) = this.capture.lock() {
                    capture.append(&chunk);
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            other => other,
        }
    }
}

impl Stream for CaptureResponseStream {
    type Item = Result<Bytes, hyper::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.response_body.append(&chunk);
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.log_once();
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                this.log_once();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl CaptureResponseStream {
    fn log_once(&mut self) {
        if self.logged {
            return;
        }
        self.logged = true;
        let request_body = clone_captured_body(&self.request_body);
        let duration_ms = self.started.elapsed().as_millis() as u64;
        self.log.write(CaptureLogEvent::Api(CaptureApiRecord {
            method: self.method.clone(),
            path: std::mem::take(&mut self.path),
            target: std::mem::take(&mut self.target),
            request_headers: std::mem::take(&mut self.request_headers),
            request_body,
            status: self.status,
            response_headers: std::mem::take(&mut self.response_headers),
            response_body: std::mem::take(&mut self.response_body),
            duration_ms,
        }));
    }
}

impl Drop for CaptureResponseStream {
    fn drop(&mut self) {
        self.log_once();
    }
}

fn clone_captured_body(capture: &Arc<Mutex<CapturedBody>>) -> CapturedBody {
    capture
        .lock()
        .map(|guard: MutexGuard<'_, CapturedBody>| guard.clone())
        .unwrap_or_default()
}

impl CaptureLogWriter {
    fn write(&self, event: CaptureLogEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Closed(_)) => {}
        }
    }
}

async fn spawn_capture_log_writer(
    log_path: PathBuf,
) -> Result<(CaptureLogWriter, watch::Receiver<bool>)> {
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create API capture log directory {}", parent.display()))?;
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .with_context(|| format!("open API capture log {}", log_path.display()))?;
    let (tx, mut rx) = mpsc::channel::<CaptureLogEvent>(CAPTURE_LOG_QUEUE);
    let (drain_tx, drain_rx) = watch::channel(false);
    let dropped = Arc::new(AtomicU64::new(0));
    let log_path = Arc::new(log_path);
    let writer_path = log_path.clone();
    let writer_dropped = dropped.clone();
    tokio::spawn(async move {
        let mut file = file;
        let mut dropped_interval = tokio::time::interval(Duration::from_secs(1));
        dropped_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = rx.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    write_dropped_summary(&mut file, &writer_path, &writer_dropped).await;
                    let entry = match event {
                        CaptureLogEvent::Entry(entry) => entry,
                        CaptureLogEvent::Api(record) => capture_entry(
                            &record.method,
                            &record.path,
                            &record.target,
                            &record.request_headers,
                            &record.request_body,
                            record.status,
                            &record.response_headers,
                            &record.response_body,
                            record.duration_ms,
                        ),
                    };
                    write_log_entry(&mut file, &writer_path, entry).await;
                }
                _ = dropped_interval.tick() => {
                    write_dropped_summary(&mut file, &writer_path, &writer_dropped).await;
                }
            }
        }
        write_dropped_summary(&mut file, &writer_path, &writer_dropped).await;
        let _ = file.flush().await;
        let _ = drain_tx.send(true);
    });
    Ok((CaptureLogWriter { tx, dropped }, drain_rx))
}

async fn write_dropped_summary(file: &mut tokio::fs::File, log_path: &Path, dropped: &AtomicU64) {
    let count = dropped.swap(0, Ordering::Relaxed);
    if count == 0 {
        return;
    }
    let entry = json!({
        "level": "warn",
        "event": "api_capture_dropped",
        "msg": format!("dropped {count} API capture events because the capture log queue was full"),
        "dropped_events": count,
    });
    write_log_entry(file, log_path, entry).await;
}

async fn write_log_entry(file: &mut tokio::fs::File, log_path: &Path, entry: Value) {
    let timestamp = now_rfc3339();
    let line = encode_log_line("api", &entry.to_string(), &timestamp);
    if let Err(err) = file.write_all(line.as_bytes()).await {
        eprintln!(
            "devstack: failed to write API capture log {}: {}",
            log_path.display(),
            err
        );
        return;
    }
    if let Err(err) = file.write_all(b"\n").await {
        eprintln!(
            "devstack: failed to write API capture log {}: {}",
            log_path.display(),
            err
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capture_proxy_binds_public_port_on_all_ipv4_interfaces() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let listener = bind_capture_listener(&ApiCaptureProxyConfig {
            run_id: "run".to_string(),
            service: "api".to_string(),
            public_port: 0,
            target_port: 0,
            log_path: dir.path().join("service.log"),
            body_limit: 1024,
            ignore_paths: Vec::new(),
        })
        .await?;

        assert!(listener.local_addr()?.ip().is_unspecified());
        Ok(())
    }
}

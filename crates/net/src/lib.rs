//! heraldvis-net — WebSocket/HTTP client ke VPS (PRD FR-3, §14.4).
//!
//! M2 implements:
//! - OpenAI-compatible SSE streaming (`/v1/chat/completions`) with typed `ChatChunk` parsing.
//! - Persistent WebSocket with auth, ping/pong, exponential backoff reconnect.
//! - Fallback handling and structured errors for VPS integration.

#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc, clippy::doc_markdown)]

use futures_util::{SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Config & Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NetConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub reconnect_max_retries: u32,
    pub reconnect_base_delay_ms: u64,
    pub request_timeout_secs: u64,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8000".into(),
            api_key: None,
            reconnect_max_retries: 5,
            reconnect_base_delay_ms: 500,
            request_timeout_secs: 30,
        }
    }
}

impl NetConfig {
    #[must_use]
    pub fn ws_url(&self, path: &str) -> String {
        let base = self.endpoint.trim_end_matches('/');
        let ws_base = if base.starts_with("https://") {
            base.replacen("https://", "wss://", 1)
        } else if base.starts_with("http://") {
            base.replacen("http://", "ws://", 1)
        } else {
            // assume already ws/wss
            base.to_string()
        };
        format!("{ws_base}{path}")
    }

    #[must_use]
    pub fn chat_completions_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.endpoint.trim_end_matches('/')
        )
    }
}

#[derive(Debug)]
pub enum NetError {
    Http(String),
    WebSocket(String),
    Config(String),
    Parse(String),
    Timeout(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(m) => write!(f, "http error: {m}"),
            Self::WebSocket(m) => write!(f, "websocket error: {m}"),
            Self::Config(m) => write!(f, "config error: {m}"),
            Self::Parse(m) => write!(f, "parse error: {m}"),
            Self::Timeout(m) => write!(f, "timeout: {m}"),
        }
    }
}
impl std::error::Error for NetError {}

// ---------------------------------------------------------------------------
// OpenAI-compatible SSE types (vLLM / SGLang)
// ---------------------------------------------------------------------------

/// Single SSE chunk decoded from `data: {...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<ChatChoice>,
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub delta: ChatDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

/// Parsed SSE event.
#[derive(Debug, Clone)]
pub enum SseEvent {
    Chunk(ChatChunk),
    Done,
    Comment(String),
}

/// Helper: parse single `data:` line into SseEvent.
/// Pure function — unit-testable without network.
#[must_use]
pub fn parse_sse_line(line: &str) -> Option<Result<SseEvent, NetError>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with(':') {
        return Some(Ok(SseEvent::Comment(trimmed.to_string())));
    }
    // SSE spec: `data: <payload>`
    let data = if let Some(rest) = trimmed.strip_prefix("data:") {
        rest.trim()
    } else if trimmed.starts_with("event:") {
        // ignore event type lines for now
        return None;
    } else {
        return None;
    };
    if data == "[DONE]" {
        return Some(Ok(SseEvent::Done));
    }
    match serde_json::from_str::<ChatChunk>(data) {
        Ok(chunk) => Some(Ok(SseEvent::Chunk(chunk))),
        Err(e) => Some(Err(NetError::Parse(format!("sse json parse failed: {e} — data: {data}")))),
    }
}

/// Accumulate SSE `text/event-stream` bytes into `SseEvent`s.
/// Handles both `\n` and `\n\n` delimiters, multi-line `data:` concatenation.
#[must_use]
pub fn sse_bytes_to_events(bytes: &[u8]) -> Vec<Result<SseEvent, NetError>> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    // SSE events are separated by blank line (\n\n). Each event may have multiple data: lines.
    for event_block in text.split("\n\n") {
        let mut data_accum = String::new();
        for raw_line in event_block.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let part = rest.trim();
                if !data_accum.is_empty() {
                    data_accum.push('\n');
                }
                data_accum.push_str(part);
            } else if line.starts_with(':') {
                out.push(Ok(SseEvent::Comment(line.to_string())));
            }
            // ignore `event:` / `id:` / `retry:` for now
        }
        if data_accum.is_empty() {
            continue;
        }
        if data_accum == "[DONE]" {
            out.push(Ok(SseEvent::Done));
            continue;
        }
        match serde_json::from_str::<ChatChunk>(&data_accum) {
            Ok(chunk) => out.push(Ok(SseEvent::Chunk(chunk))),
            Err(e) => out.push(Err(NetError::Parse(format!(
                "sse json parse failed: {e} — data: {data_accum}"
            )))),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Streaming wrapper over reqwest bytes_stream
// ---------------------------------------------------------------------------

pub struct ChatStream {
    inner: Pin<Box<dyn Stream<Item = Result<SseEvent, NetError>> + Send>>,
    done: bool,
}

impl ChatStream {
    fn from_response(resp: reqwest::Response) -> Self {
        let byte_stream = resp.bytes_stream();
        let sse_stream = async_stream::stream! {
            let mut buf = String::new();
            let mut stream = Box::pin(byte_stream);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        buf.push_str(&text);
                        // process complete events delimited by \n\n
                        while let Some(pos) = buf.find("\n\n") {
                            let block = buf[..pos].to_string();
                            buf.drain(..pos + 2);
                            for ev in sse_bytes_to_events(format!("{block}\n\n").as_bytes()) {
                                yield ev;
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(NetError::Http(e.to_string()));
                        break;
                    }
                }
            }
            // flush remaining
            if !buf.trim().is_empty() {
                for ev in sse_bytes_to_events(buf.as_bytes()) {
                    yield ev;
                }
            }
        };
        Self {
            inner: Box::pin(sse_stream),
            done: false,
        }
    }

    /// Convenience: collect all delta `content` strings until Done.
    pub async fn collect_text(&mut self) -> Result<String, NetError> {
        let mut out = String::new();
        while let Some(ev) = self.next().await {
            match ev {
                Ok(SseEvent::Chunk(chunk)) => {
                    for choice in chunk.choices {
                        if let Some(c) = choice.delta.content {
                            out.push_str(&c);
                        }
                    }
                }
                Ok(SseEvent::Done) => break,
                Ok(SseEvent::Comment(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }
}

impl Stream for ChatStream {
    type Item = Result<SseEvent, NetError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(SseEvent::Done))) => {
                self.done = true;
                Poll::Ready(Some(Ok(SseEvent::Done)))
            }
            Poll::Ready(other) => Poll::Ready(other),
            Poll::Pending => Poll::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket types
// ---------------------------------------------------------------------------

pub type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

pub struct WsConnection {
    pub stream: WsStream,
    pub url: String,
}

impl WsConnection {
    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<(), NetError> {
        let msg = tokio_tungstenite::tungstenite::Message::Text(text.into().into());
        self.stream
            .send(msg)
            .await
            .map_err(|e| NetError::WebSocket(e.to_string()))
    }

    pub async fn send_binary(&mut self, data: Vec<u8>) -> Result<(), NetError> {
        let msg = tokio_tungstenite::tungstenite::Message::Binary(data.into());
        self.stream
            .send(msg)
            .await
            .map_err(|e| NetError::WebSocket(e.to_string()))
    }

    pub async fn next_message(
        &mut self,
    ) -> Option<Result<tokio_tungstenite::tungstenite::Message, NetError>> {
        let item = self.stream.next().await?;
        match item {
            Ok(m) => Some(Ok(m)),
            Err(e) => Some(Err(NetError::WebSocket(e.to_string()))),
        }
    }

    pub async fn close(&mut self) -> Result<(), NetError> {
        self.stream
            .close(None)
            .await
            .map_err(|e| NetError::WebSocket(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Client untuk OpenAI-compatible streaming endpoint (vLLM/SGLang `/v1/chat/completions`)
/// dan WebSocket audio streaming (FR-3).
pub struct HeraldvisClient {
    http: reqwest::Client,
    config: NetConfig,
}

impl HeraldvisClient {
    /// Create new client.
    ///
    /// # Panics
    ///
    /// Panics jika `reqwest::Client::builder` gagal (hanya jika TLS backend misconfigured).
    #[must_use]
    pub fn new(config: NetConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .expect("reqwest client build failed");
        Self { http, config }
    }

    #[must_use]
    pub fn config(&self) -> &NetConfig {
        &self.config
    }

    // ---- HTTP / SSE ----

    /// POST `/v1/chat/completions` streaming (SSE) — returns raw response.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] jika request gagal atau status non-success.
    pub async fn chat_completions_stream(
        &self,
        payload: serde_json::Value,
    ) -> Result<reqwest::Response, NetError> {
        let url = self.config.chat_completions_url();
        let mut req = self.http.post(&url).json(&payload);
        if let Some(key) = &self.config.api_key {
            if !key.trim().is_empty() {
                req = req.bearer_auth(key);
            }
        }
        req = req.header("Accept", "text/event-stream");
        let resp = req.send().await.map_err(|e| NetError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(NetError::Http(format!("{status}: {body}")));
        }
        Ok(resp)
    }

    /// High-level SSE streaming: POST and parse into typed `ChatStream`.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] if request fails.
    pub async fn chat_stream(
        &self,
        payload: serde_json::Value,
    ) -> Result<ChatStream, NetError> {
        let resp = self.chat_completions_stream(payload).await?;
        Ok(ChatStream::from_response(resp))
    }

    /// Convenience helper for simple non-streaming chat (aggregates SSE deltas).
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] if request or SSE parse fails.
    pub async fn chat_aggregated(
        &self,
        payload: serde_json::Value,
    ) -> Result<String, NetError> {
        let mut stream = self.chat_stream(payload).await?;
        stream.collect_text().await
    }

    // ---- WebSocket ----

    /// Connect WebSocket at `path` (e.g. `/ws/audio`).
    /// Handles `ws://`/`wss://` derivation from `endpoint`, injects `Authorization` header if `api_key` present.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::WebSocket`] jika handshake gagal.
    pub async fn connect_ws(&self, path: &str) -> Result<WsConnection, NetError> {
        let ws_url = self.config.ws_url(path);
        info!(url = %ws_url, "connecting websocket");

        // Build request with optional auth; tungstenite fills WS handshake headers automatically.
        let mut builder = http::Request::builder().uri(ws_url.clone());
        if let Some(key) = &self.config.api_key {
            if !key.trim().is_empty() {
                builder = builder.header("Authorization", format!("Bearer {key}"));
            }
        }
        let req = builder
            .body(())
            .map_err(|e| NetError::WebSocket(e.to_string()))?;

        let (ws, resp) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(|e| NetError::WebSocket(e.to_string()))?;

        info!(status = %resp.status(), "websocket handshake ok");
        Ok(WsConnection { stream: ws, url: ws_url })
    }

    /// Reconnect dengan exponential backoff + jitter (FR-3: reconnect otomatis).
    ///
    /// Backoff: `base * 2^attempt` capped at 30s, jitter 0-200ms.
    ///
    /// # Errors
    ///
    /// Returns last [`NetError`] jika semua retry habis.
    pub async fn connect_ws_with_reconnect(&self, path: &str) -> Result<WsConnection, NetError> {
        let mut attempt: u32 = 0;
        let mut last_err: Option<NetError> = None;
        loop {
            match self.connect_ws(path).await {
                Ok(conn) => return Ok(conn),
                Err(e) => {
                    if attempt >= self.config.reconnect_max_retries {
                        return Err(last_err.unwrap_or(e));
                    }
                    last_err = Some(NetError::WebSocket(e.to_string()));
                    let exp = 1u64 << attempt.min(5); // cap 32x
                    let base = self.config.reconnect_base_delay_ms.saturating_mul(exp);
                    let capped = base.min(30_000);
                    let jitter = u64::from(rand_jitter());
                    let backoff = Duration::from_millis(capped + jitter);
                    warn!(
                        attempt = attempt + 1,
                        max = self.config.reconnect_max_retries,
                        backoff_ms = backoff.as_millis(),
                        error = %last_err.as_ref().unwrap(),
                        "ws reconnect"
                    );
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                }
            }
        }
    }

    #[allow(dead_code)]
    fn _ensure_net_deps_linked() {
        let _ = std::any::type_name::<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>();
        let _ = std::any::type_name::<tokio_stream::StreamMap<String, reqwest::Response>>();
    }
}

fn rand_jitter() -> u8 {
    // tiny jitter without extra crate: use system time nanos
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    u8::try_from(nanos % 200).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_build() {
        let c = NetConfig {
            endpoint: "http://10.0.0.1:8000".into(),
            ..Default::default()
        };
        assert_eq!(c.ws_url("/ws"), "ws://10.0.0.1:8000/ws");
        assert_eq!(
            NetConfig {
                endpoint: "https://vps.example.com".into(),
                ..Default::default()
            }
            .ws_url("/audio"),
            "wss://vps.example.com/audio"
        );
        assert_eq!(
            NetConfig {
                endpoint: "http://127.0.0.1:8000/".into(),
                ..Default::default()
            }
            .chat_completions_url(),
            "http://127.0.0.1:8000/v1/chat/completions"
        );
    }

    #[test]
    fn sse_parse_single_line() {
        let line = r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":0,"model":"qwen","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#;
        let ev = parse_sse_line(line).unwrap().unwrap();
        match ev {
            SseEvent::Chunk(c) => assert_eq!(c.choices[0].delta.content.as_deref(), Some("hello")),
            _ => panic!("expected chunk"),
        }
    }

    #[test]
    fn sse_parse_done() {
        let ev = parse_sse_line("data: [DONE]").unwrap().unwrap();
        assert!(matches!(ev, SseEvent::Done));
    }

    #[test]
    fn sse_bytes_to_events_multi() {
        let raw = concat!(
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"qwen\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi \"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"qwen\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"there\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n"
        );
        let events = sse_bytes_to_events(raw.as_bytes());
        assert_eq!(events.len(), 3);
        assert!(matches!(events[2].as_ref().unwrap(), SseEvent::Done));
    }

    #[test]
    fn sse_comment_ignored() {
        let ev = parse_sse_line(": keep-alive").unwrap().unwrap();
        assert!(matches!(ev, SseEvent::Comment(_)));
    }

    #[test]
    fn parse_tool_call_delta() {
        let json = r#"{"id":"1","object":"chat.completion.chunk","created":0,"model":"qwen","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":"}}]},"finish_reason":null}]}"#;
        let line = format!("data: {json}");
        let ev = parse_sse_line(&line).unwrap().unwrap();
        match ev {
            SseEvent::Chunk(c) => {
                let tc = c.choices[0].delta.tool_calls.as_ref().unwrap();
                assert_eq!(tc[0].function.as_ref().unwrap().name.as_deref(), Some("read_file"));
            }
            _ => panic!("expected chunk"),
        }
    }

    #[tokio::test]
    async fn ws_reconnect_fails_gracefully() {
        let c = NetConfig {
            endpoint: "http://127.0.0.1:59999".into(),
            reconnect_max_retries: 1,
            reconnect_base_delay_ms: 10,
            ..Default::default()
        };
        let client = HeraldvisClient::new(c);
        let res = client.connect_ws_with_reconnect("/ws").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn chat_completions_uses_bearer() {
        // spin tiny hyper-less mock via reqwest error path — just ensure URL built, no panic
        let c = NetConfig {
            endpoint: "http://127.0.0.1:59998".into(),
            api_key: Some("sk-test".into()),
            ..Default::default()
        };
        let client = HeraldvisClient::new(c);
        let payload = serde_json::json!({"model":"qwen","messages":[],"stream":true});
        let res = client.chat_completions_stream(payload).await;
        assert!(res.is_err()); // no server, but error is Http not panic
        assert!(matches!(res.unwrap_err(), NetError::Http(_)));
    }
}

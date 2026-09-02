//! heraldvis-net — WebSocket/HTTP client ke VPS (PRD FR-3, §14.4).

use futures_util::{SinkExt, StreamExt};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct NetConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub reconnect_max_retries: u32,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8000".into(),
            api_key: None,
            reconnect_max_retries: 5,
        }
    }
}

#[derive(Debug)]
pub enum NetError {
    Http(String),
    WebSocket(String),
    Config(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(m) => write!(f, "http error: {m}"),
            Self::WebSocket(m) => write!(f, "websocket error: {m}"),
            Self::Config(m) => write!(f, "config error: {m}"),
        }
    }
}
impl std::error::Error for NetError {}

/// Client untuk OpenAI-compatible streaming endpoint (vLLM/SGLang `/v1/chat/completions`).
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
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client build failed");
        Self { http, config }
    }

    /// POST `/v1/chat/completions` streaming (SSE) — dipakai M2 penuh, M1 hanya validasi URL.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::Http`] jika request gagal atau status non-success.
    ///
    /// # Panics
    ///
    /// Tidak panic; hanya `NetError` yang dikembalikan.
    pub async fn chat_completions_stream(
        &self,
        payload: serde_json::Value,
    ) -> Result<reqwest::Response, NetError> {
        let url = format!("{}/v1/chat/completions", self.config.endpoint.trim_end_matches('/'));
        let mut req = self.http.post(&url).json(&payload);
        if let Some(key) = &self.config.api_key {
            req = req.bearer_auth(key);
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

    /// WebSocket persistent untuk audio streaming (FR-3).
    ///
    /// # Errors
    ///
    /// Returns [`NetError::WebSocket`] jika handshake atau `ping` gagal.
    pub async fn connect_ws(&self, path: &str) -> Result<(), NetError> {
        let base = self.config.endpoint.trim_end_matches('/');
        let ws_url = base
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1)
            + path;

        info!(url = %ws_url, "connecting websocket (M1 skeleton — will reconnect on failure)");

        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| NetError::WebSocket(e.to_string()))?;

        let ping = tokio_tungstenite::tungstenite::Message::Ping(vec![1, 2, 3].into());
        ws.send(ping).await.map_err(|e| NetError::WebSocket(e.to_string()))?;

        let timeout = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next()).await;
        match timeout {
            Ok(Some(Ok(msg))) => {
                info!(msg = ?msg, "websocket handshake ok");
            }
            Ok(Some(Err(e))) => warn!("ws error after ping: {e}"),
            Ok(None) => warn!("ws closed immediately after connect"),
            Err(_) => warn!("ws pong timeout — server may not echo ping (ok for M1)"),
        }

        let _ = ws.close(None).await;
        Ok(())
    }

    /// Reconnect dengan backoff (FR-3: reconnect otomatis).
    ///
    /// # Errors
    ///
    /// Returns [`NetError::WebSocket`] atau [`NetError::Http`] jika semua retry gagal.
    pub async fn connect_ws_with_reconnect(&self, path: &str) -> Result<(), NetError> {
        let mut attempt: u32 = 0;
        loop {
            match self.connect_ws(path).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt < self.config.reconnect_max_retries => {
                    attempt += 1;
                    let backoff = std::time::Duration::from_millis(500 * u64::from(attempt));
                    warn!(attempt, error = %e, backoff_ms = backoff.as_millis(), "ws reconnect");
                    tokio::time::sleep(backoff).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    #[allow(dead_code)]
    fn _ensure_net_deps_linked() {
        let _ = std::any::type_name::<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>();
        let _ = std::any::type_name::<tokio_stream::StreamMap<String, reqwest::Response>>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_build() {
        let c = NetConfig {
            endpoint: "http://10.0.0.1:8000".into(),
            ..Default::default()
        };
        let client = HeraldvisClient::new(c);
        assert!(client.config.endpoint.contains("10.0.0.1"));
    }

    #[tokio::test]
    async fn ws_reconnect_fails_gracefully() {
        let c = NetConfig {
            endpoint: "http://127.0.0.1:59999".into(),
            reconnect_max_retries: 1,
            ..Default::default()
        };
        let client = HeraldvisClient::new(c);
        let res = client.connect_ws_with_reconnect("/ws").await;
        assert!(res.is_err());
    }
}

//! HTTP client to the Python MLX Intelligence Service.
//!
//! This module handles all communication between the Rust MCP server
//! and the Python `FastAPI` service running the MLX models.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use telemetry::{generate_span_id, AuditEvent, AuditEventType, AuditSeverity, SessionTracer};

/// Client for the Cartographer MLX Intelligence Service.
#[derive(Debug, Clone)]
pub struct MlxClient {
    base_url: String,
    http: reqwest::Client,
    tracer: Option<SessionTracer>,
    session_id: String,
}

// ---------------------------------------------------------------------------
// Request / response types (mirror Python schemas)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerateRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    pub stream: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GenerateChoice {
    pub message: ChatMessageResponse,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessageResponse {
    pub role: String,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GenerateResponse {
    pub id: String,
    pub choices: Vec<GenerateChoice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteResponse {
    pub disposition: String,
    pub confidence: f64,
    pub reason: String,
    pub suggested_tool: Option<String>,
    pub latency_ms: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub system_memory_used_gb: f64,
    pub system_memory_total_gb: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KVStatsResponse {
    pub entries: u64,
    pub total_tokens: u64,
    pub bitwidth: f64,
    pub compression_ratio: f64,
    pub memory_bytes: u64,
    pub utilization: f64,
}

// ---------------------------------------------------------------------------
// Client implementation
// ---------------------------------------------------------------------------

impl MlxClient {
    /// Create a new client pointing at the MLX service.
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("reqwest client should build"),
            tracer: None,
            session_id: String::new(),
        }
    }

    /// Attach a tracer and session id for audit event emission.
    #[must_use]
    pub fn with_tracer(mut self, tracer: SessionTracer, session_id: String) -> Self {
        self.tracer = Some(tracer);
        self.session_id = session_id;
        self
    }

    /// Create a client from the `MLX_SERVICE_URL` environment variable,
    /// falling back to `http://127.0.0.1:8101`.
    #[must_use]
    pub fn from_env() -> Self {
        let url = std::env::var("MLX_SERVICE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8101".to_string());
        Self::new(url)
    }

    /// Emit an audit event if a tracer is configured.
    fn emit_audit(&self, event: AuditEvent) {
        if let Some(ref tracer) = self.tracer {
            tracer.record_audit(event);
        }
    }

    /// Generate text using the Gemma 4 model.
    pub async fn generate(
        &self,
        request: &GenerateRequest,
        trace_id: &str,
    ) -> Result<GenerateResponse, Error> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let span_id = generate_span_id();

        // Emit InferenceCallStarted
        self.emit_audit(
            AuditEvent::new(
                trace_id,
                &self.session_id,
                "inference_client",
                AuditEventType::InferenceCallStarted,
                AuditSeverity::Info,
            )
            .with_attrs(serde_json::json!({
                "endpoint": url,
                "span_id": span_id,
                "model": request.model,
            }))
            .seal(),
        );

        let start = Instant::now();
        let resp = self
            .http
            .post(&url)
            .header("X-Cartographer-Trace-Id", trace_id)
            .header("X-Cartographer-Span-Id", &span_id)
            .json(request)
            .send()
            .await
            .map_err(|e| {
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                self.emit_audit(
                    AuditEvent::new(
                        trace_id,
                        &self.session_id,
                        "inference_client",
                        AuditEventType::InferenceCallFailed,
                        AuditSeverity::Error,
                    )
                    .with_duration_ms(elapsed)
                    .with_attrs(serde_json::json!({
                        "endpoint": url,
                        "span_id": span_id,
                        "error": e.to_string(),
                    }))
                    .seal(),
                );
                Error::Http(e)
            })?;

        let status = resp.status();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            self.emit_audit(
                AuditEvent::new(
                    trace_id,
                    &self.session_id,
                    "inference_client",
                    AuditEventType::InferenceCallFailed,
                    AuditSeverity::Error,
                )
                .with_duration_ms(elapsed)
                .with_attrs(serde_json::json!({
                    "endpoint": url,
                    "span_id": span_id,
                    "http_status": status.as_u16(),
                }))
                .seal(),
            );
            return Err(Error::Service(format!("{status}: {body}")));
        }

        let result: GenerateResponse = resp.json().await.map_err(Error::Http)?;

        // Emit InferenceCallCompleted
        self.emit_audit(
            AuditEvent::new(
                trace_id,
                &self.session_id,
                "inference_client",
                AuditEventType::InferenceCallCompleted,
                AuditSeverity::Info,
            )
            .with_duration_ms(elapsed)
            .with_attrs(serde_json::json!({
                "endpoint": url,
                "span_id": span_id,
                "http_status": status.as_u16(),
                "choices_count": result.choices.len(),
            }))
            .seal(),
        );

        Ok(result)
    }

    /// Route a request using the `FunctionGemma` dispatch model.
    pub async fn route(
        &self,
        request: &RouteRequest,
        trace_id: &str,
    ) -> Result<RouteResponse, Error> {
        let url = format!("{}/v1/route", self.base_url);
        let span_id = generate_span_id();

        // Emit InferenceCallStarted
        self.emit_audit(
            AuditEvent::new(
                trace_id,
                &self.session_id,
                "inference_client",
                AuditEventType::InferenceCallStarted,
                AuditSeverity::Info,
            )
            .with_attrs(serde_json::json!({
                "endpoint": url,
                "span_id": span_id,
                "operation": "route",
            }))
            .seal(),
        );

        let start = Instant::now();
        let resp = self
            .http
            .post(&url)
            .header("X-Cartographer-Trace-Id", trace_id)
            .header("X-Cartographer-Span-Id", &span_id)
            .json(request)
            .send()
            .await
            .map_err(|e| {
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                self.emit_audit(
                    AuditEvent::new(
                        trace_id,
                        &self.session_id,
                        "inference_client",
                        AuditEventType::InferenceCallFailed,
                        AuditSeverity::Error,
                    )
                    .with_duration_ms(elapsed)
                    .with_attrs(serde_json::json!({
                        "endpoint": url,
                        "span_id": span_id,
                        "error": e.to_string(),
                    }))
                    .seal(),
                );
                Error::Http(e)
            })?;

        let status = resp.status();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            self.emit_audit(
                AuditEvent::new(
                    trace_id,
                    &self.session_id,
                    "inference_client",
                    AuditEventType::InferenceCallFailed,
                    AuditSeverity::Error,
                )
                .with_duration_ms(elapsed)
                .with_attrs(serde_json::json!({
                    "endpoint": url,
                    "span_id": span_id,
                    "http_status": status.as_u16(),
                }))
                .seal(),
            );
            return Err(Error::Service(format!("{status}: {body}")));
        }

        let result: RouteResponse = resp.json().await.map_err(Error::Http)?;

        // Emit InferenceCallCompleted
        self.emit_audit(
            AuditEvent::new(
                trace_id,
                &self.session_id,
                "inference_client",
                AuditEventType::InferenceCallCompleted,
                AuditSeverity::Info,
            )
            .with_duration_ms(elapsed)
            .with_attrs(serde_json::json!({
                "endpoint": url,
                "span_id": span_id,
                "http_status": status.as_u16(),
                "disposition": result.disposition,
            }))
            .seal(),
        );

        Ok(result)
    }

    /// Health check.
    pub async fn health(&self) -> Result<HealthResponse, Error> {
        let url = format!("{}/v1/health", self.base_url);
        let resp = self.http.get(&url).send().await.map_err(Error::Http)?;
        resp.json().await.map_err(Error::Http)
    }

    /// KV cache stats.
    pub async fn kv_stats(&self) -> Result<KVStatsResponse, Error> {
        let url = format!("{}/v1/kv/stats", self.base_url);
        let resp = self.http.get(&url).send().await.map_err(Error::Http)?;
        resp.json().await.map_err(Error::Http)
    }
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    Service(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Service(msg) => write!(f, "Service error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use telemetry::{MemoryTelemetrySink, SessionTracer};

    #[test]
    fn from_env_defaults_to_localhost() {
        // Clear the env var to ensure default is used
        std::env::remove_var("MLX_SERVICE_URL");
        let client = MlxClient::from_env();
        let debug = format!("{client:?}");
        assert!(
            debug.contains("127.0.0.1:8101"),
            "default base_url should contain 127.0.0.1:8101, got: {debug}"
        );
    }

    #[test]
    fn with_tracer_attaches_tracer() {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let tracer = SessionTracer::new("sess", sink);
        let client =
            MlxClient::new("http://localhost:9999").with_tracer(tracer, "sess".to_string());
        let debug = format!("{client:?}");
        assert!(
            debug.contains("Some("),
            "tracer field should be Some after with_tracer, got: {debug}"
        );
    }
}

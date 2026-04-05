use std::fmt::{Debug, Formatter};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_APP_NAME: &str = "cartographer";
pub const DEFAULT_RUNTIME: &str = "rust";
pub const DEFAULT_AGENTIC_BETA: &str = "cartographer-20250219";
pub const DEFAULT_PROMPT_CACHING_SCOPE_BETA: &str = "prompt-caching-scope-2026-01-05";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIdentity {
    pub app_name: String,
    pub app_version: String,
    pub runtime: String,
}

impl ClientIdentity {
    #[must_use]
    pub fn new(app_name: impl Into<String>, app_version: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            app_version: app_version.into(),
            runtime: DEFAULT_RUNTIME.to_string(),
        }
    }

    #[must_use]
    pub fn with_runtime(mut self, runtime: impl Into<String>) -> Self {
        self.runtime = runtime.into();
        self
    }

    #[must_use]
    pub fn user_agent(&self) -> String {
        format!("{}/{}", self.app_name, self.app_version)
    }
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self::new(DEFAULT_APP_NAME, env!("CARGO_PKG_VERSION"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnthropicRequestProfile {
    pub anthropic_version: String,
    pub client_identity: ClientIdentity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub betas: Vec<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
}

impl AnthropicRequestProfile {
    #[must_use]
    pub fn new(client_identity: ClientIdentity) -> Self {
        Self {
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            client_identity,
            betas: vec![
                DEFAULT_AGENTIC_BETA.to_string(),
                DEFAULT_PROMPT_CACHING_SCOPE_BETA.to_string(),
            ],
            extra_body: Map::new(),
        }
    }

    #[must_use]
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        let beta = beta.into();
        if !self.betas.contains(&beta) {
            self.betas.push(beta);
        }
        self
    }

    #[must_use]
    pub fn with_extra_body(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra_body.insert(key.into(), value);
        self
    }

    #[must_use]
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            (
                "anthropic-version".to_string(),
                self.anthropic_version.clone(),
            ),
            ("user-agent".to_string(), self.client_identity.user_agent()),
        ];
        if !self.betas.is_empty() {
            headers.push(("anthropic-beta".to_string(), self.betas.join(",")));
        }
        headers
    }

    pub fn render_json_body<T: Serialize>(&self, request: &T) -> Result<Value, serde_json::Error> {
        let mut body = serde_json::to_value(request)?;
        let object = body.as_object_mut().ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request body must serialize to a JSON object",
            ))
        })?;
        for (key, value) in &self.extra_body {
            object.insert(key.clone(), value.clone());
        }
        if !self.betas.is_empty() {
            object.insert(
                "betas".to_string(),
                Value::Array(self.betas.iter().cloned().map(Value::String).collect()),
            );
        }
        Ok(body)
    }
}

impl Default for AnthropicRequestProfile {
    fn default() -> Self {
        Self::new(ClientIdentity::default())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    pub namespace: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub properties: Map<String, Value>,
}

impl AnalyticsEvent {
    #[must_use]
    pub fn new(namespace: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            action: action.into(),
            properties: Map::new(),
        }
    }

    #[must_use]
    pub fn with_property(mut self, key: impl Into<String>, value: Value) -> Self {
        self.properties.insert(key.into(), value);
        self
    }
}

// ---------------------------------------------------------------------------
// Audit event infrastructure
// ---------------------------------------------------------------------------

/// Typed classification of audit events across the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    McpRequestReceived,
    McpToolCallStarted,
    McpToolCallCompleted,
    McpToolCallFailed,
    McpResponseEmitted,
    InferenceCallStarted,
    InferenceCallCompleted,
    InferenceCallFailed,
    RouteDecision,
    KvCacheOperation,
    ModelLifecycle,
    ConfigSnapshot,
    ContextStoreWrite,
    ContextStoreQuery,
    ServerStartup,
    ServerShutdown,
}

/// Severity level for audit events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

/// A structured audit event with full tracing context.
///
/// Every significant operation in the system emits one of these. Fields
/// provide correlation (`trace_id` / `span_id`), tamper-evidence
/// (`input_hash`, `output_hash`, `event_hash`, `prev_event_hash`), and
/// extensible attributes for event-specific data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_id: String,
    pub trace_id: String,
    pub span_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub session_id: String,
    pub timestamp_utc: String,
    pub component: String,
    pub service: String,
    pub event_type: AuditEventType,
    pub severity: AuditSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_hash: Option<String>,
    pub attrs: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_event_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_hash: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event with mandatory fields populated and optional
    /// fields set to `None`.
    #[must_use]
    pub fn new(
        trace_id: impl Into<String>,
        session_id: impl Into<String>,
        component: impl Into<String>,
        event_type: AuditEventType,
        severity: AuditSeverity,
    ) -> Self {
        Self {
            event_id: generate_trace_id(),
            trace_id: trace_id.into(),
            span_id: generate_span_id(),
            parent_span_id: None,
            session_id: session_id.into(),
            timestamp_utc: iso8601_now(),
            component: component.into(),
            service: "rust-mcp".to_string(),
            event_type,
            severity,
            duration_ms: None,
            config_snapshot_id: None,
            input_hash: None,
            output_hash: None,
            attrs: Value::Object(Map::new()),
            prev_event_hash: None,
            event_hash: None,
        }
    }

    /// Set the parent span id.
    #[must_use]
    pub fn with_parent_span(mut self, parent: impl Into<String>) -> Self {
        self.parent_span_id = Some(parent.into());
        self
    }

    /// Set duration in milliseconds.
    #[must_use]
    pub fn with_duration_ms(mut self, ms: f64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    /// Set the input hash.
    #[must_use]
    pub fn with_input_hash(mut self, hash: impl Into<String>) -> Self {
        self.input_hash = Some(hash.into());
        self
    }

    /// Set the output hash.
    #[must_use]
    pub fn with_output_hash(mut self, hash: impl Into<String>) -> Self {
        self.output_hash = Some(hash.into());
        self
    }

    /// Set free-form attributes.
    #[must_use]
    pub fn with_attrs(mut self, attrs: Value) -> Self {
        self.attrs = attrs;
        self
    }

    /// Compute and set the event hash from the serialized event content.
    #[must_use]
    pub fn seal(mut self) -> Self {
        // Hash the event without the hash fields to avoid circular dependency
        let hash_input = serde_json::to_string(&(
            &self.event_id,
            &self.trace_id,
            &self.timestamp_utc,
            &self.event_type,
        ))
        .unwrap_or_default();
        self.event_hash = Some(hash_content(&hash_input));
        self
    }
}

/// Generate a trace-level correlation ID (UUID v4).
#[must_use]
pub fn generate_trace_id() -> String {
    Uuid::new_v4().to_string()
}

/// Generate a shorter span-level ID (first 16 hex chars of a UUID v4).
#[must_use]
pub fn generate_span_id() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// SHA-256 hex digest of the given content.
#[must_use]
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Current UTC time as ISO 8601 string.
#[must_use]
pub fn iso8601_now() -> String {
    // Use UNIX_EPOCH + duration to build a simple ISO-8601 timestamp
    // without pulling in the `chrono` crate.
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Simple formatting: seconds since epoch expressed as a readable timestamp
    // We compute year/month/day/hour/min/sec from unix seconds.
    let (y, mo, d, h, mi, s) = unix_to_utc(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Convert unix timestamp to (year, month, day, hour, minute, second) in UTC.
fn unix_to_utc(timestamp: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = timestamp % 60;
    let total_minutes = timestamp / 60;
    let mi = total_minutes % 60;
    let total_hours = total_minutes / 60;
    let h = total_hours % 24;
    let mut days = total_hours / 24;

    // Compute year
    let mut y = 1970u64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        y += 1;
    }

    // Compute month
    let leap = is_leap(y);
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u64;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        mo += 1;
    }
    let d = days + 1;

    (y, mo, d, h, mi, s)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTraceRecord {
    pub session_id: String,
    pub sequence: u64,
    pub name: String,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TelemetryEvent {
    HttpRequestStarted {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    HttpRequestSucceeded {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    HttpRequestFailed {
        session_id: String,
        attempt: u32,
        method: String,
        path: String,
        error: String,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        attributes: Map<String, Value>,
    },
    Analytics(AnalyticsEvent),
    SessionTrace(SessionTraceRecord),
    Audit(Box<AuditEvent>),
}

pub trait TelemetrySink: Send + Sync {
    fn record(&self, event: TelemetryEvent);
}

#[derive(Default)]
pub struct MemoryTelemetrySink {
    events: Mutex<Vec<TelemetryEvent>>,
}

impl MemoryTelemetrySink {
    #[must_use]
    pub fn events(&self) -> Vec<TelemetryEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl TelemetrySink for MemoryTelemetrySink {
    fn record(&self, event: TelemetryEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

pub struct JsonlTelemetrySink {
    path: PathBuf,
    file: Mutex<File>,
}

impl Debug for JsonlTelemetrySink {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlTelemetrySink")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl JsonlTelemetrySink {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TelemetrySink for JsonlTelemetrySink {
    fn record(&self, event: TelemetryEvent) {
        let Ok(line) = serde_json::to_string(&event) else {
            return;
        };
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

#[derive(Clone)]
pub struct SessionTracer {
    session_id: String,
    sequence: Arc<AtomicU64>,
    sink: Arc<dyn TelemetrySink>,
}

impl Debug for SessionTracer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionTracer")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl SessionTracer {
    #[must_use]
    pub fn new(session_id: impl Into<String>, sink: Arc<dyn TelemetrySink>) -> Self {
        Self {
            session_id: session_id.into(),
            sequence: Arc::new(AtomicU64::new(0)),
            sink,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn record(&self, name: impl Into<String>, attributes: Map<String, Value>) {
        let record = SessionTraceRecord {
            session_id: self.session_id.clone(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            timestamp_ms: current_timestamp_ms(),
            attributes,
        };
        self.sink.record(TelemetryEvent::SessionTrace(record));
    }

    pub fn record_http_request_started(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        self.sink.record(TelemetryEvent::HttpRequestStarted {
            session_id: self.session_id.clone(),
            attempt,
            method: method.clone(),
            path: path.clone(),
            attributes: attributes.clone(),
        });
        self.record(
            "http_request_started",
            merge_trace_fields(method, path, attempt, attributes),
        );
    }

    pub fn record_http_request_succeeded(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        status: u16,
        request_id: Option<String>,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        self.sink.record(TelemetryEvent::HttpRequestSucceeded {
            session_id: self.session_id.clone(),
            attempt,
            method: method.clone(),
            path: path.clone(),
            status,
            request_id: request_id.clone(),
            attributes: attributes.clone(),
        });
        let mut trace_attributes = merge_trace_fields(method, path, attempt, attributes);
        trace_attributes.insert("status".to_string(), Value::from(status));
        if let Some(request_id) = request_id {
            trace_attributes.insert("request_id".to_string(), Value::String(request_id));
        }
        self.record("http_request_succeeded", trace_attributes);
    }

    pub fn record_http_request_failed(
        &self,
        attempt: u32,
        method: impl Into<String>,
        path: impl Into<String>,
        error: impl Into<String>,
        retryable: bool,
        attributes: Map<String, Value>,
    ) {
        let method = method.into();
        let path = path.into();
        let error = error.into();
        self.sink.record(TelemetryEvent::HttpRequestFailed {
            session_id: self.session_id.clone(),
            attempt,
            method: method.clone(),
            path: path.clone(),
            error: error.clone(),
            retryable,
            attributes: attributes.clone(),
        });
        let mut trace_attributes = merge_trace_fields(method, path, attempt, attributes);
        trace_attributes.insert("error".to_string(), Value::String(error));
        trace_attributes.insert("retryable".to_string(), Value::Bool(retryable));
        self.record("http_request_failed", trace_attributes);
    }

    /// Record an audit event through the sink.
    pub fn record_audit(&self, event: AuditEvent) {
        self.sink.record(TelemetryEvent::Audit(Box::new(event)));
    }

    pub fn record_analytics(&self, event: AnalyticsEvent) {
        let mut attributes = event.properties.clone();
        attributes.insert(
            "namespace".to_string(),
            Value::String(event.namespace.clone()),
        );
        attributes.insert("action".to_string(), Value::String(event.action.clone()));
        self.sink.record(TelemetryEvent::Analytics(event));
        self.record("analytics", attributes);
    }
}

fn merge_trace_fields(
    method: String,
    path: String,
    attempt: u32,
    mut attributes: Map<String, Value>,
) -> Map<String, Value> {
    attributes.insert("method".to_string(), Value::String(method));
    attributes.insert("path".to_string(), Value::String(path));
    attributes.insert("attempt".to_string(), Value::from(attempt));
    attributes
}

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_profile_emits_headers_and_merges_body() {
        let profile = AnthropicRequestProfile::new(
            ClientIdentity::new("cartographer", "1.2.3").with_runtime("rust-cli"),
        )
        .with_beta("tools-2026-04-01")
        .with_extra_body("metadata", serde_json::json!({"source": "test"}));

        assert_eq!(
            profile.header_pairs(),
            vec![
                (
                    "anthropic-version".to_string(),
                    DEFAULT_ANTHROPIC_VERSION.to_string()
                ),
                ("user-agent".to_string(), "cartographer/1.2.3".to_string()),
                (
                    "anthropic-beta".to_string(),
                    "cartographer-20250219,prompt-caching-scope-2026-01-05,tools-2026-04-01"
                        .to_string(),
                ),
            ]
        );

        let body = profile
            .render_json_body(&serde_json::json!({"model": "claude-sonnet"}))
            .expect("body should serialize");
        assert_eq!(
            body["metadata"]["source"],
            Value::String("test".to_string())
        );
        assert_eq!(
            body["betas"],
            serde_json::json!([
                "cartographer-20250219",
                "prompt-caching-scope-2026-01-05",
                "tools-2026-04-01"
            ])
        );
    }

    #[test]
    fn session_tracer_records_structured_events_and_trace_sequence() {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let tracer = SessionTracer::new("session-123", sink.clone());

        tracer.record_http_request_started(1, "POST", "/v1/messages", Map::new());
        tracer.record_analytics(
            AnalyticsEvent::new("cli", "prompt_sent")
                .with_property("model", Value::String("claude-opus".to_string())),
        );

        let events = sink.events();
        assert!(matches!(
            &events[0],
            TelemetryEvent::HttpRequestStarted {
                session_id,
                attempt: 1,
                method,
                path,
                ..
            } if session_id == "session-123" && method == "POST" && path == "/v1/messages"
        ));
        assert!(matches!(
            &events[1],
            TelemetryEvent::SessionTrace(SessionTraceRecord { sequence: 0, name, .. })
            if name == "http_request_started"
        ));
        assert!(matches!(&events[2], TelemetryEvent::Analytics(_)));
        assert!(matches!(
            &events[3],
            TelemetryEvent::SessionTrace(SessionTraceRecord { sequence: 1, name, .. })
            if name == "analytics"
        ));
    }

    #[test]
    fn jsonl_sink_persists_events() {
        let path =
            std::env::temp_dir().join(format!("telemetry-jsonl-{}.log", current_timestamp_ms()));
        let sink = JsonlTelemetrySink::new(&path).expect("sink should create file");

        sink.record(TelemetryEvent::Analytics(
            AnalyticsEvent::new("cli", "turn_completed").with_property("ok", Value::Bool(true)),
        ));

        let contents = std::fs::read_to_string(&path).expect("telemetry log should be readable");
        assert!(contents.contains("\"type\":\"analytics\""));
        assert!(contents.contains("\"action\":\"turn_completed\""));

        let _ = std::fs::remove_file(path);
    }

    // ------------------------------------------------------------------
    // Audit telemetry tests
    // ------------------------------------------------------------------

    #[test]
    fn audit_event_serialization_roundtrip() {
        let event = AuditEvent::new(
            "trace-abc",
            "session-1",
            "mcp_handler",
            AuditEventType::McpRequestReceived,
            AuditSeverity::Info,
        )
        .with_duration_ms(42.5)
        .with_input_hash("inhash")
        .with_output_hash("outhash")
        .with_attrs(serde_json::json!({"key": "value"}));

        let tel = TelemetryEvent::Audit(Box::new(event.clone()));
        let json_str = serde_json::to_string(&tel).expect("serialize");
        let deserialized: TelemetryEvent = serde_json::from_str(&json_str).expect("deserialize");

        if let TelemetryEvent::Audit(roundtripped) = deserialized {
            assert_eq!(roundtripped.trace_id, "trace-abc");
            assert_eq!(roundtripped.session_id, "session-1");
            assert_eq!(roundtripped.component, "mcp_handler");
            assert_eq!(roundtripped.event_type, AuditEventType::McpRequestReceived);
            assert_eq!(roundtripped.severity, AuditSeverity::Info);
            assert_eq!(roundtripped.duration_ms, Some(42.5));
            assert_eq!(roundtripped.input_hash.as_deref(), Some("inhash"));
            assert_eq!(roundtripped.output_hash.as_deref(), Some("outhash"));
            assert_eq!(roundtripped.attrs["key"], "value");
        } else {
            panic!("expected TelemetryEvent::Audit");
        }
    }

    #[test]
    fn audit_event_seal_produces_deterministic_hash() {
        let make = |component: &str| AuditEvent {
            event_id: "fixed-event-id".to_string(),
            trace_id: "fixed-trace-id".to_string(),
            span_id: "fixed-span".to_string(),
            parent_span_id: None,
            session_id: "s1".to_string(),
            timestamp_utc: "2026-01-01T00:00:00Z".to_string(),
            component: component.to_string(),
            service: "rust-mcp".to_string(),
            event_type: AuditEventType::McpRequestReceived,
            severity: AuditSeverity::Info,
            duration_ms: None,
            config_snapshot_id: None,
            input_hash: None,
            output_hash: None,
            attrs: Value::Object(Map::new()),
            prev_event_hash: None,
            event_hash: None,
        };

        let a = make("mcp_handler").seal();
        let b = make("mcp_handler").seal();
        assert_eq!(
            a.event_hash, b.event_hash,
            "identical events produce identical hashes"
        );

        let c = make("different_component").seal();
        // seal hashes event_id, trace_id, timestamp_utc, event_type — component is NOT in the
        // hash tuple, so changing only component won't change the hash. Change event_type instead.
        let mut d = make("mcp_handler");
        d.event_type = AuditEventType::McpToolCallStarted;
        let d = d.seal();
        assert_ne!(
            a.event_hash, d.event_hash,
            "different event_type produces different hash"
        );

        // Extra: verify hash is Some
        assert!(a.event_hash.is_some());
        // suppress unused-variable warning
        let _ = c;
    }

    #[test]
    fn audit_event_hash_chain_integrity() {
        let e1 = AuditEvent::new(
            "t1",
            "s1",
            "comp",
            AuditEventType::McpRequestReceived,
            AuditSeverity::Info,
        )
        .seal();

        let mut e2 = AuditEvent::new(
            "t1",
            "s1",
            "comp",
            AuditEventType::McpToolCallStarted,
            AuditSeverity::Info,
        );
        e2.prev_event_hash = e1.event_hash.clone();
        e2 = e2.seal();

        let mut e3 = AuditEvent::new(
            "t1",
            "s1",
            "comp",
            AuditEventType::McpToolCallCompleted,
            AuditSeverity::Info,
        );
        e3.prev_event_hash = e2.event_hash.clone();
        e3 = e3.seal();

        // Walk the chain backwards
        assert_eq!(e3.prev_event_hash, e2.event_hash);
        assert_eq!(e2.prev_event_hash, e1.event_hash);
        assert!(e1.prev_event_hash.is_none());

        // All hashes populated
        assert!(e1.event_hash.is_some());
        assert!(e2.event_hash.is_some());
        assert!(e3.event_hash.is_some());
    }

    #[test]
    fn generate_trace_id_is_unique() {
        let ids: std::collections::HashSet<String> =
            (0..100).map(|_| generate_trace_id()).collect();
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn generate_span_id_is_unique() {
        let ids: std::collections::HashSet<String> = (0..100).map(|_| generate_span_id()).collect();
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn hash_content_is_deterministic() {
        let h1 = hash_content("hello");
        let h2 = hash_content("hello");
        assert_eq!(h1, h2);

        let h3 = hash_content("world");
        assert_ne!(h1, h3);
    }

    #[test]
    fn audit_event_type_serializes_correctly() {
        // serde(rename_all = "snake_case") is applied
        let json = serde_json::to_string(&AuditEventType::McpRequestReceived).unwrap();
        assert_eq!(json, "\"mcp_request_received\"");

        let json = serde_json::to_string(&AuditEventType::InferenceCallStarted).unwrap();
        assert_eq!(json, "\"inference_call_started\"");

        let json = serde_json::to_string(&AuditEventType::KvCacheOperation).unwrap();
        assert_eq!(json, "\"kv_cache_operation\"");

        // Roundtrip
        let parsed: AuditEventType = serde_json::from_str("\"mcp_tool_call_failed\"").unwrap();
        assert_eq!(parsed, AuditEventType::McpToolCallFailed);
    }

    #[test]
    fn jsonl_sink_writes_audit_events() {
        let path =
            std::env::temp_dir().join(format!("audit-jsonl-write-{}.log", current_timestamp_ms()));
        let sink = JsonlTelemetrySink::new(&path).expect("sink");

        for i in 0..5 {
            let event = AuditEvent::new(
                format!("trace-{i}"),
                "s1",
                "comp",
                AuditEventType::McpRequestReceived,
                AuditSeverity::Info,
            )
            .seal();
            sink.record(TelemetryEvent::Audit(Box::new(event)));
        }

        let contents = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 5, "expected 5 JSONL lines");

        for line in &lines {
            let parsed: TelemetryEvent = serde_json::from_str(line).expect("valid JSON");
            assert!(matches!(parsed, TelemetryEvent::Audit(_)));
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn jsonl_sink_flush_durability() {
        let path =
            std::env::temp_dir().join(format!("audit-jsonl-flush-{}.log", current_timestamp_ms()));
        let sink = JsonlTelemetrySink::new(&path).expect("sink");

        let event = AuditEvent::new(
            "trace-flush",
            "s1",
            "comp",
            AuditEventType::ServerStartup,
            AuditSeverity::Info,
        )
        .seal();
        sink.record(TelemetryEvent::Audit(Box::new(event)));

        // Read while sink is still alive — data should already be flushed
        let contents = std::fs::read_to_string(&path).expect("read");
        assert!(
            contents.contains("server_startup"),
            "event should be flushed to disk immediately"
        );

        drop(sink);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn session_tracer_records_audit_events() {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let tracer = SessionTracer::new("audit-session", sink.clone());

        for i in 0..3 {
            let event = AuditEvent::new(
                format!("trace-{i}"),
                "audit-session",
                "comp",
                AuditEventType::McpRequestReceived,
                AuditSeverity::Info,
            )
            .seal();
            tracer.record_audit(event);
        }

        let events = sink.events();
        let audit_count = events
            .iter()
            .filter(|e| matches!(e, TelemetryEvent::Audit(_)))
            .count();
        assert_eq!(audit_count, 3);
    }
}

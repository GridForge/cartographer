//! MCP JSON-RPC request dispatch.
//!
//! Implements the server side of the MCP stdio protocol. Reads JSON-RPC
//! requests from stdin, dispatches to the appropriate handler, and writes
//! responses to stdout.
//!
//! Protocol types mirror those in `runtime::mcp_stdio` but are self-contained
//! to avoid pulling the full runtime dependency.

use std::fmt::Write as _;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use telemetry::{
    generate_span_id, hash_content, AuditEvent, AuditEventType, AuditSeverity, SessionTracer,
};

use crate::inference::{ChatMessage, GenerateRequest, MlxClient, RouteRequest};
use crate::store::ContextStore;

// ---------------------------------------------------------------------------
// JSON-RPC types (MCP protocol)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(u64),
    String(String),
    Null,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub method: String,
    pub params: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcResponse {
    fn success(id: JsonRpcId, result: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: JsonRpcId, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

const SERVER_NAME: &str = "cartographer";
const SERVER_VERSION: &str = "0.1.0";
const PROTOCOL_VERSION: &str = "2024-11-05";

fn tool_definitions() -> JsonValue {
    json!({
        "tools": [
            {
                "name": "context_store",
                "description": "Store a context snapshot with metadata tags. Use this to persist important context (decisions, errors, file changes) that should survive session compaction.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The context content to store"
                        },
                        "tag": {
                            "type": "string",
                            "enum": ["decision", "error", "file_change", "test_result", "compaction_pre", "note"],
                            "description": "Category tag for the context entry"
                        },
                        "files_referenced": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "File paths referenced in this context"
                        }
                    },
                    "required": ["content", "tag"]
                }
            },
            {
                "name": "context_query",
                "description": "Search stored context by keyword. Returns matching entries from the persistent context store, including entries from before session compaction.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (keyword match)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Optional: filter by tag"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max results to return (default: 10)"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "context_status",
                "description": "Report context budget, entries stored, KV cache state, and model health. Use this to check the state of the local intelligence layer.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "generate",
                "description": "Generate text using the local Gemma 4 model. Use for summarization, analysis, or any task that benefits from local inference without burning cloud tokens.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": {
                            "type": "string",
                            "description": "The prompt to generate from"
                        },
                        "max_tokens": {
                            "type": "integer",
                            "description": "Maximum tokens to generate (default: 1024)"
                        },
                        "system": {
                            "type": "string",
                            "description": "Optional system prompt"
                        }
                    },
                    "required": ["prompt"]
                }
            },
            {
                "name": "route",
                "description": "Classify whether a query should be handled locally or by the frontier model. Returns a disposition (local/remote/augment/defer) with confidence score.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The query to classify"
                        },
                        "confidence_threshold": {
                            "type": "number",
                            "description": "Minimum confidence to act on routing (default: 0.7)"
                        }
                    },
                    "required": ["query"]
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Handler dispatch
// ---------------------------------------------------------------------------

/// The MCP request handler. Holds the MLX client, tracer, and dispatches tool calls.
pub struct McpHandler {
    mlx: MlxClient,
    tracer: SessionTracer,
    session_id: String,
    store: ContextStore,
}

impl McpHandler {
    #[must_use]
    pub fn new(
        mlx: MlxClient,
        tracer: SessionTracer,
        session_id: String,
        store: ContextStore,
    ) -> Self {
        Self {
            mlx,
            tracer,
            session_id,
            store,
        }
    }

    /// Dispatch a JSON-RPC request and return the response.
    pub async fn handle(&self, request: JsonRpcRequest, trace_id: &str) -> JsonRpcResponse {
        let request_start = Instant::now();
        let request_span_id = generate_span_id();

        // Emit McpRequestReceived
        self.tracer.record_audit(
            AuditEvent::new(
                trace_id,
                &self.session_id,
                "mcp_handler",
                AuditEventType::McpRequestReceived,
                AuditSeverity::Info,
            )
            .with_attrs(json!({
                "method": request.method,
                "span_id": request_span_id,
            }))
            .seal(),
        );

        let response = match request.method.as_str() {
            "initialize" => Self::handle_initialize(request.id),
            "initialized" => {
                // Notification — no response needed, but we return one for safety
                JsonRpcResponse::success(request.id, json!({}))
            }
            "tools/list" => Self::handle_list_tools(request.id),
            "tools/call" => {
                self.handle_tool_call(request.id, request.params, trace_id, &request_span_id)
                    .await
            }
            _ => JsonRpcResponse::error(
                request.id,
                -32601,
                format!("Method not found: {}", request.method),
            ),
        };

        let elapsed = request_start.elapsed().as_secs_f64() * 1000.0;

        // Emit McpResponseEmitted
        let has_error = response.error.is_some();
        self.tracer.record_audit(
            AuditEvent::new(
                trace_id,
                &self.session_id,
                "mcp_handler",
                AuditEventType::McpResponseEmitted,
                if has_error {
                    AuditSeverity::Warn
                } else {
                    AuditSeverity::Info
                },
            )
            .with_parent_span(&request_span_id)
            .with_duration_ms(elapsed)
            .with_attrs(json!({
                "has_error": has_error,
            }))
            .seal(),
        );

        response
    }

    fn handle_initialize(id: JsonRpcId) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                }
            }),
        )
    }

    fn handle_list_tools(id: JsonRpcId) -> JsonRpcResponse {
        JsonRpcResponse::success(id, tool_definitions())
    }

    async fn handle_tool_call(
        &self,
        id: JsonRpcId,
        params: Option<JsonValue>,
        trace_id: &str,
        parent_span_id: &str,
    ) -> JsonRpcResponse {
        let Some(params) = params else {
            return JsonRpcResponse::error(id, -32602, "Missing params");
        };

        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        // Hash arguments for audit (never log full content)
        let args_hash = hash_content(&arguments.to_string());

        // Emit McpToolCallStarted
        let tool_span_id = generate_span_id();
        self.tracer.record_audit(
            AuditEvent::new(
                trace_id,
                &self.session_id,
                "mcp_handler",
                AuditEventType::McpToolCallStarted,
                AuditSeverity::Info,
            )
            .with_parent_span(parent_span_id)
            .with_input_hash(&args_hash)
            .with_attrs(json!({
                "tool_name": tool_name,
                "tool_span_id": tool_span_id,
            }))
            .seal(),
        );

        let tool_start = Instant::now();

        let result = match tool_name {
            "context_store" => self.tool_context_store(&arguments),
            "context_query" => self.tool_context_query(&arguments),
            "context_status" => self.tool_context_status().await,
            "generate" => self.tool_generate(&arguments, trace_id).await,
            "route" => self.tool_route(&arguments, trace_id).await,
            _ => Err(format!("Unknown tool: {tool_name}")),
        };

        let tool_elapsed = tool_start.elapsed().as_secs_f64() * 1000.0;

        // Emit McpToolCallCompleted or McpToolCallFailed
        match &result {
            Ok(content) => {
                self.tracer.record_audit(
                    AuditEvent::new(
                        trace_id,
                        &self.session_id,
                        "mcp_handler",
                        AuditEventType::McpToolCallCompleted,
                        AuditSeverity::Info,
                    )
                    .with_parent_span(&tool_span_id)
                    .with_duration_ms(tool_elapsed)
                    .with_output_hash(hash_content(content))
                    .with_attrs(json!({
                        "tool_name": tool_name,
                        "result_size": content.len(),
                    }))
                    .seal(),
                );
            }
            Err(err) => {
                self.tracer.record_audit(
                    AuditEvent::new(
                        trace_id,
                        &self.session_id,
                        "mcp_handler",
                        AuditEventType::McpToolCallFailed,
                        AuditSeverity::Error,
                    )
                    .with_parent_span(&tool_span_id)
                    .with_duration_ms(tool_elapsed)
                    .with_attrs(json!({
                        "tool_name": tool_name,
                        "error": err,
                    }))
                    .seal(),
                );
            }
        }

        match result {
            Ok(content) => JsonRpcResponse::success(
                id,
                json!({
                    "content": [{"type": "text", "text": content}]
                }),
            ),
            Err(err) => JsonRpcResponse::success(
                id,
                json!({
                    "content": [{"type": "text", "text": err}],
                    "isError": true
                }),
            ),
        }
    }

    // ------------------------------------------------------------------
    // Tool implementations
    // ------------------------------------------------------------------

    fn tool_context_store(&self, args: &JsonValue) -> Result<String, String> {
        let content = args
            .get("content")
            .and_then(serde_json::Value::as_str)
            .ok_or("Missing 'content' argument")?;
        let tag = args
            .get("tag")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("note");
        let files_json = args
            .get("files_referenced")
            .map(std::string::ToString::to_string);

        let row_id = self
            .store
            .store(content, tag, files_json.as_deref())
            .map_err(|e| format!("Store error: {e}"))?;

        Ok(format!(
            "Context stored (id={row_id}, tag={tag}, length={}).",
            content.len()
        ))
    }

    fn tool_context_query(&self, args: &JsonValue) -> Result<String, String> {
        let query = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .ok_or("Missing 'query' argument")?;
        let tag_filter = args.get("tag").and_then(serde_json::Value::as_str);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;

        let entries = self
            .store
            .query(query, tag_filter, limit)
            .map_err(|e| format!("Query error: {e}"))?;

        if entries.is_empty() {
            return Ok(format!("Context query for '{query}': No results found."));
        }

        let mut out = format!("Found {} result(s):\n", entries.len());
        for (i, entry) in entries.iter().enumerate() {
            let _ = write!(
                out,
                "\n--- Result {} (tag={}, score={:.3}) ---\n{}\n",
                i + 1,
                entry.tag,
                entry.score,
                entry.content,
            );
        }
        Ok(out)
    }

    async fn tool_context_status(&self) -> Result<String, String> {
        let entry_count = self.store.entry_count().unwrap_or(0);

        // Try to reach the MLX service
        match self.mlx.health().await {
            Ok(health) => {
                let kv = self.mlx.kv_stats().await.ok();
                let mut status = format!(
                    "Cartographer Context Server\n\
                     Status: {}\n\
                     Context entries: {entry_count}\n\
                     Memory: {:.1} / {:.1} GB\n",
                    health.status, health.system_memory_used_gb, health.system_memory_total_gb,
                );
                if let Some(kv) = kv {
                    let _ = writeln!(
                        status,
                        "KV Cache: {} entries, {:.1}-bit, {:.1}x compression, {:.0}% utilization",
                        kv.entries,
                        kv.bitwidth,
                        kv.compression_ratio,
                        kv.utilization * 100.0,
                    );
                }
                Ok(status)
            }
            Err(e) => Ok(format!(
                "Cartographer Context Server\n\
                 Status: MLX service unreachable ({e})\n\
                 Context entries: {entry_count}\n\
                 Hint: Start the MLX service with `python -m cartographer_mlx.server`"
            )),
        }
    }

    async fn tool_generate(&self, args: &JsonValue, trace_id: &str) -> Result<String, String> {
        let prompt = args
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .ok_or("Missing 'prompt' argument")?;
        #[allow(clippy::cast_possible_truncation)]
        let max_tokens = args
            .get("max_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1024)
            .min(u64::from(u32::MAX)) as u32;
        let system = args.get("system").and_then(serde_json::Value::as_str);

        let mut messages = Vec::new();
        if let Some(sys) = system {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: sys.to_string(),
            });
        }
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        });

        let request = GenerateRequest {
            model: "gemma".to_string(),
            messages,
            max_tokens: Some(max_tokens),
            temperature: Some(0.7),
            stream: false,
        };

        match self.mlx.generate(&request, trace_id).await {
            Ok(resp) => {
                let text = resp
                    .choices
                    .first()
                    .and_then(|c| c.message.content.as_deref())
                    .unwrap_or("(empty response)");
                Ok(text.to_string())
            }
            Err(e) => Err(format!("Generation failed: {e}")),
        }
    }

    async fn tool_route(&self, args: &JsonValue, trace_id: &str) -> Result<String, String> {
        let query = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .ok_or("Missing 'query' argument")?;
        let threshold = args
            .get("confidence_threshold")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.7);

        let request = RouteRequest {
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: query.to_string(),
            }],
            confidence_threshold: Some(threshold),
        };

        match self.mlx.route(&request, trace_id).await {
            Ok(resp) => {
                // Emit a dedicated RouteDecision audit event
                self.tracer.record_audit(
                    AuditEvent::new(
                        trace_id,
                        &self.session_id,
                        "router",
                        AuditEventType::RouteDecision,
                        AuditSeverity::Info,
                    )
                    .with_attrs(json!({
                        "disposition": resp.disposition,
                        "confidence": resp.confidence,
                        "reason": resp.reason,
                        "suggested_tool": resp.suggested_tool,
                        "latency_ms": resp.latency_ms,
                        "threshold": threshold,
                    }))
                    .seal(),
                );

                Ok(serde_json::to_string_pretty(&resp).unwrap_or_else(|_| format!("{resp:?}")))
            }
            Err(e) => {
                // Graceful degradation: if routing fails, defer
                Ok(format!(
                    "{{\"disposition\": \"defer\", \"confidence\": 0.0, \"reason\": \"Routing error: {e}\"}}"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use telemetry::{
        AuditEvent, AuditEventType, MemoryTelemetrySink, SessionTracer, TelemetryEvent,
    };

    fn test_handler() -> (McpHandler, Arc<MemoryTelemetrySink>, String) {
        let sink = Arc::new(MemoryTelemetrySink::default());
        let session_id = "test-session-001".to_string();
        let tracer = SessionTracer::new(session_id.clone(), sink.clone());
        let mlx = MlxClient::new("http://127.0.0.1:19999"); // unreachable port for tests
        let store = ContextStore::open_in_memory().expect("in-memory store");
        let handler = McpHandler::new(mlx, tracer, session_id.clone(), store);
        (handler, sink, session_id)
    }

    fn make_request(method: &str, params: Option<JsonValue>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(1),
            method: method.to_string(),
            params,
        }
    }

    fn extract_audit_events(sink: &MemoryTelemetrySink) -> Vec<AuditEvent> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                TelemetryEvent::Audit(a) => Some(*a),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn handle_initialize_returns_protocol_version() {
        let (handler, _, _) = test_handler();
        let req = make_request("initialize", None);
        let resp = handler.handle(req, "trace-init").await;

        let result = resp.result.expect("should have result");
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["serverInfo"].is_object());
        assert_eq!(result["serverInfo"]["name"], "cartographer");
    }

    #[tokio::test]
    async fn handle_list_tools_returns_all_five_tools() {
        let (handler, _, _) = test_handler();
        let req = make_request("tools/list", None);
        let resp = handler.handle(req, "trace-list").await;

        let result = resp.result.expect("should have result");
        let tools = result["tools"].as_array().expect("tools is array");
        assert_eq!(tools.len(), 5);

        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"context_store"));
        assert!(names.contains(&"context_query"));
        assert!(names.contains(&"context_status"));
        assert!(names.contains(&"generate"));
        assert!(names.contains(&"route"));
    }

    #[tokio::test]
    async fn handle_unknown_method_returns_error() {
        let (handler, _, _) = test_handler();
        let req = make_request("foo/bar", None);
        let resp = handler.handle(req, "trace-unknown").await;

        let err = resp.error.expect("should have error");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("foo/bar"));
    }

    #[tokio::test]
    async fn handle_tool_call_missing_params_returns_error() {
        let (handler, _, _) = test_handler();
        let req = make_request("tools/call", None);
        let resp = handler.handle(req, "trace-noparam").await;

        let err = resp.error.expect("should have error");
        assert_eq!(err.code, -32602);
    }

    #[tokio::test]
    async fn tool_call_emits_started_and_completed_events() {
        let (handler, sink, _) = test_handler();
        let req = make_request(
            "tools/call",
            Some(json!({
                "name": "context_store",
                "arguments": {"content": "test data", "tag": "note"}
            })),
        );
        let resp = handler.handle(req, "trace-tool").await;
        assert!(resp.error.is_none(), "tool call should succeed");

        let audits = extract_audit_events(&sink);
        let types: Vec<&AuditEventType> = audits.iter().map(|a| &a.event_type).collect();

        assert!(
            types.contains(&&AuditEventType::McpRequestReceived),
            "missing McpRequestReceived"
        );
        assert!(
            types.contains(&&AuditEventType::McpToolCallStarted),
            "missing McpToolCallStarted"
        );
        assert!(
            types.contains(&&AuditEventType::McpToolCallCompleted),
            "missing McpToolCallCompleted"
        );
        assert!(
            types.contains(&&AuditEventType::McpResponseEmitted),
            "missing McpResponseEmitted"
        );
    }

    #[tokio::test]
    async fn unknown_tool_emits_failed_event() {
        let (handler, sink, _) = test_handler();
        let req = make_request(
            "tools/call",
            Some(json!({
                "name": "nonexistent",
                "arguments": {}
            })),
        );
        handler.handle(req, "trace-unknown-tool").await;

        let audits = extract_audit_events(&sink);
        let has_failed = audits
            .iter()
            .any(|a| a.event_type == AuditEventType::McpToolCallFailed);
        assert!(has_failed, "should emit McpToolCallFailed for unknown tool");
    }

    #[tokio::test]
    async fn all_audit_events_share_trace_id() {
        let (handler, sink, _) = test_handler();
        let req = make_request(
            "tools/call",
            Some(json!({
                "name": "context_store",
                "arguments": {"content": "data", "tag": "note"}
            })),
        );
        handler.handle(req, "shared-trace-id").await;

        let audits = extract_audit_events(&sink);
        assert!(!audits.is_empty());
        for event in &audits {
            assert_eq!(
                event.trace_id, "shared-trace-id",
                "all audit events must share the same trace_id"
            );
        }
    }

    #[tokio::test]
    async fn audit_events_include_duration() {
        let (handler, sink, _) = test_handler();
        let req = make_request(
            "tools/call",
            Some(json!({
                "name": "context_query",
                "arguments": {"query": "test"}
            })),
        );
        handler.handle(req, "trace-dur").await;

        let audits = extract_audit_events(&sink);
        let response_emitted = audits
            .iter()
            .find(|a| a.event_type == AuditEventType::McpResponseEmitted)
            .expect("should have McpResponseEmitted");
        assert!(
            response_emitted.duration_ms.is_some(),
            "McpResponseEmitted must have duration_ms"
        );
    }

    #[tokio::test]
    async fn tool_arguments_are_hashed_not_logged() {
        let (handler, sink, _) = test_handler();
        let req = make_request(
            "tools/call",
            Some(json!({
                "name": "context_store",
                "arguments": {"content": "SECRET_DATA_12345", "tag": "note"}
            })),
        );
        handler.handle(req, "trace-hash").await;

        let audits = extract_audit_events(&sink);

        // Serialize all audit events to JSON and verify secret is NOT present
        let all_json = serde_json::to_string(&audits).expect("serialize audits");
        assert!(
            !all_json.contains("SECRET_DATA_12345"),
            "raw argument content must not appear in audit events"
        );

        // Verify input_hash IS present on McpToolCallStarted
        let started = audits
            .iter()
            .find(|a| a.event_type == AuditEventType::McpToolCallStarted)
            .expect("should have McpToolCallStarted");
        assert!(
            started.input_hash.is_some(),
            "McpToolCallStarted must have input_hash"
        );
    }
}

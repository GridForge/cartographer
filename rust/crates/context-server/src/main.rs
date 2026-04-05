//! Cartographer Context Server — MCP stdio binary.
//!
//! This binary implements the MCP (Model Context Protocol) server that
//! Claude Code connects to via stdio transport. It reads JSON-RPC requests
//! from stdin, dispatches them through the handler, and writes responses
//! to stdout.
//!
//! The server acts as the Rust orchestration layer, bridging Claude Code
//! to the Python MLX Intelligence Service running locally.

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use context_server::inference::MlxClient;
use context_server::mcp_handler::{JsonRpcRequest, McpHandler};
use context_server::store::ContextStore;
use telemetry::{
    generate_trace_id, AuditEvent, AuditEventType, AuditSeverity, JsonlTelemetrySink, SessionTracer,
};

fn main() {
    // Initialize telemetry
    let log_dir = dirs_or_default();
    let sink = match JsonlTelemetrySink::new(log_dir.join("audit.jsonl")) {
        Ok(s) => Arc::new(s) as Arc<dyn telemetry::TelemetrySink>,
        Err(e) => {
            eprintln!("Warning: telemetry sink failed to initialize: {e}");
            Arc::new(telemetry::MemoryTelemetrySink::default()) as Arc<dyn telemetry::TelemetrySink>
        }
    };

    let session_id = generate_trace_id();
    let tracer = SessionTracer::new(session_id.clone(), sink);

    // Emit ServerStartup audit event
    tracer.record_audit(
        AuditEvent::new(
            generate_trace_id(),
            &session_id,
            "context_server",
            AuditEventType::ServerStartup,
            AuditSeverity::Info,
        )
        .with_attrs(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
        }))
        .seal(),
    );

    // Open the persistent context store
    let store_path = log_dir.parent().unwrap_or(&log_dir).join("context.db");
    let store = ContextStore::open(&store_path).unwrap_or_else(|e| {
        eprintln!(
            "Warning: failed to open context store at {}: {e} — falling back to in-memory",
            store_path.display()
        );
        ContextStore::open_in_memory().expect("in-memory context store must succeed")
    });

    // Initialize the MLX client from environment with tracer attached
    let mlx = MlxClient::from_env().with_tracer(tracer.clone(), session_id.clone());
    let handler = McpHandler::new(mlx, tracer.clone(), session_id.clone(), store);

    // Build a single-threaded tokio runtime for async HTTP calls
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build");

    let stdin = io::stdin();
    let stdout = io::stdout();

    // MCP stdio protocol: one JSON-RPC message per line
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Parse the JSON-RPC request
        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to parse JSON-RPC request: {e}");
                continue;
            }
        };

        // Generate a trace_id for this request
        let trace_id = generate_trace_id();

        // Handle the request (may involve async HTTP calls to MLX service)
        let response = rt.block_on(handler.handle(request, &trace_id));

        // Write the response as a single JSON line to stdout
        let response_json = match serde_json::to_string(&response) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("Failed to serialize JSON-RPC response: {e}");
                continue;
            }
        };

        let mut out = stdout.lock();
        if writeln!(out, "{response_json}").is_err() {
            break; // stdout closed
        }
        let _ = out.flush();
    }

    // Emit ServerShutdown audit event
    tracer.record_audit(
        AuditEvent::new(
            generate_trace_id(),
            &session_id,
            "context_server",
            AuditEventType::ServerShutdown,
            AuditSeverity::Info,
        )
        .seal(),
    );
}

/// Return the log directory, defaulting to `~/.local/share/cartographer/logs/`.
fn dirs_or_default() -> std::path::PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        std::path::PathBuf::from(home).join(".local/share/cartographer/logs")
    } else {
        std::path::PathBuf::from("/tmp/cartographer/logs")
    }
}

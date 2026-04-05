# ADR-0003: Cross-Process Trace Propagation

**Status:** accepted
**Date:** 2026-04-04

## Context
Requests flow from Claude Code to the Rust MCP server to the Python MLX service and back. Without explicit correlation, cross-process tracing depends on timestamp inference, which is fragile and non-auditable.

## Decision
UUIDv7 trace_id generated at MCP ingress, propagated via `X-Cartographer-Trace-Id`, `X-Cartographer-Span-Id`, and `X-Cartographer-Session-Id` HTTP headers. Python extracts these via FastAPI middleware into `contextvars.ContextVar`. Both services emit the same trace_id in their audit logs. Session-level grouping via session_id enables multi-request correlation.

## Consequences
Every HTTP request between Rust and Python carries three extra headers. Overhead is negligible. Enables end-to-end request reconstruction across process boundaries without timestamp guessing.

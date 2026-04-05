# ADR-0004: Structured Audit Logging

**Status:** accepted
**Date:** 2026-04-04

## Context
The project demands aerospace-grade engineering log discipline. Every decision must be auditable months later. Standard application logging (text to stderr) is insufficient for post-hoc reconstruction of decision chains.

## Decision
Dual-store architecture: append-only JSONL as source-of-truth audit ledger, SQLite as a reconstructible query index. Every audit event carries mandatory fields: event_id, trace_id, span_id, session_id, timestamp, component, service, event_type, severity. Typed events cover routing decisions, KV operations, MCP tool calls, model lifecycle, and config snapshots. Hash-chaining (prev_event_hash + event_hash) provides tamper evidence. Configurable durability: fsync for decision and failure events, buffered flush for operational events.

## Consequences
More storage than traditional logging, estimated 10-50MB per day. JSONL files need rotation via date-stamped segments. SQLite index needs periodic rebuild. Tradeoff is full reconstructibility of any decision chain, crash-recoverable with integrity verification.

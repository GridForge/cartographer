# ADR-0008: Engineering Log Discipline

**Status:** accepted
**Date:** 2026-04-04

## Context
ADRs record design decisions. Runtime audit logs record system behavior. Neither captures session-level build decisions: why a particular implementation approach was chosen, what the council found, what verification looked like. Without this layer, the reasoning behind the codebase is lost when conversation context compacts.

## Decision
Maintain a structured engineering log at `docs/engineering-log/` with dated session entries. Each entry records: session summary, numbered decisions with context/rationale/alternatives/status, council findings (if invoked), and verification results. ADRs are created for decisions with lasting architectural impact. The engineering log captures the full decision chain including tactical choices that don't warrant an ADR.

## Consequences
Three-layer audit trail: ADRs (design-time), engineering log (build-time), runtime audit ledger (run-time). More documentation overhead per session, but the record is complete and reconstructible. Future sessions can read prior engineering logs to understand not just what was built, but why and what alternatives were rejected.

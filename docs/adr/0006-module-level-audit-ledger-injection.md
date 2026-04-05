# ADR-0006: Module-Level Audit Ledger Injection

**Status:** accepted
**Date:** 2026-04-04

## Context
Python modules (router, kv_manager, engine, inference) need access to the audit ledger to emit events. Options: constructor-based dependency injection, global singleton, or module-level setter function.

## Decision
Each instrumented module exposes a `set_audit_ledger(ledger)` function. The server lifespan calls all setters during startup after creating the `AuditLedger` instance. Modules store the reference in a module-level `_audit_ledger` variable and guard all emit calls with `if _audit_ledger is not None`.

## Consequences
Simpler than threading a ledger parameter through every function signature. Tests call `set_audit_ledger()` directly with a temp-directory ledger. Trade-off: not as pure as constructor DI, but acceptable for a service with a single initialization path. If the service grows multiple entry points, revisit with proper DI.

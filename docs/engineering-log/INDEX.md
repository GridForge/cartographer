# Engineering Log Index

Reverse-chronological. Each entry records session-level decisions, council findings, and verification evidence per ADR-0008.

## Gate Status
**Current**: 2026-04-04 foundation entry — fixes applied, awaiting acceptance. CITADEL log created (investigation phase, implementation pending).

## Entries

| Date | Expedition | Title | Decisions | Council | Validated | Key Outcome |
|------|-----------|-------|-----------|---------|-----------|-------------|
| 2026-04-05 | RAMPART | [CI Integration Tests](expedition-RAMPART.md) | D-020 to D-023 | Yes (2x Judge: 64%→86%) | Pending impl | Fix 5 skipped tests: sandbox config, temp dirs, python setup |
| 2026-04-05 | CITADEL | [CI Pipeline](expedition-CITADEL.md) | D-017 to D-019 | Yes (Codex + 2 Claude specialists + recommendations agent) | Complete | Unified ci.yml, MLX optional deps, CI green |
| 2026-04-04 | BASECAMP + DEEP-STORE | [Foundation Build](2026-04-04-foundation-build.md) | D-001 to D-016 | Yes (3x: audit, persistence, log validation) | FIXES APPLIED | Full audit telemetry, SQLite FTS5 persistence, 78 tests, 10 commandments, FMEA discipline |

## Decision Registry

| ID | Decision | Expedition | ADR | Status |
|----|----------|-----------|-----|--------|
| D-001 | Custom FastAPI over mlx-lm serve | BASECAMP | ADR-0001 | Implemented |
| D-002 | Council-reviewed audit architecture | BASECAMP | ADR-0004 | Implemented |
| D-003 | Module-level audit ledger injection | BASECAMP | ADR-0006 | Implemented |
| D-004 | Placeholder models for development | BASECAMP | ADR-0007 | Configured |
| D-005 | Hash-chaining for tamper evidence | BASECAMP | ADR-0004 | Implemented |
| D-006 | Correlation IDs via HTTP headers | BASECAMP | ADR-0003 | Implemented |
| D-007 | SQLite-only (no JSONL for context) | DEEP-STORE | — | Implemented |
| D-008 | Sync architecture (no Arc/Mutex) | DEEP-STORE | — | Implemented |
| D-009 | Recency scoring formula | DEEP-STORE | — | Implemented |
| D-010 | rusqlite bundled for FTS5 | DEEP-STORE | — | Implemented |
| D-011 | Schema versioning PRAGMA user_version | DEEP-STORE | — | Implemented |
| D-012 | Phase 3 implemented as judge-approved | DEEP-STORE | — | Implemented |
| D-013 | Engineering log closure gate | BASECAMP | — | Encoded |
| D-014 | Design commandments | BASECAMP | — | Encoded |
| D-015 | FMEA discipline | BASECAMP | — | Encoded |
| D-016 | Unlogged impl decisions (council-found) | BASECAMP | — | Logged |
| D-017 | Single unified ci.yml | CITADEL | — | Implemented |
| D-018 | MLX as optional dependency | CITADEL | — | Implemented |
| D-019 | No path filters on CI | CITADEL | — | Implemented |
| D-020 | Fix tests not production code | RAMPART | — | Approved |
| D-021 | Sandbox disabled via test fixture config | RAMPART | — | Approved |
| D-022 | Temp dir PID + atomic counter | RAMPART | — | Approved |
| D-023 | Python setup in Rust CI job | RAMPART | — | Approved |

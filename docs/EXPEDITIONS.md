# Cartographer Expedition Map

The unit of work in Cartographer is an **expedition** — a scoped effort that explores specific territory. Each expedition has its own engineering log entry, FMEA analysis, and council validation gate.

## Expedition Lifecycle

```
Scope → FMEA → Plan → Log → Build → Verify → Validate → Close
```

No expedition begins without an engineering log entry. No log entry closes without council validation.

## Active Map

```
BASECAMP ✓
  └── DEEP-STORE ✓
        └── CITADEL ✓
              └── RAMPART ← you are here (CI integration tests)
                    └── FIRST-LIGHT
                    ├── WATCHTOWER
                    ├── LENS
                    ├── SECOND-SIGHT
                    ├── COMPRESS
                    └── DIRECT-ROUTE
```

## Expedition Registry

| Expedition | Territory | Entry Criteria | Exit Criteria | Status |
|------------|-----------|---------------|---------------|--------|
| BASECAMP | Audit telemetry, MCP server, Python MLX service skeleton, engineering discipline, 10 commandments | None | MCP server responds to JSON-RPC, audit events emit, 51+ tests pass | **Complete** |
| DEEP-STORE | SQLite FTS5 context persistence — Claude Code's memory layer | BASECAMP complete | `context_store` persists to SQLite, `context_query` returns FTS5 results, 8 store tests pass | **Complete** |
| CITADEL | CI pipeline — automated regression detection for full Rust workspace + MLX-free Python tests | DEEP-STORE complete | Green CI on main: fmt + clippy + `cargo test --workspace` + `pytest test_audit.py`. `pyproject.toml` MLX optional. | **Complete** |
| RAMPART | Fix 5 skipped integration tests — sandbox config, temp dir collisions, python setup | CITADEL complete | All `--skip` flags removed from CI. Full `cargo test --workspace` green on ubuntu-latest. | **Next** |
| FIRST-LIGHT | Download models, register MCP in Claude Code settings, validate end-to-end: Claude Code → MCP → Python inference → response | CITADEL complete | Claude Code discovers Cartographer tools. `generate` returns Gemma output. `route` classifies a request. Full round-trip works in a live session. | Planned |
| WATCHTOWER | Background monitoring — file watcher triggers `cargo test`, git tracker polls changes, context budget warns on threshold | FIRST-LIGHT complete | File save triggers test run, results stored in context. `monitor_status` returns last test result and recent git changes. | Planned |
| LENS | Pre-processing pipeline — bundled PreToolUse hook, Gemma summarizes large files, error triage, diff analysis | FIRST-LIGHT complete | Claude Code reads a >500 line file, hook fires, summary prepended to context. Token savings measurable. | Planned |
| SECOND-SIGHT | Council augmentation — `gemma-council.sh`, Gemma reviews with full session context, outputs council JSON schema | FIRST-LIGHT complete | Council invocation dispatches to Gemma alongside Codex. Gemma output references session history that Codex doesn't have. | Planned |
| COMPRESS | TurboQuant KV cache compression — PolarQuant for keys, QJL for values, adaptive bitwidth under memory pressure | FIRST-LIGHT complete | 128K context fits in ~4GB (vs ~24GB uncompressed). KV snapshot/restore works with compressed state. Needle-in-a-haystack passes at 64K. | Planned |
| DIRECT-ROUTE | Rust API integration — `carto --model gemma` for direct CLI inference without Claude Code | FIRST-LIGHT complete | `carto --model gemma "hello"` streams a response from local MLX. Optional expedition. | Planned |

## Dependency Graph

CITADEL → FIRST-LIGHT is the critical path. Everything after FIRST-LIGHT is independent and parallel.

- **CITADEL** before FIRST-LIGHT: CI safety net before adding model deps and MCP registration
- **FIRST-LIGHT** after CITADEL: models + MCP wiring, protected by CI
- Post-FIRST-LIGHT: all independent, all protected by CI
  - WATCHTOWER: test watching, git tracking (core loop doesn't need models)
  - LENS: file summaries via Gemma (needs models)
  - SECOND-SIGHT: session-aware council member (needs models)
  - COMPRESS: TurboQuant KV cache (needs models loaded)
  - DIRECT-ROUTE: CLI use (needs models + Rust API changes)

## Decision Registry

Decisions are globally sequential. See `docs/engineering-log/INDEX.md` for the full registry. Current highest: **D-016**.

## How to Use This Document

1. **At session start**: Read this file to understand the project's current position and what's next
2. **Before starting work**: Identify which expedition you're working on. If it's new, create the engineering log entry first.
3. **After completing an expedition**: Update the status here, update INDEX.md, and ensure council validation has passed
4. **When planning new work**: Add new expeditions to this registry with clear entry/exit criteria before building

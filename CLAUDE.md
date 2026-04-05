# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project identity
Cartographer — local intelligence layer for Claude Code. Gemma 4 26B + FunctionGemma 270M via MLX on Apple Silicon. Python handles inference, Rust handles MCP orchestration.

## Detected stack
- Languages: Rust, Python.
- Rust workspace: `rust/` (10 crates including `context-server`, `telemetry`, `api`, `runtime`, `tools`, `plugins`, `commands`, `cartographer-cli`, `compat-harness`, `mock-gemma-service`)
- Python service: `python/cartographer_mlx/` (FastAPI MLX inference service)

## Verification

**Local (before any commit):**
- Rust: `cd rust && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- Python: `cd python && pip install -e ".[all]" && python -m pytest tests/ -v`

**CI (GitHub Actions, ubuntu-latest):**
- Rust: fmt + clippy + `cargo test --workspace` (full, no skips)
- Python: `pip install ".[dev]"` (no MLX) + `pytest tests/test_audit.py -v` (MLX-free tests only)

CI does not test MLX inference paths (requires Apple Silicon). MLX validation is local-only, recorded in the engineering log.

## Repository shape
- `rust/` — Rust workspace: MCP context server, telemetry, CLI, runtime
- `python/cartographer_mlx/` — MLX inference service (FastAPI, Gemma models, KV cache management)
- `docs/EXPEDITIONS.md` — **Read this first.** The project roadmap — all planned and completed expeditions with entry/exit criteria
- `docs/adr/` — Architecture Decision Records (numbered, immutable once accepted)
- `docs/engineering-log/` — Session-level build decisions, council findings, verification evidence
- `src/` — Python porting workspace (legacy, compatibility)
- `python/tests/` — Python test suite (audit, middleware, KV manager, schemas)
- `tests/` — legacy validation surfaces

## Engineering log discipline (MANDATORY)

This project follows aerospace-grade engineering log discipline (ADR-0008). Three-layer audit trail:

1. **ADRs** (`docs/adr/NNNN-*.md`) — lasting architectural decisions. Create when a choice has cross-session impact.
2. **Engineering log** (`docs/engineering-log/YYYY-MM-DD-*.md`) — session-level decisions, council findings, verification results. **Every session that makes non-trivial changes MUST have an engineering log entry.**
3. **Runtime audit ledger** (`~/.local/share/cartographer/logs/audit.jsonl`) — automated, emitted by the context-server and MLX service.

### Session protocol
1. **At session start**: Read `docs/engineering-log/INDEX.md`, then the latest entry. Check: is the prior log closed?
2. **HARD GATE**: If the prior log is not closed (missing verification, missing council validation, open items not listed), **close it before doing anything else**. No new implementation work until the gate passes.
3. **Before implementing**: Log decisions with rationale and alternatives considered. If the council was invoked, record findings and verdicts.
4. **After implementing**: Record verification results (test counts, clippy status, smoke test evidence).
5. **Open items**: List what remains undone at the end of each log entry.
6. **Council validation**: Before a log entry is considered closed, submit it to the council for adversarial accuracy verification. The council checks: do claimed test counts match? Do claimed file changes exist? Are decisions accurately described? Are there unlogged decisions?

### Engineering log entry format
Use the template at `docs/engineering-log/template.md`. Each entry has:
- Session summary (2-3 sentences)
- Numbered decisions (D-NNN) with: context, decision, rationale, alternatives rejected, status
- Council findings (if invoked): composition, verdict, score, key findings
- Verification results: test counts, clippy, smoke test evidence
- Open items: what carries forward to the next session

### Decision numbering
Decisions are numbered sequentially across all entries (D-001, D-002, ...). Check the latest entry for the current highest number before adding new ones.

## Working agreement
- Prefer small, reviewable changes and keep generated bootstrap files aligned with actual repo workflows.
- Keep shared defaults in `.claude.json`; reserve `.claude/settings.local.json` for machine-local overrides.
- Do not overwrite existing `CLAUDE.md` content automatically; update it intentionally when repo workflows change.
- **Log before you build.** Record the decision in the engineering log before writing the implementation code.
- **Test everything.** Both Rust and Python test suites must pass. Smoke test the MCP server after changes to context-server.
- **Council for significant decisions.** Invoke the council (Codex + Claude sub-agent) for architectural choices, persistence design, or anything with cross-session impact. Record the findings.

# Cartographer

A local intelligence layer for Claude Code. Persistent memory, background monitoring, and pre-processing — powered by Gemma 4 and FunctionGemma on Apple Silicon.

## What it does

Claude Code's context window compacts over long sessions, losing detail. Cartographer fixes this by running local models that persist memory, pre-process files, and monitor your project — all exposed to Claude Code via MCP.

**The experience:** You sit down, Claude already knows what you're doing, and it stays knowing for as long as you need it to.

## Architecture

```
Claude Code (Opus)
    │
    ├── MCP stdio ──► context-server (Rust)
    │                    ├── HTTP ──► MLX Service (Python)
    │                    │              ├── Gemma 4 26B-A4B (generation)
    │                    │              └── FunctionGemma 270M (routing)
    │                    ├── SQLite FTS5 (context persistence)
    │                    └── Audit ledger (JSONL, hash-chained)
    │
    └── Council ──► Codex (read-only review)
                ──► Gemini (web research)
```

**Python** handles inference (MLX-native, Apple Silicon). **Rust** handles MCP orchestration, persistence, and audit telemetry.

## Key capabilities

| Capability | How it works | Expedition |
|-----------|-------------|-----------|
| **Context persistence** | SQLite FTS5 store survives compaction. Claude Code queries past decisions, errors, test results. | DEEP-STORE |
| **Audit telemetry** | 16 typed events, hash-chaining, cross-process correlation via HTTP headers. Every decision auditable. | BASECAMP |
| **FunctionGemma routing** | 270M model classifies requests as local/remote/augment/defer. Admits uncertainty rather than guessing. | Planned (FIRST-LIGHT) |
| **Background monitoring** | File watcher triggers tests, git tracker polls changes, context budget warns on threshold. | Planned (WATCHTOWER) |
| **Pre-processing** | Gemma summarizes large files before Claude Code reads them. Token budget goes 5x further. | Planned (LENS) |
| **KV cache compression** | TurboQuant (PolarQuant + QJL) at 3 bits — 256K context in ~2.6GB. | Planned (COMPRESS) |

## Repository layout

```
├── rust/                          # Rust workspace (10 crates)
│   └── crates/
│       ├── context-server/        # MCP server — 5 tools, SQLite FTS5, audit
│       ├── telemetry/             # Audit events, hash-chaining, session tracing
│       ├── cartographer-cli/      # CLI binary (carto)
│       ├── api/                   # API client (Anthropic/OpenAI/XAI)
│       ├── runtime/               # Session, config, permissions, bash, hooks
│       ├── tools/                 # Built-in tool implementations
│       ├── plugins/               # Plugin system + hook runner
│       ├── commands/              # Slash command registry
│       ├── compat-harness/        # Upstream manifest extraction
│       └── mock-gemma-service/    # Deterministic mock for parity tests
├── python/
│   └── cartographer_mlx/          # MLX inference service (FastAPI)
│       ├── server.py              # Unified entry point
│       ├── engine.py              # Model lifecycle
│       ├── inference.py           # Generation pipeline
│       ├── router.py              # FunctionGemma dispatch
│       ├── kv_manager.py          # KV cache as managed resource
│       ├── audit.py               # Structured audit ledger
│       └── middleware.py          # Correlation ID propagation
├── docs/
│   ├── EXPEDITIONS.md             # Project roadmap
│   ├── adr/                       # Architecture Decision Records (8)
│   └── engineering-log/           # Session-level decision logs (D-001 to D-024)
└── .github/workflows/ci.yml       # CI: Rust + Python on ubuntu-latest
```

## Verification

```bash
# Rust
cd rust && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace

# Python (MLX-free tests — runs on any platform)
cd python && pip install ".[dev]" && python -m pytest tests/ -v
```

## Engineering discipline

This project follows aerospace-grade engineering log discipline (ADR-0008):
- **10 design commandments** governing all decisions
- **FMEA analysis** before every expedition
- **Council reviews** (Codex + Claude sub-agents) for architectural decisions
- **Adversarial Judge** scoring implementation plans at 95%+ confidence threshold
- **Engineering logs** with numbered decisions (D-001 to D-024), council findings, and verification evidence

See `docs/EXPEDITIONS.md` for the full roadmap and `docs/engineering-log/` for the decision trail.

## Target hardware

MacBook Pro M4 Max, 48GB unified RAM. Memory budget with TurboQuant:

| Component | Memory |
|-----------|--------|
| Gemma 4 26B-A4B Q4 | ~14 GB |
| FunctionGemma 270M Q4 | ~150 MB |
| KV cache 256K (TurboQuant 3-bit) | ~2.6 GB |
| **Total** | **~21 GB** |
| **Headroom** | **~27 GB** |

## License

MIT

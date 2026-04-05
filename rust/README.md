# Cartographer — Rust Workspace

MCP context server, CLI runtime, audit telemetry, and tool execution for the Cartographer intelligence layer.

## Crates

| Crate | Purpose |
|-------|---------|
| **context-server** | MCP stdio server — 5 tools, SQLite FTS5 persistence, audit event emission |
| **telemetry** | Audit events (16 types), hash-chaining, session tracing, JSONL sink |
| **cartographer-cli** | CLI binary (`carto`) — REPL, one-shot prompt, streaming display |
| **api** | HTTP client — Anthropic/OpenAI/XAI providers, SSE streaming |
| **runtime** | Session management, config, permissions, bash execution, hooks, compaction |
| **tools** | Built-in tool implementations — Bash, ReadFile, WriteFile, EditFile, Glob, Grep, Web |
| **plugins** | Plugin system — registration, lifecycle, hook runner (PreToolUse/PostToolUse) |
| **commands** | Slash command registry and parsing |
| **compat-harness** | Upstream manifest extraction |
| **mock-gemma-service** | Deterministic mock for end-to-end parity tests |

## Quick start

```bash
cargo build --release

# Run the MCP context server (Claude Code connects via stdio)
./target/release/cartographer-context-server

# Run the CLI
./target/release/carto
```

## Verification

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All tests run on both macOS and Linux (ubuntu-latest CI).

## License

See repository root.

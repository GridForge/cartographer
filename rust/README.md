# Cartographer — Rust Implementation

A high-performance Rust harness runtime for Gemma model orchestration. Built for speed, safety, and native tool execution.

## Quick Start

```bash
cd rust/
cargo build --release

# Run interactive REPL
./target/release/carto

# One-shot prompt
./target/release/carto prompt "explain this codebase"

# With specific model
./target/release/carto --model sonnet prompt "fix the bug in main.rs"
```

## Mock parity harness

The workspace includes a deterministic mock service and a clean-environment CLI harness for end-to-end parity checks.

```bash
cd rust/

# Run the scripted clean-environment harness
./scripts/run_mock_parity_harness.sh

# Or start the mock service manually
cargo run -p mock-gemma-service -- --bind 127.0.0.1:0
```

## Workspace Layout

```
rust/
├── Cargo.toml              # Workspace root
├── Cargo.lock
└── crates/
    ├── api/                # API client + SSE streaming
    ├── commands/           # Shared slash-command registry
    ├── compat-harness/     # Manifest extraction harness
    ├── mock-gemma-service/ # Deterministic local mock service
    ├── runtime/            # Session, config, permissions, prompts
    ├── cartographer-cli/   # Main CLI binary (`carto`)
    ├── telemetry/          # Telemetry and request profiling
    └── tools/              # Built-in tool implementations
```

### Crate Responsibilities

- **api** — HTTP client, SSE stream parser, request/response types, auth
- **commands** — Slash command definitions and help text generation
- **compat-harness** — Extracts tool/prompt manifests from upstream source
- **mock-gemma-service** — Deterministic `/v1/messages` mock for CLI parity tests
- **runtime** — Agentic loop, config hierarchy, session persistence, permission policy, system prompt assembly
- **cartographer-cli** — REPL, one-shot prompt, streaming display, tool call rendering, CLI argument parsing
- **tools** — Tool specs + execution: Bash, ReadFile, WriteFile, EditFile, GlobSearch, GrepSearch, WebSearch, WebFetch, Agent, and more

## CLI Flags

```
carto [OPTIONS] [COMMAND]

Options:
  --model MODEL                    Set the model (alias or full name)
  --dangerously-skip-permissions   Skip all permission checks
  --permission-mode MODE           Set read-only, workspace-write, or danger-full-access
  --allowedTools TOOLS             Restrict enabled tools
  --output-format FORMAT           Output format (text or json)
  --version, -V                    Print version info

Commands:
  prompt <text>      One-shot prompt (non-interactive)
  login              Authenticate via OAuth
  logout             Clear stored credentials
  init               Initialize project config
```

## Stats

- **~20K lines** of Rust
- **Binary name:** `carto`
- **Default permissions:** `danger-full-access`

## License

See repository root.

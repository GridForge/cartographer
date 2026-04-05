# Cartographer

An experimental harness for Gemma 4, combining TurboQuant KV cache compression and FunctionGemma tool-call dispatch on Apple Silicon.

## What is this?

Cartographer is a research vehicle for novel harness engineering — exploring what's possible when you wire together open models, quantization techniques, and agentic tool systems on local hardware.

**Target hardware:** MacBook Pro M4 Max, 48GB unified RAM

**Key technologies:**
- **Gemma 4 26B MoE** — 3.8B active parameters, 128K context
- **TurboQuant** — KV cache compression (~3 bits/value, 6x memory reduction)
- **FunctionGemma** — 270M parameter function-calling dispatcher

## Repository Layout

```text
.
├── rust/               # Rust workspace — CLI and runtime crates
│   └── crates/
│       ├── api/                # API client + SSE streaming
│       ├── cartographer-cli/   # Main CLI binary (carto)
│       ├── commands/           # Slash command registry
│       ├── compat-harness/     # Manifest extraction harness
│       ├── mock-gemma-service/ # Deterministic mock service
│       ├── plugins/            # Plugin system
│       ├── runtime/            # Session, config, permissions, prompts
│       ├── telemetry/          # Telemetry and request profiling
│       └── tools/              # Built-in tool implementations
├── src/                # Python porting workspace
└── tests/              # Python verification
```

## Quick Start

```bash
cd rust/
cargo build --release
./target/release/carto
```

## License

MIT

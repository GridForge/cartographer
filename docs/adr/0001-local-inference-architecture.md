# ADR-0001: Local Inference Architecture

**Status:** accepted
**Date:** 2026-04-04

## Context
MLX is Python-native with no stable Rust bindings. Cartographer needs both Gemma 4 26B for generation and FunctionGemma 270M for routing, both running on Apple Silicon unified memory. A single-language solution would mean either sacrificing MLX performance or rewriting the MCP server in Python.

## Decision
Python FastAPI process for MLX inference, loading both models in a single process. Rust binary for the MCP stdio server. The two processes communicate via HTTP on localhost. The Python service exposes an OpenAI-compatible generation API plus custom endpoints for routing classification and KV cache management.

## Consequences
Two-process architecture adds cross-process correlation complexity. MLX zero-copy unified memory means both models share the 48GB pool naturally without duplication. No Rust FFI fragility. HTTP client patterns are well-established and reusable across both ecosystems.

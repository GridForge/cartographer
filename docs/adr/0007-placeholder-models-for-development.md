# ADR-0007: Placeholder Models for Development

**Status:** accepted
**Date:** 2026-04-04

## Context
Gemma 4 26B-A4B and FunctionGemma 270M may not yet be available as pre-quantized MLX models. The pipeline needs working defaults to enable end-to-end testing without blocking on model availability.

## Decision
Use `google/gemma-3-4b-it` as the generation placeholder and `mlx-community/gemma-3-1b-it-4bit` as the dispatch placeholder. Model IDs are constants in `engine.py` (`GENERATION_MODEL_ID`, `DISPATCH_MODEL_ID`) with default HuggingFace paths in `DEFAULT_MODEL_PATHS`. Local paths under `~/.local/share/cartographer/models/` take priority if present.

## Consequences
Enables full pipeline testing (MCP → Rust → Python → inference → audit) without the target models. Placeholder models are smaller and faster, useful for development iteration. Swap to production models by placing quantized weights in the local models directory — no code changes needed.

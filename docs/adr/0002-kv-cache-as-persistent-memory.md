# ADR-0002: KV Cache as Persistent Memory

**Status:** accepted
**Date:** 2026-04-04

## Context
Every other local inference tool treats KV cache as opaque and ephemeral. For a context persistence layer, the cache is the memory. TurboQuant compression (3-bit) makes 256K context fit in approximately 2.6GB on unified memory.

## Decision
KV cache is explicitly managed with snapshot, restore, evict, and compress operations. The Rust orchestration layer owns policy (when to compress, evict, snapshot). The Python service owns mechanism. Adaptive bitwidth: 3.5-bit default, dropping to 3.0 or 2.5 under memory pressure.

## Consequences
More complex than fire-and-forget caching. Requires audit logging of every cache mutation. Enables session persistence across process restarts, a capability no other local inference tool provides.

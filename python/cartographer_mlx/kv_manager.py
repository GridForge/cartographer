"""KV cache as a managed resource: allocate, compress, snapshot, restore, evict.

The KV cache is not an implementation detail — it's persistent memory. This
module exposes cache lifecycle operations that the Rust orchestration layer
uses to implement policy (when to compress, evict, snapshot).

Phase 1 provides the interface and basic stats tracking. TurboQuant
integration (PolarQuant keys + QJL values + adaptive bitwidth) will be
added when the turboquant_mlx package is integrated.
"""

from __future__ import annotations

import json
import logging
import os
import time
from dataclasses import dataclass, field
from pathlib import Path

from .audit import AuditEvent, AuditLedger
from .schemas import KVStats

logger = logging.getLogger(__name__)

# Module-level reference to the audit ledger, set by server lifespan.
_audit_ledger: AuditLedger | None = None


def set_audit_ledger(ledger: AuditLedger) -> None:
    """Called by server to inject the audit ledger."""
    global _audit_ledger
    _audit_ledger = ledger


def _try_trace_id() -> str:
    """Best-effort trace ID from context var (empty for background ops)."""
    try:
        from .middleware import get_trace_context
        return get_trace_context().trace_id
    except Exception:
        return ""


def _state_snapshot(state: "KVCacheState") -> dict:
    """Capture KV state as a plain dict for audit attrs."""
    return {
        "entries": state.entries,
        "total_tokens": state.total_tokens,
        "bitwidth": state.bitwidth,
        "memory_bytes": state.memory_bytes,
    }


@dataclass
class KVCacheState:
    """Tracks the state of the KV cache for a model."""

    entries: int = 0
    total_tokens: int = 0
    bitwidth: float = 16.0  # start at full precision, compress as needed
    target_bitwidth: float = 3.5  # TurboQuant target
    memory_bytes: int = 0
    max_memory_bytes: int = 0

    # Adaptive bitwidth thresholds (fraction of max_memory)
    _pressure_thresholds: dict[float, float] = field(
        default_factory=lambda: {
            0.9: 2.5,  # >90% memory → drop to 2.5-bit
            0.8: 3.0,  # >80% memory → drop to 3-bit
            0.7: 3.5,  # >70% memory → 3.5-bit (default TurboQuant)
        }
    )


class KVManager:
    """Manages KV cache lifecycle for a model.

    Phase 1: Stats tracking, snapshot/restore interface, eviction.
    Phase 2 (future): TurboQuant integration for actual compression.
    """

    def __init__(self, store_dir: str | None = None) -> None:
        self._state = KVCacheState()
        self._store_dir = Path(
            store_dir
            or os.environ.get(
                "CONTEXT_STORE_PATH",
                os.path.expanduser("~/.local/share/cartographer/context"),
            )
        )
        self._store_dir.mkdir(parents=True, exist_ok=True)

    def stats(self) -> KVStats:
        """Return current KV cache statistics."""
        utilization = (
            self._state.memory_bytes / self._state.max_memory_bytes
            if self._state.max_memory_bytes > 0
            else 0.0
        )
        compression = (
            16.0 / self._state.bitwidth if self._state.bitwidth > 0 else 1.0
        )
        return KVStats(
            entries=self._state.entries,
            total_tokens=self._state.total_tokens,
            bitwidth=self._state.bitwidth,
            compression_ratio=compression,
            memory_bytes=self._state.memory_bytes,
            max_memory_bytes=self._state.max_memory_bytes,
            utilization=utilization,
        )

    def update_stats(self, entries: int, total_tokens: int, memory_bytes: int) -> None:
        """Update cache stats after generation. Called by inference pipeline."""
        pre = _state_snapshot(self._state)
        t0 = time.monotonic()

        self._state.entries = entries
        self._state.total_tokens = total_tokens
        self._state.memory_bytes = memory_bytes

        duration_ms = (time.monotonic() - t0) * 1000
        post = _state_snapshot(self._state)

        if _audit_ledger is not None:
            _audit_ledger.emit(AuditEvent(
                event_type="KvCacheOperation",
                severity="info",
                component="kv_manager",
                trace_id=_try_trace_id(),
                duration_ms=duration_ms,
                attrs={
                    "operation": "update_stats",
                    "trigger": "inference_pipeline",
                    "pre_state": pre,
                    "post_state": post,
                },
            ))

    def set_max_memory(self, max_bytes: int) -> None:
        """Set the memory budget for KV cache."""
        self._state.max_memory_bytes = max_bytes

    def check_pressure(self) -> float:
        """Check memory pressure and return recommended bitwidth.

        The Rust orchestration layer calls this to decide whether to
        trigger compression. Returns the recommended bitwidth.
        """
        pre = _state_snapshot(self._state)
        t0 = time.monotonic()

        if self._state.max_memory_bytes == 0:
            result = self._state.target_bitwidth
        else:
            utilization = self._state.memory_bytes / self._state.max_memory_bytes
            result = self._state.target_bitwidth
            for threshold, bitwidth in sorted(
                self._state._pressure_thresholds.items(), reverse=True
            ):
                if utilization >= threshold:
                    result = bitwidth
                    break

        duration_ms = (time.monotonic() - t0) * 1000

        if _audit_ledger is not None:
            _audit_ledger.emit(AuditEvent(
                event_type="KvCacheOperation",
                severity="info",
                component="kv_manager",
                trace_id=_try_trace_id(),
                duration_ms=duration_ms,
                attrs={
                    "operation": "check_pressure",
                    "trigger": "orchestration_query",
                    "recommended_bitwidth": result,
                    "pre_state": pre,
                    "post_state": pre,  # check_pressure is read-only
                },
            ))

        return result

    def snapshot(self, path: str | None = None) -> str:
        """Snapshot current KV cache state to disk.

        Phase 1: Saves metadata only (actual KV tensor serialization
        requires TurboQuant integration in Phase 2).
        Returns the path where the snapshot was saved.
        """
        pre = _state_snapshot(self._state)
        t0 = time.monotonic()

        cache_dir = self._store_dir / "cache"
        cache_dir.mkdir(parents=True, exist_ok=True)

        snapshot_path = path or str(cache_dir / "kv_state.json")
        metadata = {
            "entries": self._state.entries,
            "total_tokens": self._state.total_tokens,
            "bitwidth": self._state.bitwidth,
            "memory_bytes": self._state.memory_bytes,
        }
        with open(snapshot_path, "w") as f:
            json.dump(metadata, f, indent=2)

        duration_ms = (time.monotonic() - t0) * 1000
        logger.info("KV cache snapshot saved to %s", snapshot_path)

        if _audit_ledger is not None:
            _audit_ledger.emit(AuditEvent(
                event_type="KvCacheOperation",
                severity="info",
                component="kv_manager",
                trace_id=_try_trace_id(),
                duration_ms=duration_ms,
                attrs={
                    "operation": "snapshot",
                    "trigger": "manual_request",
                    "snapshot_path": snapshot_path,
                    "pre_state": pre,
                    "post_state": pre,  # snapshot is read-only
                },
            ))

        return snapshot_path

    def restore(self, path: str) -> bool:
        """Restore KV cache state from a snapshot.

        Phase 1: Restores metadata only.
        Returns True if successful.
        """
        pre = _state_snapshot(self._state)
        t0 = time.monotonic()

        try:
            with open(path) as f:
                metadata = json.load(f)
            self._state.entries = metadata.get("entries", 0)
            self._state.total_tokens = metadata.get("total_tokens", 0)
            self._state.bitwidth = metadata.get("bitwidth", 16.0)
            self._state.memory_bytes = metadata.get("memory_bytes", 0)

            duration_ms = (time.monotonic() - t0) * 1000
            post = _state_snapshot(self._state)
            logger.info("KV cache state restored from %s", path)

            if _audit_ledger is not None:
                _audit_ledger.emit(AuditEvent(
                    event_type="KvCacheOperation",
                    severity="info",
                    component="kv_manager",
                    trace_id=_try_trace_id(),
                    duration_ms=duration_ms,
                    attrs={
                        "operation": "restore",
                        "trigger": "manual_request",
                        "restore_path": path,
                        "pre_state": pre,
                        "post_state": post,
                    },
                ))

            return True
        except Exception as exc:
            duration_ms = (time.monotonic() - t0) * 1000
            logger.warning("Failed to restore KV cache from %s: %s", path, exc)

            if _audit_ledger is not None:
                _audit_ledger.emit(AuditEvent(
                    event_type="KvCacheOperation",
                    severity="error",
                    component="kv_manager",
                    trace_id=_try_trace_id(),
                    duration_ms=duration_ms,
                    attrs={
                        "operation": "restore",
                        "trigger": "manual_request",
                        "restore_path": path,
                        "error": str(exc),
                        "pre_state": pre,
                        "post_state": pre,
                    },
                ))

            return False

    def evict(self, count: int) -> int:
        """Evict the oldest N entries from the cache.

        Phase 1: Updates stats only (actual KV tensor eviction requires
        direct cache manipulation in Phase 2).
        Returns the number of entries evicted.
        """
        pre = _state_snapshot(self._state)
        t0 = time.monotonic()

        evicted = min(count, self._state.entries)
        self._state.entries -= evicted
        # Rough estimate: reduce memory proportionally
        if self._state.entries > 0:
            ratio = self._state.entries / (self._state.entries + evicted)
            self._state.memory_bytes = int(self._state.memory_bytes * ratio)
        else:
            self._state.memory_bytes = 0

        duration_ms = (time.monotonic() - t0) * 1000
        post = _state_snapshot(self._state)
        logger.info("Evicted %d KV cache entries", evicted)

        if _audit_ledger is not None:
            _audit_ledger.emit(AuditEvent(
                event_type="KvCacheOperation",
                severity="info",
                component="kv_manager",
                trace_id=_try_trace_id(),
                duration_ms=duration_ms,
                attrs={
                    "operation": "evict",
                    "trigger": "manual_request",
                    "requested_count": count,
                    "evicted_count": evicted,
                    "pre_state": pre,
                    "post_state": post,
                },
            ))

        return evicted

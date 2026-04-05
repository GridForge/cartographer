"""Aerospace-grade audit event system.

Every decision, state transition, and failure emits an immutable structured
event to a JSONL audit ledger. Events are correlated via trace_id across
the Rust MCP server and Python MLX service boundary.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import threading
import time
import uuid
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)


def _uuidv7_ish() -> str:
    """Generate a time-ordered UUID: millisecond timestamp prefix + uuid4 suffix."""
    ts_ms = int(time.time() * 1000)
    ts_hex = f"{ts_ms:012x}"
    rand_hex = uuid.uuid4().hex[12:]
    raw = ts_hex + rand_hex
    # Format as UUID: 8-4-4-4-12
    return f"{raw[:8]}-{raw[8:12]}-{raw[12:16]}-{raw[16:20]}-{raw[20:32]}"


def generate_trace_id() -> str:
    """Generate a new trace ID (time-ordered UUID)."""
    return _uuidv7_ish()


def generate_span_id() -> str:
    """Generate a new span ID (16 hex chars)."""
    return uuid.uuid4().hex[:16]


def hash_content(s: str) -> str:
    """SHA256 hex digest of a string for content fingerprinting."""
    return hashlib.sha256(s.encode("utf-8")).hexdigest()


@dataclass
class AuditEvent:
    """Immutable structured event matching the Rust-side schema."""

    event_type: str
    severity: str  # "info", "warn", "error"
    component: str  # "router", "engine", "inference", "kv_manager", "server"
    attrs: dict[str, Any] = field(default_factory=dict)

    # Identifiers — populated by emit() if not set
    event_id: str = ""
    trace_id: str = ""
    span_id: str = ""
    parent_span_id: str = ""
    session_id: str = ""

    # Timing
    timestamp_utc: str = ""
    duration_ms: float = 0.0

    # Invariants
    service: str = "python-mlx"
    config_snapshot_id: str = ""
    input_hash: str = ""
    output_hash: str = ""

    # Computed on emit
    event_hash: str = ""


class AuditLedger:
    """Append-only JSONL writer for audit events.

    Writes to ~/.local/share/cartographer/logs/python-mlx-YYYY-MM-DD.jsonl
    Thread-safe via a lock. Flushes after every write for durability.
    """

    def __init__(self, log_dir: str | None = None) -> None:
        self._log_dir = Path(
            log_dir
            or os.environ.get(
                "CARTOGRAPHER_AUDIT_LOG_DIR",
                os.path.expanduser("~/.local/share/cartographer/logs"),
            )
        )
        self._log_dir.mkdir(parents=True, exist_ok=True)
        self._lock = threading.Lock()
        self._current_date: str = ""
        self._file = None
        self._event_count = 0

    def _ensure_file(self) -> None:
        """Open or rotate the log file based on current date."""
        today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
        if today != self._current_date:
            if self._file is not None:
                self._file.close()
            path = self._log_dir / f"python-mlx-{today}.jsonl"
            self._file = open(path, "a", encoding="utf-8")
            self._current_date = today

    def emit(self, event: AuditEvent) -> None:
        """Serialize and append an event to the ledger. Flushes after every write."""
        # Fill in defaults
        if not event.event_id:
            event.event_id = _uuidv7_ish()
        if not event.timestamp_utc:
            event.timestamp_utc = datetime.now(timezone.utc).isoformat()

        # Build canonical JSON for hashing (without event_hash itself)
        record = asdict(event)
        record.pop("event_hash", None)
        canonical = json.dumps(record, sort_keys=True, separators=(",", ":"))
        event.event_hash = hashlib.sha256(canonical.encode("utf-8")).hexdigest()

        # Final record with hash
        record["event_hash"] = event.event_hash
        line = json.dumps(record, separators=(",", ":"))

        with self._lock:
            try:
                self._ensure_file()
                assert self._file is not None
                self._file.write(line + "\n")
                self._file.flush()
                self._event_count += 1
            except Exception:
                logger.exception("Failed to write audit event")

    def close(self) -> None:
        """Close the underlying file."""
        with self._lock:
            if self._file is not None:
                self._file.close()
                self._file = None

    @property
    def event_count(self) -> int:
        return self._event_count

    @property
    def log_path(self) -> str:
        today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
        return str(self._log_dir / f"python-mlx-{today}.jsonl")

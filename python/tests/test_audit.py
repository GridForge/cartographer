"""Tests for the Cartographer Python audit system.

Covers: AuditEvent, AuditLedger, hash_content, ID generation,
CorrelationMiddleware, TraceContext, KVManager audit integration,
and schema field presence.
"""

from __future__ import annotations

import json
import re
import threading
from datetime import datetime, timezone

import pytest

from cartographer_mlx.audit import (
    AuditEvent,
    AuditLedger,
    generate_span_id,
    generate_trace_id,
    hash_content,
)
from cartographer_mlx.middleware import CorrelationMiddleware, get_trace_context
from cartographer_mlx.kv_manager import KVManager, set_audit_ledger
from cartographer_mlx.schemas import HealthResponse


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _ledger_lines(tmp_path) -> list[dict]:
    """Read all JSONL lines from the ledger file in tmp_path."""
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    path = tmp_path / f"python-mlx-{today}.jsonl"
    lines = path.read_text().strip().splitlines()
    return [json.loads(line) for line in lines]


def _make_event(**kwargs) -> AuditEvent:
    defaults = dict(event_type="TestEvent", severity="info", component="test")
    defaults.update(kwargs)
    return AuditEvent(**defaults)


# ===================================================================
# Tier 1: Unit Tests
# ===================================================================


class TestAuditEvent:
    def test_audit_event_has_mandatory_fields(self, tmp_path):
        """Create event, emit to ledger, verify event_id, timestamp, type, severity, component set."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        event = _make_event()
        ledger.emit(event)
        ledger.close()

        records = _ledger_lines(tmp_path)
        assert len(records) == 1
        r = records[0]
        assert r["event_id"] != ""
        assert r["timestamp_utc"] != ""
        assert r["event_type"] == "TestEvent"
        assert r["severity"] == "info"
        assert r["component"] == "test"

    def test_audit_event_serialization_roundtrip(self):
        """Create event, to_dict-equivalent via dataclasses, json roundtrip."""
        from dataclasses import asdict

        event = _make_event(attrs={"key": "value"})
        # Populate fields that emit() would fill
        event.event_id = "test-id"
        event.timestamp_utc = "2026-01-01T00:00:00+00:00"

        d = asdict(event)
        dumped = json.dumps(d)
        loaded = json.loads(dumped)

        assert loaded["event_type"] == "TestEvent"
        assert loaded["severity"] == "info"
        assert loaded["component"] == "test"
        assert loaded["attrs"] == {"key": "value"}
        assert loaded["event_id"] == "test-id"
        assert loaded["timestamp_utc"] == "2026-01-01T00:00:00+00:00"

    def test_audit_event_ids_are_unique(self, tmp_path):
        """100 events, all event_ids unique."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        for _ in range(100):
            ledger.emit(_make_event())
        ledger.close()

        records = _ledger_lines(tmp_path)
        ids = [r["event_id"] for r in records]
        assert len(set(ids)) == 100

    def test_audit_event_timestamp_is_iso8601(self, tmp_path):
        """Verify timestamp matches ISO 8601 pattern."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        ledger.emit(_make_event())
        ledger.close()

        records = _ledger_lines(tmp_path)
        ts = records[0]["timestamp_utc"]
        # Should match YYYY-MM-DDTHH:MM:SS (with optional fractional seconds and timezone)
        assert re.match(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}", ts)


class TestHashContent:
    def test_hash_content_is_deterministic(self):
        """Same input same hash."""
        assert hash_content("hello") == hash_content("hello")

    def test_hash_content_different_inputs(self):
        """Different input different hash."""
        assert hash_content("hello") != hash_content("world")

    def test_hash_content_returns_64_char_hex(self):
        """SHA-256 = 64 hex chars."""
        h = hash_content("test string")
        assert len(h) == 64
        assert re.fullmatch(r"[0-9a-f]{64}", h)


class TestIDGeneration:
    def test_trace_ids_are_unique(self):
        """100 trace IDs, no dupes."""
        ids = {generate_trace_id() for _ in range(100)}
        assert len(ids) == 100

    def test_span_ids_are_unique(self):
        """100 span IDs, no dupes."""
        ids = {generate_span_id() for _ in range(100)}
        assert len(ids) == 100

    def test_trace_id_looks_like_uuid(self):
        """Contains hyphens, roughly right length."""
        tid = generate_trace_id()
        assert "-" in tid
        # UUID format: 8-4-4-4-12 = 32 hex + 4 hyphens = 36 chars
        assert len(tid) == 36


# ===================================================================
# Tier 2: Integration Tests
# ===================================================================


class TestAuditLedger:
    def test_ledger_writes_jsonl(self, tmp_path):
        """Create ledger, emit 5 events, read file, verify 5 valid JSON lines."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        for i in range(5):
            ledger.emit(_make_event(attrs={"index": i}))
        ledger.close()

        records = _ledger_lines(tmp_path)
        assert len(records) == 5

    def test_ledger_flush_on_every_write(self, tmp_path):
        """Emit 1 event, read file WITHOUT closing ledger, verify event present."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        ledger.emit(_make_event())
        # Do NOT close — the flush should have written already
        records = _ledger_lines(tmp_path)
        assert len(records) == 1
        ledger.close()

    def test_ledger_events_are_valid_json(self, tmp_path):
        """Emit 3 events with different types, verify each line parseable with mandatory fields."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        for etype in ["Alpha", "Beta", "Gamma"]:
            ledger.emit(_make_event(event_type=etype))
        ledger.close()

        records = _ledger_lines(tmp_path)
        assert len(records) == 3
        for r in records:
            assert "event_id" in r
            assert "timestamp_utc" in r
            assert "event_type" in r
            assert "severity" in r
            assert "component" in r
            assert "event_hash" in r

    def test_ledger_event_count_tracks(self, tmp_path):
        """Emit 7 events, verify ledger.event_count == 7."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        for _ in range(7):
            ledger.emit(_make_event())
        assert ledger.event_count == 7
        ledger.close()

    def test_ledger_thread_safety(self, tmp_path):
        """10 threads each emit 5 events = 50 total."""
        ledger = AuditLedger(log_dir=str(tmp_path))

        def worker():
            for _ in range(5):
                ledger.emit(_make_event())

        threads = [threading.Thread(target=worker) for _ in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert ledger.event_count == 50
        records = _ledger_lines(tmp_path)
        assert len(records) == 50
        ledger.close()


class TestMiddleware:
    @pytest.mark.asyncio
    async def test_middleware_generates_trace_id_when_missing(self):
        """Send request without headers, verify X-Cartographer-Trace-Id in response."""
        import httpx
        from fastapi import FastAPI

        test_app = FastAPI()
        test_app.add_middleware(CorrelationMiddleware)

        @test_app.get("/ping")
        async def ping():
            return {"ok": True}

        transport = httpx.ASGITransport(app=test_app)
        async with httpx.AsyncClient(transport=transport, base_url="http://test") as client:
            resp = await client.get("/ping")

        assert resp.status_code == 200
        trace_id = resp.headers.get("x-cartographer-trace-id")
        assert trace_id is not None and len(trace_id) > 0

    @pytest.mark.asyncio
    async def test_middleware_propagates_existing_trace_id(self):
        """Send with X-Cartographer-Trace-Id header, verify same value returned."""
        import httpx
        from fastapi import FastAPI

        test_app = FastAPI()
        test_app.add_middleware(CorrelationMiddleware)

        @test_app.get("/ping")
        async def ping():
            return {"ok": True}

        transport = httpx.ASGITransport(app=test_app)
        async with httpx.AsyncClient(transport=transport, base_url="http://test") as client:
            resp = await client.get("/ping", headers={"X-Cartographer-Trace-Id": "my-trace-123"})

        assert resp.headers.get("x-cartographer-trace-id") == "my-trace-123"

    @pytest.mark.asyncio
    async def test_middleware_propagates_session_id(self):
        """Send with X-Cartographer-Session-Id, verify propagated."""
        import httpx
        from fastapi import FastAPI

        test_app = FastAPI()
        test_app.add_middleware(CorrelationMiddleware)

        @test_app.get("/ping")
        async def ping():
            return {"ok": True}

        transport = httpx.ASGITransport(app=test_app)
        async with httpx.AsyncClient(transport=transport, base_url="http://test") as client:
            resp = await client.get("/ping", headers={"X-Cartographer-Session-Id": "sess-abc"})

        assert resp.headers.get("x-cartographer-session-id") == "sess-abc"


class TestTraceContext:
    def test_get_trace_context_returns_defaults_outside_request(self):
        """Call get_trace_context() outside request, verify no crash, returns empty defaults."""
        ctx = get_trace_context()
        assert ctx.trace_id == ""
        assert ctx.span_id == ""
        assert ctx.parent_span_id == ""
        assert ctx.session_id == ""


# ===================================================================
# Tier 3: Adversarial Tests
# ===================================================================


class TestLedgerAdversarial:
    def test_ledger_handles_unicode_content(self, tmp_path):
        """Emit event with Unicode attrs, verify correct serialization."""
        ledger = AuditLedger(log_dir=str(tmp_path))
        text = "Hello \u4e16\u754c \U0001f30d \u00e9\u00e8\u00ea"
        ledger.emit(_make_event(attrs={"text": text}))
        ledger.close()

        records = _ledger_lines(tmp_path)
        assert records[0]["attrs"]["text"] == text

    def test_ledger_handles_large_attrs(self, tmp_path):
        """10KB attrs dict, verify it writes correctly."""
        large_value = "x" * 10_000
        ledger = AuditLedger(log_dir=str(tmp_path))
        ledger.emit(_make_event(attrs={"big": large_value}))
        ledger.close()

        records = _ledger_lines(tmp_path)
        assert len(records[0]["attrs"]["big"]) == 10_000

    def test_ledger_handles_nested_attrs(self, tmp_path):
        """5-level nested dict in attrs."""
        nested = {"l1": {"l2": {"l3": {"l4": {"l5": "deep"}}}}}
        ledger = AuditLedger(log_dir=str(tmp_path))
        ledger.emit(_make_event(attrs=nested))
        ledger.close()

        records = _ledger_lines(tmp_path)
        assert records[0]["attrs"]["l1"]["l2"]["l3"]["l4"]["l5"] == "deep"


class TestKVManagerAudit:
    def _setup_kv(self, tmp_path):
        """Create a KVManager with an injected audit ledger."""
        ledger = AuditLedger(log_dir=str(tmp_path / "logs"))
        set_audit_ledger(ledger)
        kv = KVManager(store_dir=str(tmp_path / "store"))
        return kv, ledger

    def _audit_records(self, tmp_path):
        today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
        path = tmp_path / "logs" / f"python-mlx-{today}.jsonl"
        if not path.exists():
            return []
        lines = path.read_text().strip().splitlines()
        return [json.loads(line) for line in lines]

    def test_kv_snapshot_emits_audit_event(self, tmp_path):
        """Call snapshot(), verify KvCacheOperation event with operation=snapshot."""
        kv, ledger = self._setup_kv(tmp_path)
        kv.snapshot()
        ledger.close()

        records = self._audit_records(tmp_path)
        assert len(records) >= 1
        snap_events = [r for r in records if r["attrs"].get("operation") == "snapshot"]
        assert len(snap_events) == 1
        assert snap_events[0]["event_type"] == "KvCacheOperation"

    def test_kv_evict_records_pre_post_state(self, tmp_path):
        """Set state, evict, verify pre_state != post_state in audit event."""
        kv, ledger = self._setup_kv(tmp_path)
        # Give the KV some state first
        kv.update_stats(entries=10, total_tokens=500, memory_bytes=1024)
        kv.evict(5)
        ledger.close()

        records = self._audit_records(tmp_path)
        evict_events = [r for r in records if r["attrs"].get("operation") == "evict"]
        assert len(evict_events) == 1
        e = evict_events[0]
        assert e["attrs"]["pre_state"]["entries"] == 10
        assert e["attrs"]["post_state"]["entries"] == 5
        assert e["attrs"]["pre_state"] != e["attrs"]["post_state"]

    def test_kv_check_pressure_emits_audit_event(self, tmp_path):
        """Call check_pressure, verify audit event emitted (read-only op still logged)."""
        kv, ledger = self._setup_kv(tmp_path)
        kv.check_pressure()
        ledger.close()

        records = self._audit_records(tmp_path)
        pressure_events = [r for r in records if r["attrs"].get("operation") == "check_pressure"]
        assert len(pressure_events) == 1
        assert pressure_events[0]["event_type"] == "KvCacheOperation"


class TestSchemas:
    def test_health_response_includes_both_metrics(self):
        """HealthResponse has gen_metrics AND route_metrics fields."""
        hr = HealthResponse()
        assert hasattr(hr, "gen_metrics")
        assert hasattr(hr, "route_metrics")
        # Verify they are InferenceMetrics instances
        assert hr.gen_metrics.total_requests == 0
        assert hr.route_metrics.total_requests == 0

    def test_health_response_includes_audit_fields(self):
        """HealthResponse has audit_ledger_events and audit_ledger_path."""
        hr = HealthResponse()
        assert hasattr(hr, "audit_ledger_events")
        assert hasattr(hr, "audit_ledger_path")
        assert hr.audit_ledger_events == 0
        assert hr.audit_ledger_path == ""

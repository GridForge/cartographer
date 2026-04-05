"""FastAPI app — unified entry point, middleware, lifecycle.

The Cartographer MLX Intelligence Service. Serves both Gemma 4 (generation)
and FunctionGemma (routing) via a single FastAPI process on unified memory.

Run with:
    python -m cartographer_mlx.server
    # or
    uvicorn cartographer_mlx.server:app --host 127.0.0.1 --port 8101
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import resource
import sys
import time
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse

from .audit import AuditEvent, AuditLedger, hash_content
from .engine import DISPATCH_MODEL_ID, GENERATION_MODEL_ID, ModelEngine
from .engine import set_audit_ledger as set_engine_audit_ledger
from .inference import generate_response, generate_stream
from .inference import set_audit_ledger as set_inference_audit_ledger
from .kv_manager import KVManager
from .kv_manager import set_audit_ledger as set_kv_audit_ledger
from .metrics import MetricsCollector
from .middleware import CorrelationMiddleware
from .router import route_request
from .router import set_audit_ledger as set_router_audit_ledger
from .schemas import (
    ChatCompletionRequest,
    ChatCompletionResponse,
    HealthResponse,
    KVEvictRequest,
    KVRestoreRequest,
    KVSnapshotRequest,
    KVStats,
    RouteRequest,
    RouteResponse,
)

# ---------------------------------------------------------------------------
# Structured JSON logging to stderr
# ---------------------------------------------------------------------------


class _JSONFormatter(logging.Formatter):
    """Emit one JSON object per log line to stderr."""

    def format(self, record: logging.LogRecord) -> str:
        doc = {
            "ts": self.formatTime(record, datefmt="%Y-%m-%dT%H:%M:%S.%fZ"),
            "level": record.levelname,
            "logger": record.name,
            "msg": record.getMessage(),
        }
        if record.exc_info and record.exc_info[0] is not None:
            doc["exc"] = self.formatException(record.exc_info)
        return json.dumps(doc, default=str)


_handler = logging.StreamHandler(sys.stderr)
_handler.setFormatter(_JSONFormatter())
logging.root.handlers = [_handler]
logging.root.setLevel(logging.INFO)

logger = logging.getLogger("cartographer.mlx")

# ---------------------------------------------------------------------------
# Globals (initialized in lifespan)
# ---------------------------------------------------------------------------
engine: ModelEngine | None = None
kv_manager: KVManager | None = None
gen_metrics: MetricsCollector | None = None
route_metrics: MetricsCollector | None = None
audit_ledger: AuditLedger | None = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Load models on startup, clean up on shutdown."""
    global engine, kv_manager, gen_metrics, route_metrics, audit_ledger

    # Initialize audit ledger first so all subsequent events are captured
    audit_ledger = AuditLedger()

    # Inject audit ledger into all instrumented modules
    set_engine_audit_ledger(audit_ledger)
    set_inference_audit_ledger(audit_ledger)
    set_kv_audit_ledger(audit_ledger)
    set_router_audit_ledger(audit_ledger)

    engine = ModelEngine()
    kv_manager = KVManager()
    gen_metrics = MetricsCollector()
    route_metrics = MetricsCollector()

    t0 = time.monotonic()

    # Set KV cache memory budget (leave ~4GB for OS + other processes)
    total_mem = _system_memory_bytes()
    kv_budget = int(total_mem * 0.5)  # 50% of RAM for KV cache
    kv_manager.set_max_memory(kv_budget)
    logger.info(
        "System memory: %.1f GB, KV budget: %.1f GB",
        total_mem / 1e9,
        kv_budget / 1e9,
    )

    # Load models — generation first (larger), then dispatch
    load_gen = os.environ.get("CARTOGRAPHER_SKIP_GEN_MODEL", "") != "1"
    load_dispatch = os.environ.get("CARTOGRAPHER_SKIP_DISPATCH_MODEL", "") != "1"

    if load_gen:
        gen_path = os.environ.get("CARTOGRAPHER_GEN_MODEL")
        try:
            engine.load_model(GENERATION_MODEL_ID, gen_path)
        except Exception as exc:
            logger.error("Failed to load generation model: %s", exc)
            logger.info("Generation endpoints will return 503")

    if load_dispatch:
        dispatch_path = os.environ.get("CARTOGRAPHER_DISPATCH_MODEL")
        try:
            engine.load_model(DISPATCH_MODEL_ID, dispatch_path)
        except Exception as exc:
            logger.error("Failed to load dispatch model: %s", exc)
            logger.info("Routing endpoints will return 503")

    startup_ms = (time.monotonic() - t0) * 1000

    # Emit ServerStartup audit event
    audit_ledger.emit(AuditEvent(
        event_type="ServerStartup",
        severity="info",
        component="server",
        duration_ms=startup_ms,
        attrs={
            "port": os.environ.get("PORT", "8101"),
            "host": os.environ.get("HOST", "127.0.0.1"),
            "system_memory_bytes": total_mem,
            "kv_budget_bytes": kv_budget,
            "models_requested": {
                "generation": load_gen,
                "dispatch": load_dispatch,
            },
        },
    ))

    # Emit ConfigSnapshot with loaded model details
    model_configs = {}
    for mid in [GENERATION_MODEL_ID, DISPATCH_MODEL_ID]:
        m = engine.get_model(mid)
        if m is not None:
            model_configs[mid] = {
                "path": m.path,
                "param_count": m.param_count,
                "quantization": m.quantization,
                "memory_estimate_bytes": m.memory_estimate_bytes,
            }

    config_snapshot = json.dumps(model_configs, sort_keys=True)
    audit_ledger.emit(AuditEvent(
        event_type="ConfigSnapshot",
        severity="info",
        component="server",
        config_snapshot_id=hash_content(config_snapshot),
        attrs={
            "models": model_configs,
            "kv_budget_bytes": kv_budget,
            "system_memory_bytes": total_mem,
        },
    ))

    logger.info("Cartographer MLX service ready on port %s", os.environ.get("PORT", "8101"))
    yield

    # Cleanup
    logger.info("Shutting down Cartographer MLX service")

    audit_ledger.emit(AuditEvent(
        event_type="ServerShutdown",
        severity="info",
        component="server",
    ))

    if engine:
        for model_id in list(engine.loaded_models()):
            engine.unload_model(model_id)

    audit_ledger.close()


app = FastAPI(
    title="Cartographer MLX Intelligence Service",
    version="0.1.0",
    lifespan=lifespan,
)

# Add correlation ID middleware
app.add_middleware(CorrelationMiddleware)


# ---------------------------------------------------------------------------
# Generation endpoints (OpenAI-compatible)
# ---------------------------------------------------------------------------

@app.post("/v1/chat/completions", response_model=None)
async def chat_completions(request: ChatCompletionRequest):
    """OpenAI-compatible chat completion endpoint using Gemma 4."""
    assert engine is not None and gen_metrics is not None
    model = engine.get_model(GENERATION_MODEL_ID)
    if model is None:
        raise HTTPException(503, "Generation model not loaded")

    if request.stream:
        return StreamingResponse(
            generate_stream(model, request, gen_metrics),
            media_type="text/event-stream",
        )

    # Run synchronous generation in thread pool to not block event loop
    loop = asyncio.get_event_loop()
    response = await loop.run_in_executor(
        None, generate_response, model, request, gen_metrics
    )
    return response


# ---------------------------------------------------------------------------
# Routing endpoint (FunctionGemma dispatch)
# ---------------------------------------------------------------------------

@app.post("/v1/route", response_model=RouteResponse)
async def route(request: RouteRequest):
    """Classify a request using FunctionGemma dispatch model."""
    assert engine is not None
    model = engine.get_model(DISPATCH_MODEL_ID)
    if model is None:
        # Graceful degradation: if dispatch model isn't loaded, always defer
        return RouteResponse(
            disposition="defer",
            confidence=0.0,
            reason="Dispatch model not loaded",
        )

    loop = asyncio.get_event_loop()
    return await loop.run_in_executor(None, route_request, model, request)


# ---------------------------------------------------------------------------
# KV cache management
# ---------------------------------------------------------------------------

@app.get("/v1/kv/stats", response_model=KVStats)
async def kv_stats():
    """Current KV cache statistics."""
    assert kv_manager is not None
    return kv_manager.stats()


@app.post("/v1/kv/snapshot")
async def kv_snapshot(request: KVSnapshotRequest):
    """Snapshot KV cache to disk."""
    assert kv_manager is not None
    path = kv_manager.snapshot(request.path)
    return {"status": "ok", "path": path}


@app.post("/v1/kv/restore")
async def kv_restore(request: KVRestoreRequest):
    """Restore KV cache from disk."""
    assert kv_manager is not None
    ok = kv_manager.restore(request.path)
    if not ok:
        raise HTTPException(400, "Failed to restore KV cache")
    return {"status": "ok"}


@app.post("/v1/kv/evict")
async def kv_evict(request: KVEvictRequest):
    """Evict oldest N entries from KV cache."""
    assert kv_manager is not None
    evicted = kv_manager.evict(request.count)
    return {"status": "ok", "evicted": evicted}


# ---------------------------------------------------------------------------
# Health and metrics
# ---------------------------------------------------------------------------

@app.get("/v1/health", response_model=HealthResponse)
async def health():
    """Health check with model status, cache stats, and inference metrics."""
    assert engine is not None and kv_manager is not None
    assert gen_metrics is not None and route_metrics is not None

    total_mem = _system_memory_bytes()
    used_mem = _used_memory_bytes()

    models = []
    for model_id in [GENERATION_MODEL_ID, DISPATCH_MODEL_ID]:
        models.append(engine.model_status(model_id))

    return HealthResponse(
        status="ok",
        models=models,
        kv_cache=kv_manager.stats(),
        gen_metrics=gen_metrics.snapshot(),
        route_metrics=route_metrics.snapshot(),
        system_memory_used_gb=used_mem / 1e9,
        system_memory_total_gb=total_mem / 1e9,
        audit_ledger_events=audit_ledger.event_count if audit_ledger else 0,
        audit_ledger_path=audit_ledger.log_path if audit_ledger else "",
    )


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _system_memory_bytes() -> int:
    """Total system memory in bytes."""
    try:
        import subprocess
        result = subprocess.run(
            ["sysctl", "-n", "hw.memsize"],
            capture_output=True, text=True, timeout=2,
        )
        return int(result.stdout.strip())
    except Exception:
        return 48 * 1024 * 1024 * 1024  # fallback: 48 GB


def _used_memory_bytes() -> int:
    """Approximate memory used by this process."""
    try:
        usage = resource.getrusage(resource.RUSAGE_SELF)
        return usage.ru_maxrss  # macOS: bytes
    except Exception:
        return 0


def main():
    """Entry point for `cartographer-mlx` CLI command."""
    import uvicorn

    port = int(os.environ.get("PORT", "8101"))
    host = os.environ.get("HOST", "127.0.0.1")
    logger.info("Starting Cartographer MLX service on %s:%d", host, port)
    uvicorn.run(
        "cartographer_mlx.server:app",
        host=host,
        port=port,
        log_level="info",
    )


if __name__ == "__main__":
    main()

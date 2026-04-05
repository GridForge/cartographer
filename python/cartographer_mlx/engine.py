"""Model registry: load, unload, health per model.

Manages the lifecycle of MLX models. Each model is loaded once and kept
resident in unified memory. The engine owns the model weights and tokenizer;
inference pipelines consume them by reference.
"""

from __future__ import annotations

import json
import logging
import os
import time
from dataclasses import dataclass, field
from pathlib import Path

import mlx.core as mx
from mlx_lm import load as mlx_load
from mlx_lm.utils import generate_step

from .audit import AuditEvent, AuditLedger, hash_content
from .schemas import ModelStatus

logger = logging.getLogger(__name__)

DEFAULT_MODELS_DIR = Path(
    os.environ.get(
        "CARTOGRAPHER_MODELS_DIR",
        os.path.expanduser("~/.local/share/cartographer/models"),
    )
)

# Well-known model IDs that the engine recognizes.
GENERATION_MODEL_ID = "gemma-gen"
DISPATCH_MODEL_ID = "gemma-dispatch"

# Default HuggingFace paths (used if local directory doesn't exist yet).
DEFAULT_MODEL_PATHS: dict[str, str] = {
    GENERATION_MODEL_ID: "google/gemma-3-4b-it",  # placeholder until Gemma 4 26B available
    DISPATCH_MODEL_ID: "mlx-community/gemma-3-1b-it-4bit",  # placeholder until FunctionGemma available
}

# Module-level reference to the audit ledger, set by server lifespan.
_audit_ledger: AuditLedger | None = None


def set_audit_ledger(ledger: AuditLedger) -> None:
    """Called by server to inject the audit ledger."""
    global _audit_ledger
    _audit_ledger = ledger


@dataclass
class LoadedModel:
    """A model that has been loaded into MLX memory."""

    model_id: str
    model: object  # mlx_lm model
    tokenizer: object  # mlx_lm tokenizer
    path: str = ""
    param_count: str = ""
    quantization: str = "Q4"
    memory_estimate_bytes: int = 0


class ModelEngine:
    """Registry of loaded MLX models.

    Call ``load_model`` to bring a model into memory, then access it via
    ``get_model``.  The engine is intentionally synchronous — MLX operations
    are GPU-bound and the FastAPI server wraps calls in ``run_in_executor``
    when needed.
    """

    def __init__(self, models_dir: Path | None = None) -> None:
        self._models: dict[str, LoadedModel] = {}
        self._models_dir = models_dir or DEFAULT_MODELS_DIR

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    def load_model(self, model_id: str, path: str | None = None) -> LoadedModel:
        """Load a model from *path* (local dir or HF repo) into MLX memory."""
        if model_id in self._models:
            logger.info("Model %s already loaded, skipping", model_id)
            return self._models[model_id]

        resolve_path = path or self._resolve_path(model_id)
        logger.info("Loading model %s from %s …", model_id, resolve_path)

        t0 = time.monotonic()
        model, tokenizer = mlx_load(resolve_path)
        load_duration_ms = (time.monotonic() - t0) * 1000

        param_count_str = self._count_params(model)

        # Estimate memory after load
        try:
            import resource
            memory_bytes = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        except Exception:
            memory_bytes = 0

        loaded = LoadedModel(
            model_id=model_id,
            model=model,
            tokenizer=tokenizer,
            path=resolve_path,
            param_count=param_count_str,
            quantization="Q4",
            memory_estimate_bytes=memory_bytes,
        )
        self._models[model_id] = loaded
        logger.info("Model %s loaded (%s params) in %.1f ms", model_id, loaded.param_count, load_duration_ms)

        # Emit ModelLifecycle audit event
        if _audit_ledger is not None:
            _audit_ledger.emit(AuditEvent(
                event_type="ModelLifecycle",
                severity="info",
                component="engine",
                duration_ms=load_duration_ms,
                attrs={
                    "action": "load",
                    "model_id": model_id,
                    "path": resolve_path,
                    "param_count": param_count_str,
                    "quantization": "Q4",
                    "load_duration_ms": load_duration_ms,
                    "memory_bytes": memory_bytes,
                },
            ))

            # Emit ConfigSnapshot for this model
            config = {
                "model_id": model_id,
                "path": resolve_path,
                "param_count": param_count_str,
                "quantization": "Q4",
                "memory_bytes": memory_bytes,
            }
            config_json = json.dumps(config, sort_keys=True)
            _audit_ledger.emit(AuditEvent(
                event_type="ConfigSnapshot",
                severity="info",
                component="engine",
                config_snapshot_id=hash_content(config_json),
                attrs=config,
            ))

        return loaded

    def unload_model(self, model_id: str) -> None:
        """Remove a model from memory."""
        if model_id in self._models:
            t0 = time.monotonic()
            loaded = self._models[model_id]
            del self._models[model_id]
            mx.metal.clear_cache()
            duration_ms = (time.monotonic() - t0) * 1000
            logger.info("Model %s unloaded", model_id)

            if _audit_ledger is not None:
                _audit_ledger.emit(AuditEvent(
                    event_type="ModelLifecycle",
                    severity="info",
                    component="engine",
                    duration_ms=duration_ms,
                    attrs={
                        "action": "unload",
                        "model_id": model_id,
                        "path": loaded.path,
                        "param_count": loaded.param_count,
                    },
                ))

    def get_model(self, model_id: str) -> LoadedModel | None:
        return self._models.get(model_id)

    def loaded_models(self) -> list[str]:
        return list(self._models.keys())

    # ------------------------------------------------------------------
    # Status
    # ------------------------------------------------------------------

    def model_status(self, model_id: str) -> ModelStatus:
        loaded = self._models.get(model_id)
        if loaded is None:
            return ModelStatus(name=model_id, loaded=False)
        return ModelStatus(
            name=model_id,
            loaded=True,
            parameters=loaded.param_count,
            quantization=loaded.quantization,
            memory_bytes=loaded.memory_estimate_bytes,
        )

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _resolve_path(self, model_id: str) -> str:
        """Resolve a model_id to a loadable path.

        Priority:
        1. Local directory under models_dir
        2. Default HuggingFace path (triggers download on first use)
        """
        local = self._models_dir / model_id
        if local.is_dir():
            return str(local)
        return DEFAULT_MODEL_PATHS.get(model_id, model_id)

    @staticmethod
    def _count_params(model: object) -> str:
        try:
            total = sum(p.size for _, p in mx.utils.tree_flatten(model.parameters()))
            if total >= 1_000_000_000:
                return f"{total / 1_000_000_000:.1f}B"
            return f"{total / 1_000_000:.0f}M"
        except Exception:
            return "unknown"

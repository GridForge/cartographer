"""Cartographer MLX Intelligence Service — local inference for Gemma 4 + FunctionGemma."""

__version__ = "0.1.0"

from .audit import AuditEvent, AuditLedger, generate_span_id, generate_trace_id, hash_content
from .middleware import CorrelationMiddleware, TraceContext, get_trace_context

__all__ = [
    "AuditEvent",
    "AuditLedger",
    "CorrelationMiddleware",
    "TraceContext",
    "generate_span_id",
    "generate_trace_id",
    "get_trace_context",
    "hash_content",
]

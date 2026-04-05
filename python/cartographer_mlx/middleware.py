"""Correlation ID propagation middleware.

Extracts trace/span/session IDs from incoming request headers set by the
Rust MCP server, stores them in contextvars for use by any code in the
request path, and returns them in the response headers.
"""

from __future__ import annotations

import contextvars
from dataclasses import dataclass

from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response

from .audit import generate_span_id, generate_trace_id

# Context variables — accessible anywhere in the request call chain.
_trace_id_var: contextvars.ContextVar[str] = contextvars.ContextVar("trace_id", default="")
_span_id_var: contextvars.ContextVar[str] = contextvars.ContextVar("span_id", default="")
_parent_span_id_var: contextvars.ContextVar[str] = contextvars.ContextVar("parent_span_id", default="")
_session_id_var: contextvars.ContextVar[str] = contextvars.ContextVar("session_id", default="")

# Header names matching the Rust side.
HEADER_TRACE_ID = "X-Cartographer-Trace-Id"
HEADER_SPAN_ID = "X-Cartographer-Span-Id"
HEADER_SESSION_ID = "X-Cartographer-Session-Id"


@dataclass
class TraceContext:
    """Snapshot of the current trace context."""
    trace_id: str
    span_id: str
    parent_span_id: str
    session_id: str


def get_trace_context() -> TraceContext:
    """Return the current trace context from contextvars.

    Safe to call from any code in the request path.  Returns empty strings
    for fields that are not set (e.g., background tasks not in a request).
    """
    return TraceContext(
        trace_id=_trace_id_var.get(),
        span_id=_span_id_var.get(),
        parent_span_id=_parent_span_id_var.get(),
        session_id=_session_id_var.get(),
    )


class CorrelationMiddleware(BaseHTTPMiddleware):
    """Extract or generate correlation IDs for every request."""

    async def dispatch(self, request: Request, call_next) -> Response:
        # Extract from headers, generate if missing
        trace_id = request.headers.get(HEADER_TRACE_ID) or generate_trace_id()
        parent_span_id = request.headers.get(HEADER_SPAN_ID) or ""
        session_id = request.headers.get(HEADER_SESSION_ID) or ""

        # This request gets its own span
        span_id = generate_span_id()

        # Set context vars
        t1 = _trace_id_var.set(trace_id)
        t2 = _span_id_var.set(span_id)
        t3 = _parent_span_id_var.set(parent_span_id)
        t4 = _session_id_var.set(session_id)

        try:
            response: Response = await call_next(request)
        finally:
            # Reset context vars
            _trace_id_var.reset(t1)
            _span_id_var.reset(t2)
            _parent_span_id_var.reset(t3)
            _session_id_var.reset(t4)

        # Propagate headers back to caller
        response.headers[HEADER_TRACE_ID] = trace_id
        response.headers[HEADER_SPAN_ID] = span_id
        if session_id:
            response.headers[HEADER_SESSION_ID] = session_id

        return response

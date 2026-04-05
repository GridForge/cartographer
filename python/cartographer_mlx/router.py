"""FunctionGemma dispatch logic: classify, confidence score, defer.

The router uses the dispatch model (FunctionGemma 270M) to classify
incoming messages and decide whether they should be handled locally,
sent to the remote frontier model, or augmented with local preprocessing.

Key principle: a 270M model that admits uncertainty is more valuable than
one that guesses. The "defer" disposition means "I don't know, let the
frontier model decide."
"""

from __future__ import annotations

import json
import logging
import time

import mlx.core as mx
from mlx_lm.utils import generate_step

from .audit import AuditEvent, AuditLedger, hash_content
from .engine import LoadedModel
from .middleware import get_trace_context
from .schemas import (
    ChatMessage,
    RouteDisposition,
    RouteRequest,
    RouteResponse,
    ToolDefinition,
)

logger = logging.getLogger(__name__)

# System prompt that turns FunctionGemma into a routing classifier.
ROUTER_SYSTEM_PROMPT = """\
You are a routing classifier. Given a user message and available tools, \
classify how it should be handled. Respond with ONLY a JSON object:
{
  "disposition": "local" | "remote" | "augment" | "defer",
  "confidence": 0.0-1.0,
  "reason": "brief explanation",
  "suggested_tool": "tool_name or null"
}

Dispositions:
- "local": Can be handled by local tools (file reads, context lookups, summaries)
- "remote": Requires frontier model (complex reasoning, code generation, architecture)
- "augment": Local preprocessing helps, then send to frontier model
- "defer": Uncertain — let the frontier model decide (ALWAYS prefer this over a bad guess)

Be conservative. When in doubt, use "defer"."""

# Module-level reference to the audit ledger, set by server lifespan.
_audit_ledger: AuditLedger | None = None


def set_audit_ledger(ledger: AuditLedger) -> None:
    """Called by server to inject the audit ledger."""
    global _audit_ledger
    _audit_ledger = ledger


def route_request(
    dispatch_model: LoadedModel,
    request: RouteRequest,
) -> RouteResponse:
    """Classify a request using the dispatch model."""
    t0 = time.monotonic()
    ctx = get_trace_context()

    # Build the routing prompt
    messages = [
        {"role": "user", "content": _build_routing_prompt(request)},
    ]

    prompt = dispatch_model.tokenizer.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True
    )
    input_tokens = mx.array(dispatch_model.tokenizer.encode(prompt))
    input_token_count = input_tokens.size

    # Generate with low temperature for deterministic classification
    generated = []
    for token, _ in generate_step(
        prompt=input_tokens,
        model=dispatch_model.model,
        temp=0.1,
        top_p=0.9,
    ):
        tok_id = token.item()
        if tok_id == dispatch_model.tokenizer.eos_token_id:
            break
        generated.append(tok_id)
        if len(generated) >= 256:  # routing should be short
            break

    raw_output = dispatch_model.tokenizer.decode(generated).strip()
    latency_ms = (time.monotonic() - t0) * 1000

    # Parse the JSON response
    response = _parse_route_response(raw_output, request.confidence_threshold, latency_ms)

    # Emit audit event
    if _audit_ledger is not None:
        # Determine if threshold changed the result
        threshold_applied = False
        parsed_disposition = response.disposition.value
        try:
            cleaned = raw_output
            if "```" in cleaned:
                cleaned = cleaned.split("```")[1]
                if cleaned.startswith("json"):
                    cleaned = cleaned[4:]
                cleaned = cleaned.strip()
            start = cleaned.find("{")
            end = cleaned.rfind("}") + 1
            if start >= 0 and end > start:
                parsed_json = json.loads(cleaned[start:end])
                original_disp = parsed_json.get("disposition", "defer")
                parsed_confidence = float(parsed_json.get("confidence", 0.0))
                if original_disp != "defer" and parsed_confidence < request.confidence_threshold:
                    threshold_applied = True
                parsed_disposition = original_disp
        except Exception:
            parsed_disposition = "defer"
            parsed_confidence = 0.0

        _audit_ledger.emit(AuditEvent(
            event_type="RouteDecision",
            severity="info",
            component="router",
            trace_id=ctx.trace_id,
            span_id=ctx.span_id,
            parent_span_id=ctx.parent_span_id,
            session_id=ctx.session_id,
            duration_ms=latency_ms,
            input_hash=hash_content(prompt),
            output_hash=hash_content(raw_output),
            attrs={
                "raw_output_hash": hash_content(raw_output),
                "parsed_disposition": parsed_disposition,
                "parsed_confidence": response.confidence,
                "threshold": request.confidence_threshold,
                "threshold_applied": threshold_applied,
                "final_disposition": response.disposition.value,
                "suggested_tool": response.suggested_tool,
                "reason": response.reason,
                "latency_ms": latency_ms,
                "input_token_count": int(input_token_count),
                "output_token_count": len(generated),
            },
        ))

    return response


def _build_routing_prompt(request: RouteRequest) -> str:
    """Build the prompt for the routing classifier."""
    parts = [ROUTER_SYSTEM_PROMPT, "\n\n--- User message ---\n"]

    # Last user message
    for msg in reversed(request.messages):
        if msg.role == "user" and msg.content:
            parts.append(str(msg.content))
            break

    # Available tools summary
    if request.tools:
        parts.append("\n\n--- Available tools ---\n")
        for tool in request.tools[:20]:  # cap at 20 to stay within context
            parts.append(f"- {tool.function.name}: {tool.function.description or ''}")

    parts.append("\n\n--- Classify now (JSON only) ---")
    return "\n".join(parts)


def _parse_route_response(
    raw: str,
    confidence_threshold: float,
    latency_ms: float,
) -> RouteResponse:
    """Parse the model's JSON output into a RouteResponse.

    If parsing fails or confidence is below threshold, returns "defer".
    """
    # Try to extract JSON from the output
    try:
        # Handle markdown code blocks
        cleaned = raw
        if "```" in cleaned:
            cleaned = cleaned.split("```")[1]
            if cleaned.startswith("json"):
                cleaned = cleaned[4:]
            cleaned = cleaned.strip()

        # Find JSON object
        start = cleaned.find("{")
        end = cleaned.rfind("}") + 1
        if start >= 0 and end > start:
            parsed = json.loads(cleaned[start:end])
        else:
            raise ValueError("No JSON object found")

        disposition = RouteDisposition(parsed.get("disposition", "defer"))
        confidence = float(parsed.get("confidence", 0.0))
        reason = str(parsed.get("reason", ""))
        suggested_tool = parsed.get("suggested_tool")

        # Apply confidence threshold — defer if not confident enough
        if disposition != RouteDisposition.DEFER and confidence < confidence_threshold:
            logger.info(
                "Routing confidence %.2f below threshold %.2f, deferring",
                confidence,
                confidence_threshold,
            )
            return RouteResponse(
                disposition=RouteDisposition.DEFER,
                confidence=confidence,
                reason=f"Confidence {confidence:.2f} below threshold {confidence_threshold:.2f}: {reason}",
                suggested_tool=suggested_tool,
                latency_ms=latency_ms,
            )

        return RouteResponse(
            disposition=disposition,
            confidence=confidence,
            reason=reason,
            suggested_tool=suggested_tool if isinstance(suggested_tool, str) else None,
            latency_ms=latency_ms,
        )

    except Exception as exc:
        logger.warning("Failed to parse routing response: %s — raw: %s", exc, raw[:200])

        # Emit failure audit event
        if _audit_ledger is not None:
            ctx = get_trace_context()
            _audit_ledger.emit(AuditEvent(
                event_type="RouteDecision",
                severity="error",
                component="router",
                trace_id=ctx.trace_id,
                span_id=ctx.span_id,
                session_id=ctx.session_id,
                duration_ms=latency_ms,
                output_hash=hash_content(raw),
                attrs={
                    "error": str(exc),
                    "raw_output_preview": raw[:200],
                    "final_disposition": "defer",
                    "parsed_confidence": 0.0,
                    "latency_ms": latency_ms,
                },
            ))

        return RouteResponse(
            disposition=RouteDisposition.DEFER,
            confidence=0.0,
            reason=f"Parse failure: {exc}",
            latency_ms=latency_ms,
        )

"""Generation pipeline: prompt → tokenize → generate → decode.

Provides the core generate() function that the server endpoints call.
Handles both streaming and non-streaming generation, and collects
metrics for every call.
"""

from __future__ import annotations

import logging
import time
import uuid
from typing import AsyncIterator

import mlx.core as mx
from mlx_lm.utils import generate_step

from .audit import AuditEvent, AuditLedger, hash_content
from .engine import LoadedModel
from .metrics import MetricsCollector
from .middleware import get_trace_context
from .schemas import (
    ChatCompletionChunk,
    ChatCompletionChunkChoice,
    ChatCompletionChunkDelta,
    ChatCompletionChoice,
    ChatCompletionRequest,
    ChatCompletionResponse,
    ChatMessage,
    UsageInfo,
)

logger = logging.getLogger(__name__)

# Module-level reference to the audit ledger, set by server lifespan.
_audit_ledger: AuditLedger | None = None


def set_audit_ledger(ledger: AuditLedger) -> None:
    """Called by server to inject the audit ledger."""
    global _audit_ledger
    _audit_ledger = ledger


def _apply_chat_template(model: LoadedModel, messages: list[ChatMessage]) -> str:
    """Convert chat messages to a prompt string using the model's tokenizer."""
    formatted = [{"role": m.role, "content": m.content or ""} for m in messages]
    try:
        return model.tokenizer.apply_chat_template(
            formatted, tokenize=False, add_generation_prompt=True
        )
    except Exception:
        # Fallback: simple concatenation
        parts = []
        for m in messages:
            parts.append(f"<start_of_turn>{m.role}\n{m.content or ''}<end_of_turn>")
        parts.append("<start_of_turn>model\n")
        return "\n".join(parts)


def generate_response(
    model: LoadedModel,
    request: ChatCompletionRequest,
    metrics: MetricsCollector,
) -> ChatCompletionResponse:
    """Non-streaming generation. Returns a complete response."""
    t0 = time.monotonic()
    ctx = get_trace_context()

    prompt = _apply_chat_template(model, request.messages)
    tokens = mx.array(model.tokenizer.encode(prompt))
    prompt_tokens = tokens.size

    generated_tokens = []
    for token, _ in generate_step(
        prompt=tokens,
        model=model.model,
        temp=request.temperature,
        top_p=request.top_p,
    ):
        generated_tokens.append(token.item())
        if len(generated_tokens) >= request.max_tokens:
            break
        # Check for EOS
        if token.item() == model.tokenizer.eos_token_id:
            break

    text = model.tokenizer.decode(generated_tokens)
    # Strip EOS artifacts
    for eos in ("<end_of_turn>", "<eos>"):
        if text.endswith(eos):
            text = text[: -len(eos)].rstrip()

    latency_ms = (time.monotonic() - t0) * 1000
    metrics.record(latency_ms, len(generated_tokens))

    # Emit InferenceCompleted audit event
    if _audit_ledger is not None:
        _audit_ledger.emit(AuditEvent(
            event_type="InferenceCompleted",
            severity="info",
            component="inference",
            trace_id=ctx.trace_id,
            span_id=ctx.span_id,
            parent_span_id=ctx.parent_span_id,
            session_id=ctx.session_id,
            duration_ms=latency_ms,
            input_hash=hash_content(prompt),
            output_hash=hash_content(text),
            attrs={
                "model_id": model.model_id,
                "prompt_token_count": int(prompt_tokens),
                "completion_token_count": len(generated_tokens),
                "latency_ms": latency_ms,
                "temperature": request.temperature,
                "max_tokens": request.max_tokens,
                "stream": False,
            },
        ))

    return ChatCompletionResponse(
        id=f"chatcmpl-{uuid.uuid4().hex[:12]}",
        model=request.model,
        choices=[
            ChatCompletionChoice(
                message=ChatMessage(role="assistant", content=text),
                finish_reason="stop",
            )
        ],
        usage=UsageInfo(
            prompt_tokens=prompt_tokens,
            completion_tokens=len(generated_tokens),
            total_tokens=prompt_tokens + len(generated_tokens),
        ),
    )


async def generate_stream(
    model: LoadedModel,
    request: ChatCompletionRequest,
    metrics: MetricsCollector,
) -> AsyncIterator[str]:
    """Streaming generation. Yields SSE-formatted chunks."""
    t0 = time.monotonic()
    ctx = get_trace_context()
    completion_id = f"chatcmpl-{uuid.uuid4().hex[:12]}"

    prompt = _apply_chat_template(model, request.messages)
    tokens = mx.array(model.tokenizer.encode(prompt))
    prompt_tokens = tokens.size

    # Initial chunk with role
    initial = ChatCompletionChunk(
        id=completion_id,
        model=request.model,
        choices=[
            ChatCompletionChunkChoice(
                delta=ChatCompletionChunkDelta(role="assistant"),
            )
        ],
    )
    yield f"data: {initial.model_dump_json()}\n\n"

    generated_count = 0
    buffer = []
    all_text_parts = []

    for token, _ in generate_step(
        prompt=tokens,
        model=model.model,
        temp=request.temperature,
        top_p=request.top_p,
    ):
        tok_id = token.item()
        if tok_id == model.tokenizer.eos_token_id:
            break

        generated_count += 1
        buffer.append(tok_id)

        # Decode incrementally (handles multi-byte tokens)
        text = model.tokenizer.decode(buffer)
        if text and not text.endswith("\ufffd"):  # skip incomplete UTF-8
            all_text_parts.append(text)
            chunk = ChatCompletionChunk(
                id=completion_id,
                model=request.model,
                choices=[
                    ChatCompletionChunkChoice(
                        delta=ChatCompletionChunkDelta(content=text),
                    )
                ],
            )
            yield f"data: {chunk.model_dump_json()}\n\n"
            buffer = []

        if generated_count >= request.max_tokens:
            break

    # Flush any remaining buffer
    if buffer:
        text = model.tokenizer.decode(buffer)
        if text:
            all_text_parts.append(text)
            chunk = ChatCompletionChunk(
                id=completion_id,
                model=request.model,
                choices=[
                    ChatCompletionChunkChoice(
                        delta=ChatCompletionChunkDelta(content=text),
                    )
                ],
            )
            yield f"data: {chunk.model_dump_json()}\n\n"

    # Final chunk
    final = ChatCompletionChunk(
        id=completion_id,
        model=request.model,
        choices=[
            ChatCompletionChunkChoice(
                delta=ChatCompletionChunkDelta(),
                finish_reason="stop",
            )
        ],
    )
    yield f"data: {final.model_dump_json()}\n\n"
    yield "data: [DONE]\n\n"

    latency_ms = (time.monotonic() - t0) * 1000
    metrics.record(latency_ms, generated_count)

    # Emit InferenceCompleted audit event for streaming
    full_output = "".join(all_text_parts)
    if _audit_ledger is not None:
        _audit_ledger.emit(AuditEvent(
            event_type="InferenceCompleted",
            severity="info",
            component="inference",
            trace_id=ctx.trace_id,
            span_id=ctx.span_id,
            parent_span_id=ctx.parent_span_id,
            session_id=ctx.session_id,
            duration_ms=latency_ms,
            input_hash=hash_content(prompt),
            output_hash=hash_content(full_output),
            attrs={
                "model_id": model.model_id,
                "prompt_token_count": int(prompt_tokens),
                "completion_token_count": generated_count,
                "latency_ms": latency_ms,
                "temperature": request.temperature,
                "max_tokens": request.max_tokens,
                "stream": True,
            },
        ))

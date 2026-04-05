"""Pydantic models for request/response/health/metrics."""

from __future__ import annotations

from enum import Enum
from typing import Any

from pydantic import BaseModel, Field


# ---------------------------------------------------------------------------
# Chat completions (OpenAI-compatible subset)
# ---------------------------------------------------------------------------

class ChatMessage(BaseModel):
    role: str
    content: str | list[dict[str, Any]] | None = None
    tool_calls: list[ToolCall] | None = None
    tool_call_id: str | None = None


class ToolFunction(BaseModel):
    name: str
    description: str | None = None
    parameters: dict[str, Any] | None = None


class ToolDefinition(BaseModel):
    type: str = "function"
    function: ToolFunction


class ToolCall(BaseModel):
    id: str
    type: str = "function"
    function: ToolCallFunction


class ToolCallFunction(BaseModel):
    name: str
    arguments: str  # JSON string


class ChatCompletionRequest(BaseModel):
    model: str = "gemma"
    messages: list[ChatMessage]
    max_tokens: int = 4096
    temperature: float = 0.7
    top_p: float = 0.95
    stream: bool = False
    tools: list[ToolDefinition] | None = None
    tool_choice: str | dict[str, Any] | None = None
    stop: list[str] | str | None = None


class ChatCompletionChoice(BaseModel):
    index: int = 0
    message: ChatMessage
    finish_reason: str | None = "stop"


class UsageInfo(BaseModel):
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0


class ChatCompletionResponse(BaseModel):
    id: str
    object: str = "chat.completion"
    model: str = "gemma"
    choices: list[ChatCompletionChoice]
    usage: UsageInfo = UsageInfo()


# ---------------------------------------------------------------------------
# Streaming (SSE chunks)
# ---------------------------------------------------------------------------

class ChatCompletionChunkDelta(BaseModel):
    role: str | None = None
    content: str | None = None
    tool_calls: list[ToolCall] | None = None


class ChatCompletionChunkChoice(BaseModel):
    index: int = 0
    delta: ChatCompletionChunkDelta
    finish_reason: str | None = None


class ChatCompletionChunk(BaseModel):
    id: str
    object: str = "chat.completion.chunk"
    model: str = "gemma"
    choices: list[ChatCompletionChunkChoice]


# ---------------------------------------------------------------------------
# Routing (FunctionGemma dispatch)
# ---------------------------------------------------------------------------

class RouteDisposition(str, Enum):
    LOCAL = "local"
    REMOTE = "remote"
    AUGMENT = "augment"
    DEFER = "defer"


class RouteRequest(BaseModel):
    messages: list[ChatMessage]
    tools: list[ToolDefinition] | None = None
    confidence_threshold: float = 0.7


class RouteResponse(BaseModel):
    disposition: RouteDisposition
    confidence: float
    reason: str
    suggested_tool: str | None = None
    latency_ms: float = 0.0


# ---------------------------------------------------------------------------
# KV cache management
# ---------------------------------------------------------------------------

class KVStats(BaseModel):
    entries: int = 0
    total_tokens: int = 0
    bitwidth: float = 3.5
    compression_ratio: float = 1.0
    memory_bytes: int = 0
    max_memory_bytes: int = 0
    utilization: float = 0.0


class KVSnapshotRequest(BaseModel):
    path: str | None = None  # default: auto-generated in context store


class KVRestoreRequest(BaseModel):
    path: str


class KVEvictRequest(BaseModel):
    count: int  # number of oldest entries to evict


# ---------------------------------------------------------------------------
# Health and metrics
# ---------------------------------------------------------------------------

class ModelStatus(BaseModel):
    name: str
    loaded: bool = False
    parameters: str = ""
    quantization: str = ""
    memory_bytes: int = 0


class InferenceMetrics(BaseModel):
    total_requests: int = 0
    total_tokens_generated: int = 0
    avg_latency_ms: float = 0.0
    p50_latency_ms: float = 0.0
    p99_latency_ms: float = 0.0
    tokens_per_second: float = 0.0


class HealthResponse(BaseModel):
    status: str = "ok"
    models: list[ModelStatus] = []
    kv_cache: KVStats = KVStats()
    gen_metrics: InferenceMetrics = InferenceMetrics()
    route_metrics: InferenceMetrics = InferenceMetrics()
    system_memory_used_gb: float = 0.0
    system_memory_total_gb: float = 0.0
    audit_ledger_events: int = 0
    audit_ledger_path: str = ""

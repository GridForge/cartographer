"""Observable inference: per-call latency, memory, cache stats."""

from __future__ import annotations

import time
from collections import deque
from dataclasses import dataclass, field

from .schemas import InferenceMetrics


@dataclass
class _Sample:
    latency_ms: float
    tokens: int
    timestamp: float


class MetricsCollector:
    """Thread-safe (single-event-loop) metrics for inference calls."""

    def __init__(self, window_size: int = 500) -> None:
        self._samples: deque[_Sample] = deque(maxlen=window_size)
        self._total_requests = 0
        self._total_tokens = 0

    def record(self, latency_ms: float, tokens: int) -> None:
        self._samples.append(
            _Sample(latency_ms=latency_ms, tokens=tokens, timestamp=time.monotonic())
        )
        self._total_requests += 1
        self._total_tokens += tokens

    def snapshot(self) -> InferenceMetrics:
        if not self._samples:
            return InferenceMetrics(
                total_requests=self._total_requests,
                total_tokens_generated=self._total_tokens,
            )

        latencies = sorted(s.latency_ms for s in self._samples)
        n = len(latencies)
        total_time_s = sum(s.latency_ms for s in self._samples) / 1000.0
        total_tok = sum(s.tokens for s in self._samples)

        return InferenceMetrics(
            total_requests=self._total_requests,
            total_tokens_generated=self._total_tokens,
            avg_latency_ms=sum(latencies) / n,
            p50_latency_ms=latencies[n // 2],
            p99_latency_ms=latencies[min(int(n * 0.99), n - 1)],
            tokens_per_second=total_tok / total_time_s if total_time_s > 0 else 0.0,
        )

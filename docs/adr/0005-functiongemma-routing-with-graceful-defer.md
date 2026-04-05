# ADR-0005: FunctionGemma Routing with Graceful Defer

**Status:** accepted
**Date:** 2026-04-04

## Context
FunctionGemma (270M params) classifies requests to decide local versus remote handling. A small model that guesses wrong is worse than one that admits uncertainty. Incorrect routing silently degrades user experience with no recovery path.

## Decision
Four dispositions: local, remote, augment, defer. "Defer" means the model cannot classify with sufficient confidence, so the frontier model decides. Confidence threshold defaults to 0.7; below threshold, always defer regardless of predicted disposition. All routing decisions are logged with full audit trail: input hash, raw output hash, parsed disposition, threshold application, final action.

## Consequences
Conservative routing means more requests go to the frontier model initially. Zero bad routing decisions at the cost of higher frontier usage. Threshold is tunable based on accumulated audit data, enabling evidence-based optimization over time.

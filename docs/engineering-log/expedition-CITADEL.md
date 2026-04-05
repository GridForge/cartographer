# Engineering Log: expedition-CITADEL — CI Pipeline

## Session Summary
Designed CI strategy through multi-agent pipeline: Codex council (3 specialists, 73s) + Claude Python specialist (57s, traced all 27 test import chains) + Claude Rust specialist (68s, analyzed existing workflow + compile times) → adversarial recommendations agent. Result: single workflow, two jobs, no macOS runner, ~40 lines YAML.

## Prior Context
Foundation log (2026-04-04) is pending gate acceptance. CITADEL was identified as the next expedition — needed before FIRST-LIGHT to provide regression safety net.

## Investigation Results

### Existing CI Gaps (Rust specialist finding)
- Current `.github/workflows/rust-ci.yml` only runs `cargo fmt` + `cargo test -p cartographer-cli`
- **453 test annotations across 47 files** — CI exercises only one crate out of 10
- No clippy job despite workspace-level pedantic lints
- rusqlite `bundled` compiles SQLite from C: ~60-90s cold, negligible with `Swatinem/rust-cache`
- Zero macOS-specific dependencies in Rust — all tests run on ubuntu-latest

### Python Test Isolation (Claude specialist finding, confirmed by Codex)
All 27 Python tests are MLX-free. Full import chain traced:

| Module | MLX dependency? |
|--------|----------------|
| `audit.py` | No — stdlib only |
| `middleware.py` | No — starlette + audit |
| `schemas.py` | No — pydantic only |
| `kv_manager.py` | No — audit + schemas (lazy middleware import in function body) |
| `metrics.py` | No — stdlib + schemas |
| `engine.py` | **YES** — `import mlx.core`, `from mlx_lm import load` |
| `inference.py` | **YES** — `import mlx.core`, `from mlx_lm.utils` |
| `router.py` | **YES** — `import mlx.core`, `from mlx_lm.utils` |
| `server.py` | **Transitively YES** — imports engine, inference, router |

**Critical: `test_audit.py` imports only audit, middleware, kv_manager, schemas — zero MLX paths.**

### Packaging Issue (Codex finding)
`pyproject.toml` declares `mlx>=0.31` as base dependency. `pip install .` fails on Linux. Must make MLX optional:
- Base deps: pydantic, starlette (cross-platform)
- Optional `[mlx]` extra: mlx, mlx-lm, uvicorn, fastapi
- Dev extra `[dev]`: pytest, httpx, pytest-asyncio

### Recommendations Agent Final Verdict

**Architecture:** One file `.github/workflows/ci.yml`, two jobs (Rust + Python), ubuntu-latest only.

**Rejected alternatives:**
- macOS runner (costs 10x, validates nothing Linux doesn't cover)
- Tiered pipeline (taxonomy overhead, zero behavioral difference)
- `@pytest.mark.mlx` (zero MLX tests exist — don't build for hypothetical)
- Manual dispatch macOS workflow ("never triggered = doesn't exist")
- Path filters ("create silent breakage on cross-cutting changes")

**Self-assessment:** Follows Commandment VII (simplest correct), Commandment II (invisible > configurable). Implementable in <2 hours.

### FMEA: CI System

| # | Failure Mode | S | O | D | RPN | Mitigation |
|---|-------------|---|---|---|-----|------------|
| 1 | rusqlite bundled compile exceeds timeout | 4 | 3 | 2 | 24 | rust-cache reduces to first-run only |
| 2 | Transitive dep pulls mlx on Linux | 7 | 2 | 3 | 42 | CI itself is the detection mechanism |
| 3 | CI passes but MLX inference breaks | 6 | 4 | 5 | **120** | Accepted risk — engineering log gate is the mitigation |

RPN 120 on failure mode 3 is the accepted gap. CI covers compilation parity, not inference parity. Documented, not ignored.

## Decisions

### D-017: Single unified ci.yml replacing rust-ci.yml
- **Context**: Existing `rust-ci.yml` only tested `cartographer-cli` (1 of 10 crates, no clippy). Recommendations agent specified single file, two jobs.
- **Decision**: Replaced `.github/workflows/rust-ci.yml` with `.github/workflows/ci.yml`. Two jobs: `rust` (fmt + clippy + test --workspace) and `python` (install .[dev] + pytest).
- **Rationale**: Follows recommendations agent exactly. No path filters (prevent silent breakage). No macOS runner (all tests run on Linux). Commandment VII.
- **Status**: Implemented

### D-018: MLX as optional dependency in pyproject.toml
- **Context**: `pip install .` fails on Linux because `mlx>=0.31` is a base dependency. CI needs to install without MLX.
- **Decision**: Base deps: pydantic, starlette. Optional extras: `[mlx]` (mlx, mlx-lm, fastapi, uvicorn), `[dev]` (pytest, httpx, pytest-asyncio), `[all]` (both). CI installs `.[dev]`. Local installs `.[all]`.
- **Rationale**: Explicit extras group is obvious and greppable. No `sys_platform` markers (recommendations agent: "clever packaging breaks").
- **Status**: Implemented, verified — `pip install ".[dev]"` + pytest passes on local machine

### D-019: No path filters on CI triggers
- **Context**: Old workflow had `paths: [rust/**, .github/workflows/rust-ci.yml]`. Recommendations agent explicitly rejected path filters.
- **Decision**: Trigger on all pushes to main and all PRs. No path filtering.
- **Rationale**: "Path filters create silent breakage on cross-cutting changes." A Python packaging change that breaks Rust integration would be missed with path filters.
- **Status**: Implemented

## Verification Results

| Check | Result |
|-------|--------|
| `cargo fmt --all --check` | Clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | Clean (0 warnings) |
| `cargo test --workspace` | 167 passed, 1 ignored (pre-existing intermittent init test) |
| `pip install ".[dev]"` (no MLX) | Success — pydantic, starlette, pytest, httpx installed |
| `python -m pytest tests/test_audit.py -v` | 27/27 passed (0.12s) |

## Open Items
- CI workflow not yet pushed to GitHub — needs commit + push to verify green on runner
- Old `rust-ci.yml` deleted locally, needs to be committed
- Council validation of this log entry pending after CI green

## Council Validation Status
- [x] Implementation matches investigation findings (single file, two jobs, no macOS, MLX optional)
- [ ] CI green on main (needs push)
- [ ] Engineering log closed and validated

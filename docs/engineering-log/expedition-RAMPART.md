# Engineering Log: expedition-RAMPART — CI Integration Test Fixes

## Session Summary
Designed fixes for 5 integration tests skipped in CI (ubuntu-latest failures). Multi-agent pipeline: 2 exploration agents (root cause analysis + shell spawning patterns) → Plan agent → Council Judge. First plan REJECTED (64/100) for proposing production code changes (`sh -lc` → `sh -c`). Revised plan: test-only and CI-only changes, zero production modifications. Judge scored revised plan 86% CONDITIONAL_ACCEPT with 20/20 safety.

## Prior Context
Expedition CITADEL established CI but skipped 5 shell-spawning integration tests. Accepted as FMEA failure mode #3 (RPN 120). This expedition closes that gap.

## Decisions Made

### D-020: Fix tests, not production code
- **Context**: First plan proposed changing `sh -lc` to `sh -c` globally in bash.rs, hooks.rs, tools/lib.rs. Council Judge REJECTED (64/100, Safety 9/20): "removing `-l` is not a no-op... can break real users who rely on login-shell initialization."
- **Decision**: Fix the test environment and test helpers only. Production code (`sh -lc`) remains untouched.
- **Rationale**: Login shell semantics are intentional for user-facing shell execution. Tests should configure their environment to work with production behavior, not change production behavior to match CI.
- **Status**: Judge-approved (Safety 20/20)

### D-021: Sandbox disabled via test fixture config
- **Context**: Tests 1 and 3 fail because Linux sandbox (`unshare --user`) is enabled by default. On macOS, sandbox is disabled (not Linux). On CI, sandbox fails in clean env.
- **Decision**: Tests write `settings.json` with `{"sandbox":{"enabled":false}}` in their temp workspace. `std::env::set_current_dir` to the workspace so `ConfigLoader::default_for(cwd)` picks up the config. Restore cwd after test.
- **Rationale**: Tests are testing tool dispatch logic, not sandbox behavior. Sandbox has its own dedicated tests.
- **Status**: Approved

### D-022: Temp dir collision fix via PID + atomic counter
- **Context**: Tests 3 and 5 use `SystemTime::now().as_nanos()` for temp dir naming. On fast CI runners, parallel tests get identical nanosecond timestamps → directory collision.
- **Decision**: Replace with `std::process::id()` + `AtomicU64::fetch_add(1)`. Applied in 3 locations: hooks.rs, init.rs, tools/lib.rs.
- **Rationale**: Atomic counter + PID is deterministically unique within and across processes. Standard pattern.
- **Status**: Approved (Judge confidence 0.97)

### D-023: Python setup in Rust CI job
- **Context**: Test 2 (REPL) needs `python3` on PATH. CI's `setup-python` step is only in the Python job.
- **Decision**: Add `actions/setup-python@v5` to Rust job. Also add defensive skip guard in test if python not found.
- **Rationale**: Belt and suspenders — CI provides python, test skips gracefully if somehow missing.
- **Status**: Approved

## Council Judge Results

### First pass: 64/100 — REJECTED
| Category | Score | Issue |
|----------|-------|-------|
| Correctness | 19/30 | Root causes assumed, `-lc` → `-c` not proven as fix |
| Completeness | 17/25 | Additional `-lc` sites not covered |
| Safety | 9/20 | **Production behavior change** |
| Testability | 11/15 | |
| Minimality | 8/10 | |

### Second pass: 86/100 — CONDITIONAL_ACCEPT
| Category | Score | Issue |
|----------|-------|-------|
| Correctness | 24/30 | bash_tool cwd specificity needed |
| Completeness | 22/25 | All 5 addressed |
| Safety | **20/20** | Zero production changes |
| Testability | 12/15 | REPL skip guard needed |
| Minimality | 9/10 | |

### D-024: EPIPE handling in hook stdin write (production bug found by CI)
- **Context**: After fixing the 5 test issues, CI revealed a 6th failure: `collects_and_runs_hooks_from_enabled_plugins` fails with "Broken pipe (os error 32)". The `output_with_stdin` method in `hooks.rs` writes JSON payload to the child process stdin. On Linux, hook scripts that don't read stdin (e.g., `printf`-only) exit before the parent finishes writing, causing EPIPE.
- **Decision**: Ignore `ErrorKind::BrokenPipe` on stdin `write_all`. Propagate all other IO errors. This is a legitimate production bug, not just a test issue — any hook script that exits fast on Linux would trigger this.
- **Rationale**: The child process may not need stdin. EPIPE on stdin is informational, not fatal. The child's exit code and stdout/stderr are the meaningful results.
- **Status**: Implemented. CI green.

## Verification Results

| Check | Result |
|-------|--------|
| `cargo fmt --all --check` | Clean |
| `cargo clippy --workspace -- -D warnings` | Clean |
| `cargo test --workspace` (local) | 446 passed, 0 failed |
| CI: Rust job (ubuntu-latest) | **Green** — fmt + clippy + full `cargo test --workspace` (no skips) |
| CI: Python job (ubuntu-latest) | **Green** — 27/27 tests |
| Previously skipped tests on CI | All 5 now passing: `initialize_repo` ✓, `clean_env_cli` ✓, `collects_and_runs_hooks` ✓, `bash_tool` ✓, `repl_python` ✓ |

### CI run evidence
- PR: GridForge/cartographer#1
- Run: green on both jobs (Rust 48s, Python 15s)
- All `--skip` flags removed from CI workflow
- EPIPE fix surfaced and resolved a real production bug

## Open Items
- Council validation of this log entry pending

## Council Validation Status
- [x] Implementation matches investigation findings
- [x] CI green on ubuntu-latest with zero skips
- [x] All 5 previously-skipped tests passing
- [x] EPIPE production bug found and fixed
- [ ] Adversarial documentation validation pending

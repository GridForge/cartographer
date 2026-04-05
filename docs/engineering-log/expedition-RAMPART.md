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

## Verification Results
*Pending implementation.*

## Open Items
- Implementation pending
- Docker-based Linux verification recommended before pushing
- Council validation of completed log pending

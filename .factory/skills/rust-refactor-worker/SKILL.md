---
name: rust-refactor-worker
description: Refactoring worker for pure structural changes — module splits, deduplication, test cleanup, comment removal, doc additions. Preserves all behavior; uses cargo test/check/fmt as gates.
---

# Rust Refactor Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Pure refactoring features: module splits, file reorganization, test harness deduplication, test consolidation, comment cleanup, dead code removal, doc comment additions, documentation updates. **Zero behavior changes.**

## Required Skills

None. All work is done with Rust tooling (cargo test, cargo check, cargo fmt).

## Work Procedure

### 1. Read Feature Description and Baseline

1. Read the feature description in `features.json` carefully
2. Run `cargo test --workspace` to establish a passing baseline — **all tests must pass before you start**
3. Record the current test count and any pre-existing warnings
4. Identify which crate(s) and file(s) need changes

### 2. Investigate Current Structure

Before any changes:
1. Read all relevant source files that will be modified
2. Map out the current public API (types, traits, functions, constants)
3. Map out the current module structure
4. Understand what downstream crates depend on

### 3. Execute Refactoring

Follow the specific feature instructions. General rules:

**For module splits:**
1. Create the new module files with the split-out code
2. Update `mod` declarations and `pub use` re-exports in `lib.rs` (or `mod.rs`)
3. Ensure the public API is identical — same types reachable from the same paths
4. Run `cargo check --workspace` after every module file change
5. Run `cargo test --workspace` to confirm no behavioral change

**For test harness deduplication:**
1. Create the shared harness module
2. Migrate one test file at a time to use the shared harness
3. Run `cargo test -p launa-integration-tests` after each migration
4. Never delete a test — only move/consolidate the infrastructure

**For test consolidation:**
1. Identify the tests to consolidate (escape tests, HTTP tests, mock tests)
2. Before removing any test, verify another test covers the same scenario
3. Write the consolidated test first, then remove the originals
4. Run `cargo test --workspace` after each consolidation step

**For comment/dead code cleanup:**
1. Remove decorative banners (`// ===`, `// ──`), bare `//` lines
2. Remove `#[allow(dead_code)]` on actually-unused code, then remove the dead code
3. Remove unused imports
4. Run `cargo check --workspace` and `cargo fmt` after cleanup

**For doc comment additions:**
1. Add `///` doc comments to public items that lack them
2. Follow existing doc comment style in the codebase
3. Doc comments should explain **why** and **what**, not restate the code

### 4. Verify After Every Change

After each logical change (module split, file migration, cleanup):

1. `cargo check --workspace` — must exit 0, no new warnings
2. `cargo test --workspace` — must exit 0, all tests pass
3. If the feature touches `app/`: `cd C:\dev\launa\app && cargo +esp check` — must exit 0
4. `cargo fmt` — format all changed files

### 5. Final Verification

After all changes for the feature are complete:

1. `cargo test --workspace` — all tests pass, compare count to baseline
2. `cargo check --workspace` — no new warnings
3. `cargo fmt --all -- --check` — clean formatting
4. If touched: `cd C:\dev\launa\app && cargo +esp check`
5. Review `git diff` — confirm only structural changes, no behavioral changes

### 6. Commit

Commit with a focused message describing the structural change. Use imperative mood.
Example: `Split launa-core into rate_limiter, command_tracker, timers, spa_app modules`

## Critical Rules

- **NEVER change behavior** — only move, rename, reorganize, add docs, remove noise
- **NEVER delete a test** unless an equivalent test exists elsewhere
- **ALWAYS run `cargo test --workspace`** after every change that touches .rs files
- **ALWAYS run `cargo fmt`** before committing
- **PRESERVE the public API** — downstream crates must compile without changes
- If `cargo test --workspace` fails at any point, STOP and fix before proceeding

## Example Handoff

```json
{
  "salientSummary": "Split launa-core/src/lib.rs (2631 lines) into 8 focused modules: rate_limiter.rs, command_tracker.rs, timers.rs, heap_monitor.rs, spa_app.rs, log_buffer.rs, actions.rs, types.rs. All 31 unit tests preserved, all 172 integration tests pass, no API changes.",
  "whatWasImplemented": "Module split of launa-core: RateLimiter → rate_limiter.rs, CommandTracker+ExpectedChange → command_tracker.rs, PumpTimer+PumpTimerManager+HoldModeTimer → timers.rs, HeapMonitor → heap_monitor.rs, SpaApp → spa_app.rs, RemoteLogBuffer+LogEntry → log_buffer.rs, AppAction → actions.rs, shared types → types.rs. lib.rs re-exports all public items. No behavioral changes.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      { "command": "cargo test -p launa-core", "exitCode": 0, "observation": "31 tests passed" },
      { "command": "cargo test --workspace", "exitCode": 0, "observation": "904 tests passed, no failures" },
      { "command": "cargo check --workspace", "exitCode": 0, "observation": "No warnings" },
      { "command": "cargo fmt --all -- --check", "exitCode": 0, "observation": "Clean" }
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": []
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- Refactoring reveals a behavioral bug that needs a separate fix
- Public API cannot be preserved without a design decision
- `cargo test --workspace` fails and the failure is not caused by this feature's changes
- Feature scope is larger than expected and needs splitting

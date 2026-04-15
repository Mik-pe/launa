---
name: rust-worker
description: Rust workspace crate worker for launa project — implements features with TDD, runs tests, verifies no regressions.
---

# Rust Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Features involving changes to workspace crates: `launa-protocol`, `launa-ota`, `launa-sim`, `launa-integration-tests`, `launa-core`. These are all desktop-testable Rust crates.

## Required Skills

None. All work is done with Rust tooling (cargo test, cargo check, cargo fmt).

## Work Procedure

### 1. Read Feature Description and Understand Scope

Read the feature description carefully. Identify which crate(s) need changes. Read the relevant source files before making any changes.

### 2. Write Tests First (RED)

Before any implementation, write tests that will fail:

1. Read the existing test structure in the target crate
2. Add new tests following the established naming conventions
3. Run `cargo test -p <crate>` and verify the NEW tests FAIL (expected)
4. Verify all EXISTING tests still pass

### 3. Implement (GREEN)

1. Make the minimal changes needed to make the new tests pass
2. Follow existing code patterns and conventions in the crate
3. Keep changes focused — one logical change per feature
4. All workspace crates are `#![no_std]` — never use `std::`
5. Use `extern crate alloc` and `alloc::` collections where needed

### 4. Verify No Regressions

1. Run `cargo test -p <crate>` — all tests pass (new + existing)
2. Run `cargo test --workspace` — no regressions in any crate
3. Run `cargo fmt` — clean formatting
4. If the feature touches `launa-ota`, run `cargo check -p launa-ota --no-default-features` to verify no-alloc compilation

### 5. Update TASKS.md

Check off the completed task items by changing `- [ ]` to `- [x]` for the items this feature covers.

### 6. Commit

Commit with a focused message describing the change. Use imperative mood.

## Key Conventions

- **TDD**: Tests written BEFORE implementation, always
- **no_std**: All workspace crates are `#![no_std]`
- **Backward compatible**: New features default to off/disabled
- **No new dependencies**: Use only existing crate dependencies
- **Test structure**: Integration tests in `launa-integration-tests/src/lib.rs` (test groups), sim tests in `tests/sim_tests.rs`
- **SpaApp helpers**: `make_spaapp()`, `make_status_frame()`, `make_ready_frame()` in integration tests

## Example Handoff

```json
{
  "salientSummary": "Added failure injection to MockOta (fail_on_begin, fail_on_write_after, fail_on_finalize) and OtaError context fields (byte_offset, address). All 6 new tests pass, all 359 existing tests still pass.",
  "whatWasImplemented": "MockOta failure injection fields, OtaError thiserror derive with context fields, MAX_FIRMWARE_SIZE constant, in_progress guard. 8 new tests added to launa-ota.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      { "command": "cargo test -p launa-ota", "exitCode": 0, "observation": "All 14 tests passed" },
      { "command": "cargo test --workspace", "exitCode": 0, "observation": "All 367 tests passed" },
      { "command": "cargo check -p launa-ota --no-default-features", "exitCode": 0, "observation": "Compiles without alloc" },
      { "command": "cargo fmt", "exitCode": 0, "observation": "Clean" }
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": [
      { "file": "crates/launa-ota/src/lib.rs", "cases": [
        { "name": "test_mock_ota_fail_on_begin", "verifies": "fail_on_begin returns BeginFailed" },
        { "name": "test_mock_ota_fail_on_write_after", "verifies": "fail_on_write_after(N) fails at byte N" },
        { "name": "test_mock_ota_fail_on_finalize", "verifies": "fail_on_finalize returns FinalizeFailed" },
        { "name": "test_ota_error_display", "verifies": "all variants have non-empty Display" },
        { "name": "test_ota_error_write_failed_byte_offset", "verifies": "WriteFailed has byte_offset field" },
        { "name": "test_ota_error_flash_error_address", "verifies": "FlashError has address field" },
        { "name": "test_ota_firmware_size_exceeded", "verifies": "rejects firmware past MAX_FIRMWARE_SIZE" },
        { "name": "test_ota_concurrent_safety", "verifies": "begin-while-in-progress, write-before-begin errors" }
      ] }
    ]
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- Feature requires changes to `app/` crate (ESP32-only, can't test on desktop)
- Need to add a new workspace dependency not already in Cargo.toml
- Existing tests fail and the fix is outside the feature's scope
- Requirements are ambiguous — check feature description and AGENTS.md first

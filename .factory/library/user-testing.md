# User Testing

Testing surface, required testing skills/tools, and resource cost classification.

## Validation Surface

This is a refactoring mission. No user-facing UI, no web server, no CLI tool.

**Primary validation surface:** `cargo test --workspace` — automated test suite.

**Additional validation commands:**
- `cargo check --workspace` — compilation gate
- `cd C:\dev\launa\app && cargo +esp check` — ESP32 cross-compilation gate
- `cargo fmt --all -- --check` — formatting gate
- `cargo doc --workspace --no-deps` — documentation build gate

**Manual review assertions:**
- Directory structure verification (module splits, test file organization)
- Grep for noise/decorative comments (cleanup verification)
- Git diff review for behavioral changes
- Table-driven test vector completeness review

No browser testing, no TUI testing, no manual interaction needed.

## Milestone-Specific Validation Notes

### monolith-splits
- Verify mod.rs ≤930 lines, lib.rs ≤100 lines via line count
- Verify test count preservation (≥157 for launa-sim, ≥baseline for integration-tests)
- Verify module structure: proper mod declarations, snake_case files

### test-quality
- Verify table-driven test functions ≤10 in command.rs and command_parser.rs
- Verify misplaced tests relocated (grep for specific test names)
- Verify vacuous assertion fixed (grep for tautological pattern)
- Verify ignored doc-tests resolved (grep for ```ignore)

### comment-cleanup
- Grep for noise comments, triple-documented offsets, verbose test comments
- Verify crate-level docs present (head -5 of each lib.rs)
- Verify cargo doc builds clean

### test-coverage
- Count new tests in registration.rs, hal tests, filter.rs, dispatcher.rs, integration-tests
- Verify error injection API works (MockTransport methods)

### final-polish
- Full verification suite (cargo test, check, fmt, doc, esp check)
- Verify test utility consolidation (grep for duplicate definitions)

## Validation Concurrency

**Max concurrent validators:** 1

All validation is via cargo operations — single process, no concurrency benefit.

## ESP32 Verification Note

`app/` crate changes cannot be unit-tested on desktop. Verification is via:
- `cargo +esp check` (default features)
- `cargo +esp check --features sniff`
- `cargo +esp check --features hw-test`
Code inspection validates behavioral correctness for app/-only changes.

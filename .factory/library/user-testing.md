# User Testing

Testing surface, required testing skills/tools, and resource cost classification.

## Validation Surface

This mission modifies Rust library crates only. No user-facing UI, no web server, no CLI tool.

**Primary validation surface:** `cargo test --workspace` — automated test suite.

**Tool:** `cargo test` — all assertions verified through unit and integration tests.

No browser testing, no TUI testing, no manual interaction needed.

## Validation Concurrency

**Max concurrent validators:** 1

All validation is via `cargo test` which is single-process. No concurrent execution benefit.

## Test Structure

- `crates/launa-protocol/src/*.rs` — unit tests per module (133+ tests)
- `crates/launa-ota/src/lib.rs` — unit tests in mock module (14 tests)
- `crates/launa-esp-ota/src/lib.rs` — unit tests (34 tests)
- `crates/launa-sim/src/*.rs` — unit tests per module (90+ tests)
- `crates/launa-core/src/lib.rs` — SpaApp unit tests (29+ tests)
- `crates/launa-integration-tests/src/lib.rs` — integration tests (108+ tests)
- `crates/launa-integration-tests/tests/sim_tests.rs` — sim-level integration tests (29+ tests)

## ESP32 Verification Note

`app/` crate changes cannot be unit-tested on desktop. Verification is via:
- `cargo +esp check` (default features)
- `cargo +esp check --features sniff`
- `cargo +esp check --features hw-test`
Code inspection validates behavioral correctness for app/-only changes.

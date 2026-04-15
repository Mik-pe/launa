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

- `crates/launa-protocol/src/*.rs` — unit tests per module (159 tests: 71 unit + 27 fuzz + 17 property + misc)
- `crates/launa-ota/src/lib.rs` — unit tests in mock module
- `crates/launa-sim/src/*.rs` — unit tests per module (41 tests)
- `crates/launa-integration-tests/src/lib.rs` — integration tests using SpaApp + SpaSim (73 tests)
- `crates/launa-integration-tests/tests/sim_tests.rs` — sim-level tests using SimTransport + SpaController (30 tests)

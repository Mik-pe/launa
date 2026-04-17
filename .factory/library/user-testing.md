# User Testing

Testing surface, required testing skills/tools, and resource cost classification.

## Validation Surface

This is a refactoring mission. No user-facing UI, no web server, no CLI tool.

**Primary validation surface:** `cargo test --workspace` — automated test suite.

**Additional validation commands:**
- `cargo check --workspace` — compilation gate
- `cd C:\dev\launa\app && cargo +esp check` — ESP32 cross-compilation gate
- `cargo fmt --all -- --check` — formatting gate
- `cargo doc --workspace` — documentation build gate

**Manual review assertions:**
- Directory structure verification (module splits)
- Grep for decorative comments (cleanup verification)
- Git diff review for behavioral changes
- Documentation consistency check (entity counts = 27)

No browser testing, no TUI testing, no manual interaction needed.

## Validation Concurrency

**Max concurrent validators:** 1

All validation is via cargo operations — single process, no concurrency benefit.

## ESP32 Verification Note

`app/` crate changes cannot be unit-tested on desktop. Verification is via:
- `cargo +esp check` (default features)
- `cargo +esp check --features sniff`
- `cargo +esp check --features hw-test`
Code inspection validates behavioral correctness for app/-only changes.

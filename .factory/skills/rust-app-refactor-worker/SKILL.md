---
name: rust-app-refactor-worker
description: Refactoring worker for app/ and xtask/ crates — module extraction, comment cleanup, dead code removal, deduplication. Preserves all behavior; uses cargo +esp check as gate.
---

# Rust App Refactor Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Pure refactoring in `app/` (ESP32 firmware) and `xtask/` (host tooling): module extraction, comment cleanup, dead code removal, deduplication of repeated logic, doc additions. **Zero behavior changes.**

## Required Skills

None.

## Work Procedure

### 1. Read Feature and Baseline

1. Read the feature description carefully
2. Run `cargo test --workspace` to establish baseline
3. Run `cd C:\dev\launa\app && cargo +esp check` to verify app/ compiles
4. Record pre-existing warnings

### 2. Investigate

1. Read all affected files in `app/src/` and/or `xtask/src/`
2. For app/: understand ESP32-specific constraints (32 KiB heap, no_std, esp-hal + embassy)
3. For xtask/: understand the host-side tooling patterns
4. Map out what can be moved to workspace crates vs what must stay

### 3. Execute Refactoring

**For MQTT codec extraction (app → launa-mqtt):**
1. Identify protocol encoding/decoding functions in `app/src/mqtt_client.rs`
2. Create the target module in `crates/launa-mqtt/src/`
3. Move pure protocol logic (no TCP/socket/ESP32 dependencies)
4. Add desktop tests for the extracted functions
5. Update `app/src/mqtt_client.rs` to import from `launa-mqtt`
6. Verify: `cd C:\dev\launa\app && cargo +esp check` after each step
7. Ensure `launa-mqtt` remains `no_std` compatible

**For app/ module splits:**
1. Extract logical units from `main.rs` into separate files (sniff mode, diagnostics, etc.)
2. Update module declarations
3. Keep all `#[embassy_executor::task]` functions accessible

**For xtask/ deduplication:**
1. Extract shared utilities (`project_root()`, `ctrlc_handler()`, arg parsing) into `xtask/src/lib.rs` or a new `util.rs`
2. Update all modules to import from shared location
3. Run `cargo test -p xtask` to verify

**For comment/dead code cleanup:**
1. Remove decorative banners, bare `//` lines, dead code, unused imports
2. Fix any `_skip_confirm`-like dead variables
3. Remove unused dependencies from `Cargo.toml` if any

### 4. Verify After Every Change

1. `cargo test --workspace` — all tests pass
2. `cd C:\dev\launa\app && cargo +esp check` — app/ compiles
3. `cargo check -p xtask` — if xtask touched
4. `cargo fmt` — clean formatting
5. Review `git diff` — confirm no behavioral changes

### 5. Commit

Focused commit message. Example: `Extract MQTT v5 codec from app/mqtt_client.rs to launa-mqtt`

## Critical Rules

- **NEVER change behavior** — only structural/organizational changes
- **NEVER break no_std** — launa-mqtt must remain `#![no_std]` compatible
- **ALWAYS verify ESP32 cross-compilation** after changes to app/ or launa-mqtt
- **32 KiB heap** — no new allocations in app/ code
- Run `cargo fmt` before committing

## Example Handoff

```json
{
  "salientSummary": "Extracted MQTT v5 protocol codec (CONNECT/PUBLISH/SUBSCRIBE encoding, SUBACK/CONNACK parsing, remaining_length codec) from app/mqtt_client.rs to launa-mqtt/src/v5_codec.rs. Added 15 desktop tests. app/mqtt_client.rs reduced by ~200 lines. ESP32 cross-compilation verified.",
  "whatWasImplemented": "New module launa-mqtt/src/v5_codec.rs with encode_connect, encode_publish, encode_subscribe, parse_suback, parse_connack, encode_remaining_length, decode_remaining_length. Tests cover all packet types with edge cases. app/mqtt_client.rs imports and delegates to v5_codec.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      { "command": "cargo test -p launa-mqtt", "exitCode": 0, "observation": "207 tests passed (15 new codec tests)" },
      { "command": "cargo test --workspace", "exitCode": 0, "observation": "All tests pass" },
      { "command": "cd C:\\dev\\launa\\app && cargo +esp check", "exitCode": 0, "observation": "ESP32 build succeeds" },
      { "command": "cargo fmt --all -- --check", "exitCode": 0, "observation": "Clean" }
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": [
      { "file": "crates/launa-mqtt/src/v5_codec.rs", "cases": [
        { "name": "test_encode_connect", "verifies": "CONNECT packet encoding" },
        { "name": "test_encode_publish_qos1", "verifies": "PUBLISH QoS 1 encoding" },
        { "name": "test_decode_remaining_length", "verifies": "Variable-length decoding" }
      ] }
    ]
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- Extracted code depends on types that don't exist in any workspace crate
- `cargo +esp check` fails due to a dependency conflict (not fixable by refactoring)
- MQTT codec extraction reveals coupling to ESP32-specific types that can't be abstracted
- Feature scope is larger than expected

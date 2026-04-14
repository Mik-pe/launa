---
name: worker
description: >-
  General-purpose worker droid for the Launa ESP32 spa controller project.
  Use for code exploration, bug fixes, feature implementation, testing,
  protocol work, and any non-trivial task in the launa codebase.
model: inherit
tools:
  - file_read
  - file_edit
  - file_create
  - shell
  - grep
  - glob
  - web_search
  - context7_resolve
  - context7_query
---

# Worker Droid — Launa Project

Complete the requested task and report back concisely. Include your chain of
thought and a paper trail of relevant resources (files, code, git commits, web
searches, etc.) following the order and logic through which you discovered them.

## Project Context

You are working on **Launa**, an ESP32 firmware (Rust) that interfaces with
Balboa BP6013G1 spa controllers over RS-485 and publishes state to Home
Assistant via MQTT with OTA support.

## Repository Layout

- `crates/launa-protocol/` — Balboa protocol parser (no_std, pure logic)
- `crates/launa-hal/` — Hardware abstraction traits + mocks
- `crates/launa-mqtt/` — MQTT client with HA auto-discovery
- `crates/launa-ota/` — OTA firmware update support
- `crates/launa-integration-tests/` — Integration tests with SpaSimulator
- `app/` — ESP32 firmware binary (excluded from workspace, needs esp-idf)
- `docs/` — Architecture, protocol reference, BP6013G1 notes

## Key Commands

Before starting work, verify the codebase compiles and tests pass:

```
cargo check          # Verify workspace compiles
cargo test            # Run all workspace tests
```

After making changes, always run:

```
cargo test            # All tests must pass before reporting done
cargo check           # Workspace must compile cleanly
```

## Coding Rules

1. **no_std for workspace crates** — Use `extern crate alloc`, not `std::`.
   Only `app/` has access to `std` via `esp-idf-svc`.
2. **Never panic in parsers** — All protocol parsers return `Result` and handle
   malformed input gracefully.
3. **Mock behind features** — Mock implementations go behind
   `cfg(feature = "std")` or in `#[cfg(test)]` modules.
4. **Error handling** — `thiserror` for library crate errors, `anyhow` for
   application errors.
5. **Test everything** — Protocol logic must have unit tests. Use SpaSimulator
   for integration tests.
6. **Run tests before finishing** — `cargo test` must pass with zero failures.

## Protocol Quick Reference

- RS-485 at 115200 baud, 8N1
- Frames: `0x7E` delimited, CRC-8 checksum
- Status: type `FF AF 13`, ~1 second interval
- Commands: type `0A BF`, sub-type is first payload byte
- Full docs: `docs/protocol.md`

## When Blocked

- Check `TASKS.md` for current priorities and known bugs
- Check `docs/architecture.md` for system design
- Check `docs/protocol.md` for protocol details
- Check `AGENTS.md` for full project context

# Environment

Environment variables, external dependencies, and setup notes.

**What belongs here:** Required env vars, external dependencies, toolchain requirements, platform-specific notes.
**What does NOT belong here:** Service ports/commands (use `.factory/services.yaml`).

---

## Toolchain

- Rust stable + `esp` toolchain for `app/` (not used in this mission)
- `cargo test --workspace` runs all 359+ desktop tests
- No external services required for workspace crate development

## No Environment Variables

This mission only modifies workspace crates. No MQTT broker, WiFi, or ESP32 hardware needed.

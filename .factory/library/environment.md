# Environment

Environment variables, external dependencies, and setup notes.

**What belongs here:** Required env vars, external dependencies, toolchain requirements, platform-specific notes.
**What does NOT belong here:** Service ports/commands (use `.factory/services.yaml`).

---

## Toolchain

- **Workspace crates**: Rust stable (x86_64-pc-windows-msvc). `cargo test --workspace` and `cargo check --workspace`.
- **ESP32 app crate**: `esp` toolchain. Verify with `cd app && cargo +esp check`. The `esp` toolchain is at `C:\Users\mikae\.rustup\toolchains\esp`.
- `app/.cargo/config.toml` sets `target = "xtensa-esp32-none-elf"` and `build-std = ["core", "alloc"]`.
- No external services required for development.

## No Environment Variables

No MQTT broker, WiFi, or ESP32 hardware needed for this mission. All changes verified through desktop tests and `cargo +esp check`.

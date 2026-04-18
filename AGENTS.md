# Launa - Agent Context

When working with a specific crate, check for crate-level notes in its own AGENTS.md or lib.rs header.

## Project Overview

ESP32 firmware (Rust, no_std) that interfaces with Balboa BP6013G1 spa controllers over RS-485 and publishes state to Home Assistant via MQTT. Supports OTA firmware updates.

## Workspace Crates

- **launa-protocol** — Balboa spa protocol parser (frame encode/decode, status, commands, config, registration, fault/filter/info)
- **launa-hal** — Hardware abstraction traits + mock implementations
- **launa-mqtt** — MQTT client with Home Assistant auto-discovery (27 entities)
- **launa-ota** — OTA firmware update trait + mock
- **launa-esp-ota** — ESP32 OTA using esp-storage (custom, replaces esp-hal-ota)
- **launa-sim** — Spa simulator (mock BP6013G1 mainboard) for integration testing
- **launa-integration-tests** — Integration tests using SpaSimulator
- **xtask** — Cargo xtask tooling (flash, monitor, spa-sim, OTA, sniffer)
- **app/** — ESP32 firmware binary (excluded from workspace; uses esp-hal + embassy)

Protocol reference: `docs/protocol.md`. Architecture details: `docs/architecture.md`.

## Build and Test

```bash
cargo check                         # Typecheck workspace
cargo test                          # Run all workspace tests
cargo test -p launa-protocol        # Test single crate
cargo test -p launa-integration-tests  # Integration tests with SpaSimulator
cd app && cargo espflash flash --chip esp32 --monitor  # Flash to ESP32
```

### xtask Commands

Requires `launa.toml` at project root (gitignored; copy from `launa.example.toml`).

| Command | Description |
|---|---|
| `cargo xtask flash` | Flash firmware to ESP32 via USB |
| `cargo xtask monitor` | Read serial output from ESP32 |
| `cargo xtask flash-monitor` | Flash + monitor in one command |
| `cargo xtask sniff-decode` | Decode sniffer frames from MQTT |
| `cargo xtask spa-sim` | Simulate spa over RS-485 |
| `cargo xtask ota-serve` | Serve firmware .bin over HTTP for OTA |
| `cargo xtask ota-flash` | Build + serve + trigger OTA |
| `cargo xtask self-test` | Run hardware self-test on ESP32 |
| `cargo xtask config-flash` | Write WiFi/MQTT config to ESP32 NVS |

## Architecture

- All workspace crates are `no_std`, pure Rust, desktop-testable
- `app/` is ESP32-only: `esp-hal` 1.0 + `embassy` + `esp-radio` + `rust-mqtt` + `esp-nvs`
- No ESP-IDF C SDK — pure Rust throughout
- Protocol logic in `launa-protocol`; hardware abstractions in `launa-hal`
- Tests use SpaSimulator (mock BP6013G1 mainboard)
- HA integration via MQTT auto-discovery (sensor, number, select, switch, light, fan, binary_sensor)

## Coding Conventions

- `no_std` for workspace crates — use `extern crate alloc`, not `std::`
- Protocol parsers must handle malformed input gracefully (`Result`, never panic)
- Mock implementations behind `cfg(feature = "std")` or in test modules
- Error handling: `thiserror` for library errors, `anyhow` for application errors
- Run `cargo test` before committing
- Rust 2021 edition, workspace uses `resolver = "2"`

## Git Commit Conventions

```
Summary line (50-72 chars, imperative mood)

- Optional bullet points describing key changes
- Be specific, avoid vague "Update" summaries
- No Co-Authored-By tags
```

Run `cargo fmt` before committing. Keep commits focused: one logical change per commit.

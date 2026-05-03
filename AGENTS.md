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

## Hardware

- **RS-485 transceiver:** MAX13487EESA + 131 (auto-direction half-duplex transceiver — no DE/RE pin control needed)
- **Never speculate about faulty RS-485 hardware.** The transceivers have been verified working with the RS-485 debugger firmware — communication issues are always firmware/software, not hardware.

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
| `cargo xtask flash --monitor` | Flash + monitor in one command |
| `cargo xtask monitor` | Read serial output from ESP32 |
| `cargo xtask sniff-decode` | Decode sniffer frames from MQTT |
| `cargo xtask spa-sim` | Simulate spa over RS-485 |
| `cargo xtask ota-serve` | Serve firmware .bin over HTTP for OTA |
| `cargo xtask ota-flash` | Build + serve + trigger OTA |
| `cargo xtask config-flash` | Write WiFi/MQTT config to ESP32 NVS |
## Web Frontend

- Located in `web/`, uses Vue 3 + Vite + Tailwind CSS 4
- **Use `bun`** (not npm/node) for all web commands: `bun run build`, `bun run dev`, `bun run typecheck`
- Bun is installed at `/opt/homebrew/bin/bun`

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
- **NEVER add preamble bytes before RS-485 frames.** This has been tried and verified to have no effect — the auto-direction transceiver handles line turnaround without any preamble flushing.
- **When stuck on a problem, search the web for answers.** Use WebSearch to look up documentation, known issues, and solutions. Spawn worker subagents to research in parallel if needed. Do not guess — look it up.

## Git Commit Conventions

```
Summary line (50-72 chars, imperative mood)

- Optional bullet points describing key changes
- Be specific, avoid vague "Update" summaries
- No Co-Authored-By tags
```

Run `cargo fmt` before committing. Keep commits focused: one logical change per commit.

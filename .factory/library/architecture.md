# Architecture

How the launa system works at a high level.

## Workspace Crates

- **launa-protocol** — Balboa spa RS-485 protocol: frame encode/decode (CRC-8 + HDLC byte stuffing), message dispatch, status/config/fault/filter/info parsing, command encoding, registration state machine
- **launa-hal** — Hardware abstraction traits: Transport (UART), Clock, Network. Mock impls behind `std` feature.
- **launa-mqtt** — MQTT topics, discovery builder (20 HA entities), command parser (allowlist), state JSON serializer. no_std compatible.
- **launa-ota** — OTA firmware update trait + MockOta. no_std, mock behind `mock` feature. MockOta session state machine: `not_started → in_progress → finalized/rolled_back`. begin() sets in_progress=true, successful finalize() sets in_progress=false, rollback_and_reboot() resets state. Failed operations leave state unchanged. MAX_FIRMWARE_SIZE=1_835_008 bytes (1.75 MiB). Failure injection fields: fail_on_begin, fail_on_write_after(N), fail_on_finalize.
- **launa-esp-ota** — ESP32 OTA implementation using esp-storage directly.
- **launa-sim** — Spa simulator (SpaSim/SpaState), SimBroker (mock MQTT), SimTransport (virtual RS-485), SpaController, VirtualClock
- **launa-core** — SpaApp: extracted app logic (registration, command tracking, pump/hold timers, stale detection, diagnostics, alerting). Pure synchronous, no IO.
- **launa-integration-tests** — 73+ integration tests using SpaApp + SpaSim + VirtualClock
- **xtask** — Cargo xtask tooling (flash, monitor, spa-sim, OTA, sniffer)

## Key Data Flow

```
RS-485 bytes → FrameDecoder → Frame → SpaApp::process_frame() → Vec<AppAction>
MQTT command → SpaApp::on_mqtt_command() → queued
Periodic     → SpaApp::tick() → Vec<AppAction>

AppAction::SendFrame(bytes)     → UART TX
AppAction::PublishState(status) → MQTT publish
AppAction::PublishAlert/Diag    → MQTT publish
```

## Invariants

- All workspace crates are `#![no_std]` with `extern crate alloc`
- All tests run on desktop via `cargo test --workspace`
- SpaSim defaults produce deterministic output (new features default to off)
- CommandTracker bounded at MAX_PENDING_COMMANDS=8
- Temperature validation: hard upper limit 108°F / 42°C
- FrameDecoder: CRC-8 + HDLC byte stuffing for 0x7E/0x7D, configurable max_buffer_size (default 512) with overflow protection

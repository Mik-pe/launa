# Launa - Architecture

## Project Goal

ESP32 firmware that reads from a Balboa BP6013G1 spa controller over RS-485 and
publishes state to Home Assistant via MQTT. Supports OTA firmware updates.

## High-Level Architecture

```
┌─────────────────────────────────────────────────┐
│                   ESP32 Firmware                 │
│                                                  │
│  ┌──────────────┐   ┌──────────────┐            │
│  │   UART/RS485 │◄──┤   launa-     │            │
│  │   Transport  │   │   protocol   │            │
│  └──────────────┘   └──────┬───────┘            │
│                             │                    │
│  ┌──────────────┐   ┌──────▼───────┐            │
│  │   launa-     │◄──┤   launa-     │            │
│  │   mqtt       │   │   hal        │            │
│  └──────┬───────┘   └──────────────┘            │
│         │                                        │
│  ┌──────▼───────┐   ┌──────────────┐            │
│  │   WiFi /     │   │   launa-     │            │
│  │   TCP Stack  │   │   ota        │            │
│  └──────────────┘   └──────────────┘            │
└─────────────────────────────────────────────────┘
         │                          │
         ▼                          ▼
   ┌──────────┐              ┌──────────┐
   │   MQTT   │              │   OTA    │
   │  Broker  │              │  Server  │
   └────┬─────┘              └──────────┘
        │
        ▼
   ┌──────────┐
   │  Home    │
   │ Assistant│
   └──────────┘
```

## Crate Structure

### `launa-protocol`

Balboa spa protocol parser. Pure logic, no hardware dependencies, `no_std`
compatible, fully testable on desktop.

- `frame` — Message frame encoding/decoding (0x7E delimited, CRC-8)
- `status` — Status update parsing (temperature, pumps, heating mode, etc.)
- `command` — Command construction (toggle items, set temperature, config requests)
- `config` — Configuration response parsing
- `registration` — Client ID registration state machine
- `dispatcher` — Frame type routing / dispatching
- `fault` — Fault log entry parsing
- `filter` — Filter cycle schedule parsing
- `information` — Spa information response parsing
- `crc8` — CRC-8 computation

### `launa-hal`

Hardware abstraction traits. Defines the interface between protocol logic and
real hardware. Enables desktop testing via mock implementations.

- `Transport` trait (async read/write bytes over UART/RS-485)
- `Network` trait (WiFi connect, TCP socket creation)
- `TcpSocket` trait (read/write/close over TCP)
- `Clock` trait (current time, timestamps)
- Mock implementations behind `std` feature flag

### `launa-mqtt`

MQTT topics, Home Assistant auto-discovery, command parsing, and state serialization.
Includes an extracted MQTT v5 protocol codec.

- `discovery` — HA auto-discovery message generation (27 entities)
- `topics` — MQTT topic builder and LWT/birth configuration
- `command_parser` — MQTT command parsing (toggle, set temp)
- `state` — Spa state to JSON serialization
- `v5_codec` — MQTT v5 protocol packet encoding/decoding
- `packet` — Packet extraction from TCP stream
- `remote_log` — Remote log entry serialization
- `ota_url` — OTA URL parsing
- `escape` — JSON string escaping (no_std, no serde_json)

### `launa-ota`

OTA firmware update trait with mock implementation for desktop testing.
Provides the `OtaUpdate` trait that `launa-esp-ota` implements for real hardware.

- `OtaUpdate` trait (begin, write, finalize, mark_valid, rollback)
- `OtaError` error types
- `http` — HTTP response parser for firmware download (Range header, status parsing)
- `MockOta` for unit testing

### `launa-esp-ota`

Custom ESP32 OTA implementation using `esp-storage` (embedded-storage traits)
for direct flash access. Replaces `esp-hal-ota`.

- `crypto` — SHA-256, HMAC-SHA256, CRC-32/MPEG-2 implementations
- `flash` — Partition operations (read, erase, write, otadata management)
- `ota` — `EspOtaFlash` state machine (begin/write/finalize/rollback with CRC + signature verification)

### `launa-core`

Extracted application logic — `SpaApp` owns all stateful firmware logic including
registration, command tracking, pump timers, hold timers, stale detection,
diagnostics, and fault handling. Returns `Vec<AppAction>` side effects.

- `spa_app` — Main `SpaApp` struct (process frames, tick, registration lifecycle)
- `actions` — `AppAction` enum (side effects: publish state, send frames, OTA, etc.)
- `command_tracker` — Tracks pending commands and confirms/refires on mismatch
- `timers` — `PumpTimer`, `PumpTimerManager`, `HoldModeTimer`
- `rate_limiter` — Command rate limiting (sliding window)
- `log_buffer` — `RemoteLogBuffer` for MQTT remote logging
- `heap_monitor` — Free heap tracking with alert thresholds
- `types` — Shared types and constants

### `launa-sim`

Spa simulator — mock Balboa BP6013G1 mainboard for integration testing.
Also provides `SimBroker` (mock MQTT broker) and `SimTransport` (virtual RS-485 wire).

- `spa_sim` — `SpaSim` with configurable responses, error injection, physics
  - `config` — Simulator configuration structs
  - `state` — `SpaState`, `SpaEvent`, `SpaEventType`
  - `frame_gen` — Status/config/fault/filter/info frame generation
  - `physics` — Thermal model, sensor noise, heater/pump interlock, overshoot
- `sim_broker` — `SimBroker` mock MQTT broker for verification
- `sim_transport` — `SimTransport` virtual RS-485 bidirectional pipe
- `clock` — `VirtualClock` for deterministic time in tests

### `launa-integration-tests`

130+ integration tests exercising `SpaApp` through the full simulation pipeline.
Tests use `SpaSimulator` → `SimTransport` → `SpaApp` → `SimBroker`.

- `harness` — Shared `TestHarness` base (eliminates duplicate setup code)
- `lib` — Integration test suites (registration, commands, OTA, fault, stale, etc.)

### `xtask`

Host-side Cargo xtask tooling. Not part of the firmware.

- Flash (with optional --monitor), monitor
- OTA build + serve + trigger
- Spa simulator (USB-RS485)
- Sniffer frame decoder
- NVS config flash

### `app/` (ESP32 firmware binary)

The final firmware binary. Excluded from the main workspace because it targets
`xtensa-esp32-none-elf` with `esp-hal` + `embassy` (pure Rust, no_std, no ESP-IDF C SDK).

- `main` — Embassy async main loop with inter-task channels
- `mqtt_client` — MQTT v5 client over embassy-net TCP (uses `launa-mqtt` codec)
- `mqtt_task` — MQTT task wiring (connect, subscribe, publish loop)
- `transport` — UART/RS-485 transport (esp-hal UART with optional DE pin)
- `wifi` — WiFi connectivity (esp-radio + embassy-net)
- `ota` — OTA download and partition management
- `config` — NVS config storage (esp-nvs)
- `clock` — ESP32 real-time clock implementation
- `diagnostics` — Diagnostic publishing and alerts
- `remote_log` — Remote log publishing over MQTT
- `sniff` — Sniffer firmware (passive RS-485 monitoring)
- `types` — App-specific types (FaultBuf, etc.)
- `net_util` — Network utility functions
- `crypto` — ESP32-specific crypto utilities

## Desktop Testing Strategy

All protocol and business logic lives in workspace crates that compile for both
host and ESP32 targets:

```
launa-protocol  → cargo test (desktop) ✅  →  ESP32 binary ✅
launa-hal       → cargo test with mocks   →  real hardware impl
launa-mqtt      → cargo test with mocks   →  real MQTT broker
```

The `app/` crate only compiles for the ESP32 target and contains
ESP32-specific code that glues everything together.

## Home Assistant Integration

Uses MQTT auto-discovery to automatically create 27 entities in Home Assistant:

| Entity | HA Component | Description |
|--------|-------------|-------------|
| Water Temperature | `sensor` | Current water temp |
| Set Temperature | `number` | Target temperature |
| Heating State | `binary_sensor` | Is the heater active |
| Pump 1–6 | `switch` | On/Off toggle (6 pumps) |
| Light 1–4 | `light` | On/Off (4 light zones) |
| Blower | `fan` | On/Off |
| Heat Mode | `select` | Ready / Rest / Ready-in-Rest |
| Circulation Pump | `switch` | Circ pump toggle |
| Temperature Range | `select` | High / Low |
| Hold Mode | `switch` | Hold mode toggle |
| Mister | `switch` | Mister toggle |
| AUX 1 / AUX 2 | `switch` | Optimistic switches (no state feedback) |
| Soak Mode | `switch` | Optimistic switch |
| Normal Operation | `switch` | Optimistic switch |
| Clear Notification | `switch` | Optimistic switch |
| Fault | `sensor` | Last fault code |
| Diagnostics | `sensor` | Diagnostic counters (entity_category: diagnostic) |
| Alert | `sensor` | Alert messages (entity_category: diagnostic) |

## OTA Update Flow

1. Firmware image hosted on HTTP server or pushed via MQTT
2. ESP32 downloads new image via WiFi
3. Writes to alternate OTA partition
4. Sets boot partition and reboots
5. On boot, validates and marks new firmware as valid (or rolls back)

## Security Considerations

> **WARNING: This firmware does NOT use TLS for MQTT or OTA connections.**
>
> - MQTT credentials (username/password) are transmitted in plaintext over TCP.
>   Any device on the same WiFi network can intercept them.
> - OTA firmware is downloaded over plaintext HTTP. A network attacker (MITM)
>   could inject malicious firmware.
> - HMAC-SHA256 signature verification exists in `launa-esp-ota` but is not
>   wired into the OTA flow and the signing key is hardcoded in source.
>   Do NOT rely on it for production security.
>
> **This is acceptable only on isolated/trusted home networks.** Before deploying
> in shared or untrusted network environments, add:
>
> 1. TLS for MQTT connections (port 8883) using `esp-tls` or `embedded-tls`
> 2. HTTPS for OTA firmware downloads
> 3. Per-device signing keys derived from eFuse BLOCK3 for firmware verification
>
> NVS passwords are encrypted at rest using AES-128-CTR with eFuse-derived keys.

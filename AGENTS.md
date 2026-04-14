# Launa - Agent Context

## Project Overview

ESP32 firmware (Rust) that interfaces with Balboa BP6013G1 spa controllers over RS-485 and publishes state to Home Assistant via MQTT with OTA support.

## Repository Structure

```
launa/
├── Cargo.toml                  # Workspace root (5 crates, excludes app/)
├── TASKS.md                    # Prioritized task tracker & known bugs
├── docs/
│   ├── architecture.md         # System architecture & crate descriptions
│   ├── protocol.md             # Balboa RS-485 protocol reference
│   └── bp6013g1.md             # BP6013G1 controller-specific notes
├── crates/
│   ├── launa-protocol/         # Balboa spa protocol parser (no_std, pure logic)
│   │   └── src/
│   │       ├── lib.rs          # Crate root & public API
│   │       ├── crc8.rs         # CRC-8 checksum
│   │       ├── frame.rs        # Frame encode/decode (0x7E delimited)
│   │       ├── status.rs       # Status update parser (FF AF 13 messages)
│   │       ├── command.rs      # Command builder (0A BF messages)
│   │       ├── config.rs       # Spa configuration parser
│   │       ├── registration.rs # Client ID registration state machine
│   │       ├── message.rs      # Message type definitions
│   │       ├── dispatcher.rs   # 0A BF message sub-type dispatcher
│   │       ├── information.rs  # Information response parser (0A BF 24)
│   │       ├── fault.rs        # Fault log response parser (0A BF 28)
│   │       └── filter.rs       # Filter cycles response parser (0A BF 23)
│   ├── launa-hal/              # Hardware abstraction traits + mocks
│   ├── launa-mqtt/             # MQTT client with HA auto-discovery
│   ├── launa-ota/              # OTA firmware update support
│   └── launa-integration-tests/ # Integration tests with SpaSimulator
├── app/                        # ESP32 firmware binary (excluded from workspace)
│   ├── Cargo.toml              # Uses esp-idf-sys (separate build)
│   └── src/                    # ESP32 main, UART, WiFi, MQTT glue
└── .factory/droids/            # Factory Droid configurations
```

## Key Commands

- `cargo test` — Run all tests across workspace crates (desktop only)
- `cargo test -p launa-protocol` — Protocol crate tests only
- `cargo test -p launa-integration-tests` — Integration tests with SpaSimulator
- `cargo check` — Verify workspace compiles
- `cd app && cargo espflash flash --chip esp32 --monitor` — Flash to ESP32 (needs esp-idf toolchain)

## Architecture Notes

- **Workspace crates** (`launa-protocol`, `launa-hal`, `launa-mqtt`, `launa-ota`, `launa-integration-tests`) are all pure Rust, desktop-testable, and `no_std` compatible
- The **`app/`** crate is ESP32-only using `esp-idf-svc`/`esp-idf-hal`, excluded from the Cargo workspace due to unconventional ESP-IDF build rules
- All protocol logic lives in `launa-protocol`; hardware abstractions are in `launa-hal`
- Tests use **SpaSimulator** (a mock BP6013G1 mainboard) for integration testing
- Home Assistant integration uses MQTT auto-discovery to create 8 entities (sensor, number, select, switch, light, fan, binary_sensor)

## Protocol Reference

The Balboa spa protocol is documented in `docs/protocol.md`. Key points:

- **Physical layer**: RS-485 at 115200 baud, 8N1
- **Framing**: Frames delimited by `0x7E` markers with CRC-8 checksum
- **Status updates**: Every ~1 second, type `FF AF 13` (28-byte payload)
- **Commands**: Type `0A BF` with sub-type discriminator byte as first payload byte:
  - `0x04` = Configuration request
  - `0x11` = Toggle item
  - `0x20` = Set temperature
  - `0x22` = Settings request
  - `0x23` = Filter cycles request
  - `0x24` = Information request
  - `0x27` = Temperature scale
  - `0x28` = Fault log request
  - `0x2E` = Control configuration
  - `0x94` = Configuration response

## Coding Conventions

- `no_std` compatible for workspace crates — use `extern crate alloc`, not `std::`
- `esp-idf-svc`/`esp-idf-hal` for the `app/` crate (std available there)
- All protocol parsers must handle malformed input gracefully (return `Result`, never panic)
- Mock implementations behind `cfg(feature = "std")` or in test modules
- Error handling: `thiserror` for library errors, `anyhow` for application errors
- Run `cargo test` before committing — all tests must pass
- Follow Rust 2021 edition conventions; workspace uses `resolver = "2"`

## Dependencies

Key workspace dependencies (see `Cargo.toml` for versions):
- `crc` — CRC-8 computation
- `heapless` — No-alloc collections for embedded
- `embedded-io` / `embedded-io-async` — Async I/O traits
- `serde` / `serde_json` — Serialization (MQTT payloads)
- `byteorder` — Byte order handling
- `log` — Logging facade
- `thiserror` — Library error types
- `anyhow` — Application error handling

## Current State

### Completed
- Project structure and Cargo workspace setup
- Balboa CRC-8 implementation with tests
- Frame encode/decode with streaming decoder
- Status update parser (temperature, pumps, lights, heating)
- Command builder (toggle, set temp, set time, settings requests)
- Spa configuration parser (pump/light/blower/circ capabilities)
- Client ID registration state machine
- Hardware abstraction traits with mock implementations
- Home Assistant MQTT auto-discovery builder (8 entities)
- OTA update trait with mock
- ESP32 app skeleton
- Full protocol documentation
- Comprehensive test suite with SpaSimulator mock infrastructure

### In Progress / Next (see TASKS.md)
- **Critical bugs**: Command encoding missing sub-type bytes, `heating_mode` wrong offset, mister status not parsed
- **Protocol**: Parse information/fault log/filter cycles responses; disambiguate `0A BF` sub-types
- **MQTT**: State serialization, command parsing, birth/last-will messages
- **ESP32 firmware**: UART transport, WiFi, MQTT client, main event loop, OTA, NVS config

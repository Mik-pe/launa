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
│   ├── launa-ota/              # OTA firmware update trait + mock
│   ├── launa-esp-ota/          # ESP32 OTA using embedded-storage (replaces esp-hal-ota)
│   ├── launa-sim/              # Spa simulator for integration testing
│   └── launa-integration-tests/ # Integration tests with SpaSimulator
├── xtask/                      # Cargo xtask tooling (desktop-only workspace crate)
├── app/                        # ESP32 firmware binary (excluded from workspace)
│   ├── Cargo.toml              # Uses esp-hal + embassy (no_std, xtensa-esp32-none-elf)
│   └── src/                    # ESP32 main, UART, WiFi, MQTT glue
└── .factory/droids/            # Factory Droid configurations
```

## Key Commands

- `cargo test` — Run all tests across workspace crates (desktop only)
- `cargo test -p launa-protocol` — Protocol crate tests only
- `cargo test -p launa-integration-tests` — Integration tests with SpaSimulator
- `cargo check` — Verify workspace compiles
- `cd app && cargo espflash flash --chip esp32 --monitor` — Flash to ESP32 (needs espflash + xtensa toolchain)

### Project Commands (`cargo xtask`)

All xtask commands require a `launa.toml` config file at the project root (gitignored). Create it from `launa.example.toml`:

```toml
[wifi]
ssid = "MyWiFi"
password = "MyPassword"

[mqtt]
host = "192.168.1.100"
port = 1883
user = ""
password = ""

[device]
id = "launa_spa"
serial_port = "COM3"

[ota]
serve_port = 8080
```

| Command | Description |
|---|---|
| `cargo xtask flash [--feature <name>] [--port <COMx>]` | Flash firmware to ESP32 via USB |
| `cargo xtask monitor [--port <COMx>] [--duration <secs>]` | Read serial output from ESP32 |
| `cargo xtask flash-monitor [--feature <name>] [--port <COMx>]` | Flash + monitor in one command |
| `cargo xtask sniff-decode [--host <host>] [--port <1883>]` | Decode sniffer frames from MQTT in real-time |
| `cargo xtask spa-sim [--port <COMx>] [--duration <secs>]` | Simulate spa over RS-485 via USB adapter |
| `cargo xtask ota-serve --firmware <path> [--port <8080>]` | Serve firmware .bin over HTTP for OTA |
| `cargo xtask ota-flash [--feature <name>] [--device-id <id>]` | Build + serve + trigger OTA remotely |
| `cargo xtask self-test [--port <COMx>]` | Run hardware self-test on ESP32 |
| `cargo xtask config-flash [--port <COMx>]` | Write WiFi/MQTT config to ESP32 NVS |

## Architecture Notes

- **Workspace crates** (`launa-protocol`, `launa-hal`, `launa-mqtt`, `launa-ota`, `launa-esp-ota`, `launa-sim`, `launa-integration-tests`) are all pure Rust, desktop-testable, and `no_std` compatible
- The **`app/`** crate is ESP32-only using `esp-hal` + `embassy` (no_std, pure Rust), excluded from the Cargo workspace
- **ESP32 stack**: `esp-hal` 1.0 (UART, GPIO), `esp-radio` (WiFi), `embassy-net` (TCP/IP), `rust-mqtt` (MQTTv5), `launa-esp-ota` (OTA via esp-storage), `esp-nvs` (config storage), `embassy` (async executor)
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
- `esp-hal` + `embassy` for the `app/` crate (no_std, pure Rust, no ESP-IDF C SDK)
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

Key `app/` dependencies:
- `esp-hal` 1.0+ — Hardware abstraction (UART, GPIO, SPI, I2C)
- `esp-rtos` — Scheduler for esp-radio + embassy bridge
- `esp-radio` — WiFi driver (unstable feature)
- `embassy-net` — Async TCP/IP stack (smoltcp)
- `embassy-executor`, `embassy-time` — Async runtime
- `rust-mqtt` — MQTT v5 client (no_std)
- `launa-esp-ota` — OTA partition management with rollback (custom, uses esp-storage)
- `esp-nvs` — Non-volatile storage (ESP-IDF compatible format)
- `esp-storage` — Raw flash access via embedded-storage traits

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
- Home Assistant MQTT auto-discovery builder (14 entities)
- OTA update trait with mock
- Full protocol documentation
- Comprehensive test suite with SpaSimulator (240 tests)
- `0A BF` message dispatcher, information/fault/filter parsers
- State serialization, command parsing, topic builder
- Integration tests, fuzz tests, property tests

### In Progress / Next (see TASKS.md)
- **ESP32 firmware rewrite**: Migrating `app/` from esp-idf-svc to esp-hal + embassy (pure Rust, no_std)
- **Protocol**: Validate parser against real spa hardware via sniffer
- **OTA**: HTTP download over embassy-net TCP still pending
- **Hardware testing**: Sniffer firmware, bench testing with RS-485 adapter

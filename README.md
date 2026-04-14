# Launa — ESP32 Balboa Spa Controller

**Rust firmware for ESP32 that bridges Balboa BP series spa controllers to Home Assistant via MQTT.**

[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-14%20passing-brightgreen.svg)]()

## Overview

Launa reads real-time state from a Balboa BP6013G1 (and compatible BP series) spa controller over RS-485, parses the Balboa serial protocol, and publishes everything to Home Assistant using MQTT auto-discovery. It also accepts commands from Home Assistant (toggle pumps, set temperature, change heat mode) and forwards them to the spa. Over-the-air (OTA) firmware updates are supported so you don't need physical access after initial deployment.

## Features

- **Balboa protocol parsing** — CRC-8 validated, `0x7E`-delimited frame encoder/decoder with streaming support
- **Full status decoding** — water temperature, set temperature, pump/light/blower/circ states, heat mode, fault codes
- **Command support** — toggle pumps, set temperature, set time, request configuration, change temperature scale
- **Client ID registration** — complete state machine for joining the RS-485 bus as a new client
- **Home Assistant auto-discovery** — 8 entities (sensors, switches, numbers, select, fan, light) appear automatically
- **OTA firmware updates** — dual-partition update flow with rollback support
- **Desktop testable** — all protocol logic compiles and runs on your dev machine; no hardware needed for unit tests
- **Modular crate design** — protocol, HAL, MQTT, and OTA are separate crates with clean trait boundaries
- **`no_std` compatible** — protocol crate works in bare-metal environments

## Hardware

### What You Need

| Component | Notes |
|-----------|-------|
| ESP32-C3 or ESP32-S3 dev board | Any standard board works |
| RS-485 transceiver module | MAX485, SP3485, or similar |
| Balboa BP series spa controller | BP6013G1 or compatible BP model |
| Jumper wires | For ESP32 ↔ transceiver and transceiver ↔ spa bus |

### Wiring Reference

```
ESP32                RS-485 Transceiver         Spa Controller
──────               ──────────────────         ─────────────
GPIO TX (UART) ────► DI (Data In)
GPIO RX (UART) ◄──── RO (Data Out)
GPIO DIR       ────► DE/RE (Direction)
3.3V           ────► VCC
GND            ────► GND
                      A ──────────────────────► RS-485 Data+
                      B ──────────────────────► RS-485 Data-
```

> ⚠️ **Always power off the spa controller before making wiring changes.** Verify that your RS-485 module operates at 3.3V logic, or use a level shifter.

## Architecture

The project is organized as a Cargo workspace with an excluded ESP32 binary crate:

| Crate | Description |
|-------|-------------|
| `launa-protocol` | Balboa spa protocol parser — CRC-8, frame codec, status/config parsing, command builder, client ID state machine |
| `launa-hal` | Hardware abstraction traits (`Transport`, `Network`, `Clock`) with mock implementations for desktop testing |
| `launa-mqtt` | MQTT client wrapper with Home Assistant discovery message generation and state/command routing |
| `launa-ota` | OTA firmware update support — download, partition management, boot switching, rollback |
| `launa-integration-tests` | End-to-end tests: mock transport → real frames → parsed status → JSON → MQTT topics |
| `app/` | ESP32 firmware binary — glues everything together with `esp-idf-hal` UART, WiFi, and MQTT implementations |

```
  Spa Controller ──RS-485──► ESP32 ──WiFi/MQTT──► Home Assistant
```

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2021, stable toolchain)
- [`espflash`](https://github.com/esp-rs/espflash) — for flashing ESP32
- ESP-IDF toolchain (installed automatically by `esp-idf-sys` on first build)

### Clone & Build

```bash
git clone https://github.com/Mik-pe/launa.git
cd launa

# Build and test all desktop crates
cargo build
cargo test
```

### Desktop Testing

All protocol and business logic lives in workspace crates that compile for both host and ESP32 targets:

```bash
# Run all 14 unit tests (no hardware needed)
cargo test
```

Tests cover CRC-8 computation, frame encoding/decoding, status update parsing, command construction, client ID registration, and MQTT discovery message generation.

### Flashing to ESP32

```bash
cd app
cargo espflash flash --monitor
```

## Configuration

Configuration is currently set at compile time via environment variables or by editing the app source:

| Setting | Description |
|---------|-------------|
| WiFi SSID | Your wireless network name |
| WiFi password | Your wireless network password |
| MQTT broker address | IP/hostname of your MQTT broker (e.g., `192.168.1.100:1883`) |
| Device ID | Unique identifier for Home Assistant entity prefixes |

> Runtime NVS-based configuration is planned — see [TASKS.md](TASKS.md) for roadmap.

## Home Assistant Setup

1. **MQTT broker** — Install and configure an MQTT broker (e.g., [Mosquitto](https://mosquitto.org/)) as a Home Assistant add-on or standalone service.
2. **MQTT integration** — Enable the MQTT integration in Home Assistant (Settings → Devices & Services → Add Integration → MQTT).
3. **Auto-discovery** — Launa publishes MQTT auto-discovery messages on startup. Entities appear automatically under a new device:

| Entity | Type | Description |
|--------|------|-------------|
| Water Temperature | Sensor | Current water temperature |
| Set Temperature | Number | Target temperature (adjustable) |
| Heat Mode | Select | Ready / Rest / Ready-in-Rest |
| Pump 1 / 2 / 3 | Switch | Toggle pumps on/off |
| Light | Light | Toggle spa light |
| Blower | Fan | Toggle blower |
| Heating State | Binary Sensor | Is the heater currently active |
| Fault | Sensor | Last fault code |

No manual entity configuration is needed — Launa handles discovery automatically.

## Development

### Desktop Testing Strategy

The key design principle is that all protocol and business logic lives in workspace crates that compile for both host and ESP32 targets:

```
launa-protocol  → cargo test (desktop) ✅  →  ESP32 binary ✅
launa-hal       → cargo test with mocks   →  real hardware impl
launa-mqtt      → cargo test with mocks   →  real MQTT broker
```

The `app/` crate only compiles for the ESP32 target and contains ESP32-specific glue code. This means you can develop and test the vast majority of the firmware on your desktop without any hardware.

### Running Tests

```bash
# All workspace tests
cargo test

# Verbose output
cargo test -- --nocapture

# Specific crate
cargo test -p launa-protocol
```

## License

This project is licensed under the [MIT License](LICENSE).

## Acknowledgments

- [ccutrer/balboa_worldwide_app](https://github.com/ccutrer/balboa_worldwide_app) — Comprehensive Balboa spa protocol documentation
- [jasta/esp32-balboa-spa](https://github.com/jasta/esp32-balboa-spa) — Rust ESP32 Balboa spa integration reference
- [cribskip/esp8266_spa](https://github.com/cribskip/esp8266_spa) — Protocol implementation reference

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

- CRC-8 computation
- Message frame encoding/decoding (0x7E delimited)
- Status update parsing
- Configuration response parsing
- Command message construction (toggle, set temp, etc.)
- Client ID registration state machine

### `launa-hal`

Hardware abstraction traits. Defines the interface between protocol logic and
real hardware. Enables desktop testing via mock implementations.

- `Transport` trait (read/write bytes)
- `Network` trait (WiFi connect, TCP socket)
- `Clock` trait (current time)
- Mock implementations behind `std` feature flag

### `launa-mqtt`

MQTT client wrapper with Home Assistant discovery support.

- Home Assistant MQTT auto-discovery message generation
- State publication (temperature, pump status, etc.)
- Command subscription (toggle pumps, set temperature, etc.)
- Device configuration for HA UI

### `launa-ota`

OTA firmware update support.

- Firmware download (HTTP or MQTT)
- Partition management
- Boot partition switching
- Rollback support

### `app/` (ESP32 firmware binary)

The final firmware binary. Excluded from the main workspace due to ESP-IDF
build requirements.

- ESP32-specific hardware implementations of `launa-hal` traits
- UART/RS-485 transport
- WiFi connectivity
- Main event loop

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

Uses MQTT auto-discovery to automatically create entities in Home Assistant:

| Entity | HA Component | Description |
|--------|-------------|-------------|
| Water Temperature | `sensor` | Current water temp |
| Set Temperature | `number` | Target temperature |
| Heat Mode | `select` | Ready / Rest / Ready-in-Rest |
| Pump 1/2/3 | `switch` | On/Off toggle |
| Light | `light` | On/Off |
| Blower | `fan` | On/Off |
| Heating State | `binary_sensor` | Is the heater active |
| Fault | `sensor` | Last fault code |

## OTA Update Flow

1. Firmware image hosted on HTTP server or pushed via MQTT
2. ESP32 downloads new image via WiFi
3. Writes to alternate OTA partition
4. Sets boot partition and reboots
5. On boot, validates and marks new firmware as valid (or rolls back)

# Launa

`~ ˖ ೫ ˖ ~`

Open-source ESP32 firmware that puts your Balboa hot tub on Home Assistant.

Control your pumps, temperature, lights, and blower from your phone. Set the temperature on your way home. Get notified if something goes wrong. All built in Rust, OTA-updatable, and safe enough to install without worrying about your spa.

```
Your hot tub ──RS-485──► ESP32 ──WiFi──► Home Assistant
```

Supports Balboa BP-series controllers (BP6013G1 and compatibles) — the kind found in most hot tubs.

## Hardware

| Component | Notes |
|-----------|-------|
| ESP-WROOM-32 dev board | Any standard ESP32 board |
| RS-485 transceiver module | Auto-direction type recommended (no DE pin) |
| USB power supply | Phone charger to power ESP32 at the spa |
| Balboa BP series spa controller | BP6013G1 or compatible |

### Wiring

```
ESP32                  RS-485 Module         Spa Controller
──────                 ──────────────         ─────────────
GPIO16 (TX) ───────► TX
GPIO17 (RX) ◄─────── RX
3.3V         ───────► VCC
GND          ───────► GND
                       A ─────────────────► Data+ (A)
                       B ─────────────────► Data- (B)
```

The RS-485 bus is electrically isolated — your ESP32 cannot damage the spa controller. The sniffer firmware (read-only, no transmission) is the safe first step to validate your setup.

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) stable toolchain
- `cargo espflash` — `cargo install cargo-espflash --locked`
- USB driver for your board (CP210x or CH340)

### Configure

```bash
git clone https://github.com/Mik-pe/launa.git
cd launa
cp launa.example.toml launa.toml
# Edit launa.toml with your WiFi and MQTT broker details
```

`launa.toml` is gitignored. All xtask commands fail early if required fields are missing.

### Flash

```bash
cargo test                    # run desktop tests, no hardware needed

cargo xtask config-flash      # write WiFi/MQTT config to ESP32 NVS (USB)
cargo xtask flash --feature sniff   # flash sniffer firmware (USB, read-only)
cargo xtask flash             # flash full firmware (USB)
```

### Remote Updates

After the first USB flash, the ESP32 stays at the spa on WiFi:

```bash
cargo xtask ota-flash --feature sniff   # deploy sniffer remotely
cargo xtask ota-flash                   # deploy full firmware remotely
```

Bad updates auto-rollback — if the new firmware crashes before connecting to MQTT, the bootloader reverts to the previous version.

## Commands

| Command | What it does |
|---------|-------------|
| `cargo xtask flash [--feature sniff\|hw-test]` | Flash via USB |
| `cargo xtask monitor` | Read serial output |
| `cargo xtask flash-monitor` | Flash + monitor |
| `cargo xtask config-flash` | Write config to NVS |
| `cargo xtask ota-flash` | Build and flash remotely over WiFi |
| `cargo xtask sniff-decode` | Decode sniffer frames from MQTT in real-time |
| `cargo xtask spa-sim` | Simulate a spa controller over USB-RS485 |
| `cargo xtask self-test` | Run hardware self-test |

## Home Assistant

1. Install an MQTT broker (e.g. [Mosquitto](https://mosquitto.org/))
2. Enable the MQTT integration in HA
3. Launa publishes auto-discovery messages on boot — entities appear automatically:

| Entity | Type | Description |
|--------|------|-------------|
| Water Temperature | Sensor | Current water temp |
| Set Temperature | Number | Target temp |
| Heat Mode | Select | Ready / Rest / Ready-in-Rest |
| Temperature Range | Select | High / Low |
| Pump 1 / 2 / 3 | Switch | Toggle pumps |
| Light | Light | Toggle spa light |
| Blower | Fan | Toggle blower |
| Circ Pump | Switch | Circ pump state |
| Mister | Switch | Mister state |
| Hold Mode | Switch | Hold mode |
| Heating | Binary Sensor | Heater active |
| Fault | Sensor | Last fault code |

## Safety

- **Temperature clamping** — commands validated against Balboa safe ranges (F: 80-104, C: 26-40), hard cap at 42°C/108°F
- **Command allowlist** — only known commands forwarded to the bus
- **Hold mode timeout** — auto-clears after 60 minutes
- **OTA rollback** — bad firmware reverts automatically

## Crates

| Crate | What it does |
|-------|-------------|
| `launa-protocol` | Balboa protocol parser, CRC-8, frame codec, status/command types |
| `launa-hal` | Hardware abstraction traits with mock impls for desktop testing |
| `launa-mqtt` | MQTT client, HA discovery, state serialization, command parsing |
| `launa-ota` | OTA update trait |
| `launa-integration-tests` | End-to-end tests with SpaSimulator |
| `app/` | ESP32 firmware binary |
| `xtask/` | Host-side tooling (reuses `launa-protocol` directly) |

```
Spa Controller ──RS-485──► ESP32 ──WiFi/MQTT──► Home Assistant
```

## License

MIT

## Acknowledgments

- [ccutrer/balboa_worldwide_app](https://github.com/ccutrer/balboa_worldwide_app) — protocol docs
- [NorthernMan54/esp32_balboa_spa](https://github.com/NorthernMan54/esp32_balboa_spa) — reference implementation
- [cribskip/esp8266_spa](https://github.com/cribskip/esp8266_spa) — original protocol reference

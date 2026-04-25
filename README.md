# Launa

`~ ˖ ೫ ˖ ~`

Open-source ESP32 firmware + MQTT broker + web UI for Balboa BP6013G1 spa controllers.

Control your pumps, temperature, lights, and blower from your phone. Set the temperature on your way home. Get notified if something goes wrong. All built in Rust, OTA-updatable, and safe enough to install without worrying about your spa.

## Architecture

```
                     ┌──────────────────┐
                     │  launa-server    │
                     │  MQTT Broker     │──── Web UI (Vue 3)
                     │  + SQLite DB     │     localhost:8080
                     └────────┬─────────┘
                              │ MQTT (TCP 1883 / WS 9001)
              ┌───────────────┼───────────────┐
              │               │               │
              ▼               ▼               ▼
         ┌─────────┐   ┌──────────┐    ┌──────────┐
         │  ESP32  │   │   Home   │    │  Any MQTT│
         │ Firmware│   │Assistant │    │  client  │
         └────┬────┘   └──────────┘    └──────────┘
              │
       RS-485 │
              ▼
     ┌─────────────────┐
     │  Balboa BP-series│
     │  Spa Controller  │
     └─────────────────┘
```

**ESP32 firmware** runs on a WROOM-32 board at the spa. It talks RS-485 to the Balboa controller and publishes state over WiFi/MQTT. It supports OTA updates — flash once over USB, then deploy remotely over WiFi.

**launa-server** is a standalone Rust binary that bundles an MQTT broker (rumqttd), a web UI, and a SQLite database for history. It runs on any Linux machine — a Raspberry Pi, a home server, or your laptop. The ESP32 and Home Assistant both connect to it.

**Web UI** is a Vue 3 + TypeScript + Tailwind app served by launa-server. It connects over MQTT WebSocket to display live spa state and controls.

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

## Prerequisites

- **Rust** — [rustup.rs](https://rustup.rs/) (stable toolchain)
- **ESP toolchain** — [espup](https://github.com/esp-rs/espup) for the Xtensa cross-compiler
  ```bash
  cargo install espup
  espup install          # installs xtensa-esp32-none-elf target
  . $HOME/export-esp.sh  # source before building the app/ crate
  ```
- **cargo-espflash** — `cargo install cargo-espflash --locked`
- **USB driver** — CP210x or CH340 driver for your board
- **Docker Desktop** — (optional) for Raspberry Pi deployment with Rosetta cross-compilation

## Getting Started

### 1. Clone and Configure

```bash
git clone https://github.com/Mik-pe/launa.git
cd launa
cp launa.example.toml launa.toml
# Edit launa.toml with your WiFi and MQTT broker details
```

`launa.toml` is gitignored. All xtask commands fail early if required fields are missing.

### 2. Build and Test Workspace Crates

All protocol and business logic crates compile for your host machine — no ESP32 needed:

```bash
cargo check                          # typecheck everything
cargo test                           # run all tests (140+ integration tests)
cargo test -p launa-protocol         # test a single crate
cargo test -p launa-integration-tests  # integration tests with SpaSimulator
```

### 3. Build and Flash the ESP32 Firmware

The `app/` crate targets `xtensa-esp32-none-elf` and is excluded from the workspace. Before building it, set up the vendored esp-nvs dependency:

```bash
./app/vendor-esp-nvs.sh              # one-time: patches esp-nvs for esp-hal 1.1
```

Then flash over USB:

```bash
cargo xtask config-flash             # write WiFi/MQTT config to ESP32 NVS (USB)
cargo xtask flash --feature sniff    # flash sniffer firmware (USB, read-only)
cargo xtask flash                    # flash full firmware (USB)
```

Or build manually:

```bash
cd app && cargo espflash flash --chip esp32 --monitor
```

### 4. Run the Server Locally

launa-server bundles an MQTT broker, web UI, and history database:

```bash
# Build the web UI first
cd web && npm install && npm run build && cd ..

# Start the server (MQTT on :1883, WebSocket on :9001, web UI on :8080)
cargo run -p launa-server
```

Or use the combined web+server script:

```bash
cd web && npm run start    # builds web, then runs launa-server
```

For development with hot-reload:

```bash
cd web && npm run dev:sim  # vite dev server + cargo watch on launa-server
```

### 5. Deploy to a Raspberry Pi

launa-server cross-compiles to `aarch64` via Docker and deploys over SSH:

```bash
cp deploy.example.sh deploy.sh
# Edit deploy.sh with your Pi's hostname and username
chmod +x deploy.sh
./deploy.sh
```

This builds a Docker image (with Rosetta cross-compilation), extracts the binary and web assets, copies them to the Pi over SSH, and installs a systemd service. After deploy:

- **MQTT**: `your-pi:1883`
- **WebSocket**: `your-pi:9001`
- **Web UI**: `http://your-pi`

Useful commands after deploying:

```bash
ssh user@your-pi 'sudo systemctl status launa-server'
ssh user@your-pi 'sudo journalctl -u launa-server -f'
```

### 6. Remote Firmware Updates (OTA)

After the first USB flash, the ESP32 stays at the spa on WiFi:

```bash
cargo xtask ota-flash --feature sniff   # deploy sniffer remotely
cargo xtask ota-flash                   # deploy full firmware remotely
```

Bad updates auto-rollback — if the new firmware crashes before connecting to MQTT, the bootloader reverts to the previous version.

## xtask Commands

Requires `launa.toml` at project root (gitignored; copy from `launa.example.toml`).

| Command | Description |
|---------|-------------|
| `cargo xtask flash [--feature sniff\|hw-test]` | Flash firmware to ESP32 via USB |
| `cargo xtask monitor` | Read serial output from ESP32 |
| `cargo xtask flash-monitor` | Flash + monitor in one command |
| `cargo xtask config-flash` | Write WiFi/MQTT config to ESP32 NVS |
| `cargo xtask ota-flash` | Build + serve + trigger OTA update over WiFi |
| `cargo xtask ota-serve` | Serve firmware `.bin` over HTTP for OTA |
| `cargo xtask sniff-decode` | Decode sniffer frames from MQTT in real-time |
| `cargo xtask spa-sim` | Simulate a spa controller over USB-RS485 |
| `cargo xtask self-test` | Run hardware self-test on ESP32 |
| `cargo xtask provision` | Burn AES key to ESP32 eFuse BLOCK3 for encrypted NVS |
| `cargo xtask listen` | Subscribe to MQTT topics from the broker |

## Home Assistant

1. Point the ESP32's MQTT config at your launa-server (or any MQTT broker)
2. Enable the MQTT integration in HA
3. Launa publishes auto-discovery messages on boot — entities appear automatically

| Entity | Type | Description |
|--------|------|-------------|
| Water Temperature | Sensor | Current water temp |
| Set Temperature | Number | Target temp |
| Heat Mode | Select | Ready / Rest / Ready-in-Rest |
| Temperature Range | Select | High / Low |
| Pump 1–6 | Switch | Toggle pumps |
| Light 1–4 | Light | Toggle spa lights |
| Blower | Fan | Toggle blower |
| Circ Pump | Switch | Circ pump state |
| Mister | Switch | Mister state |
| Hold Mode | Switch | Hold mode |
| AUX 1 / AUX 2 | Switch | Auxiliary toggles |
| Soak Mode | Switch | Soak mode |
| Normal Operation | Switch | Return to normal |
| Clear Notification | Switch | Clear alerts |
| Heating | Binary Sensor | Heater active |
| Fault | Sensor | Last fault code |
| Diagnostics | Sensor | Uptime and counters |
| Alert | Sensor | Active alerts |

## Configuration

`launa.toml` holds all device-specific settings:

```toml
[wifi]
ssid = "YourWiFiName"
password = "YourWiFiPassword"

[mqtt]
host = "192.168.1.100"     # IP or hostname of your MQTT broker
port = 1883
user = ""                  # optional
password = ""              # optional

[device]
id = "launa_spa"           # unique ID for HA entity prefixes
serial_port = "COM3"       # USB serial port for flash commands

[ota]
serve_port = 8081          # local OTA HTTP server port
host = ""                  # OTA server address (defaults to mqtt.host)
```

## Safety

- **Temperature clamping** — commands validated against Balboa safe ranges (F: 80–104, C: 26–40), hard cap at 42°C/108°F
- **Command allowlist** — only known commands forwarded to the bus
- **Hold mode timeout** — auto-clears after 60 minutes
- **OTA rollback** — bad firmware reverts automatically

## Crates

| Crate | Description |
|-------|-------------|
| `launa-protocol` | Balboa protocol parser, CRC-8, frame codec, status/command types |
| `launa-hal` | Hardware abstraction traits (async Transport, Clock) with mock impls |
| `launa-mqtt` | MQTT topics, HA discovery builder (28+ entities), command parser, MQTT v5 codec |
| `launa-ota` | OTA update trait with mock for testing |
| `launa-esp-ota` | ESP32 OTA: crypto verification, flash partition management, OTA state machine |
| `launa-core` | SpaApp with rate limiter, command tracker, pump/hold timers, diagnostics, alerting |
| `launa-sim` | Spa simulator: physics model, frame generation, state management, SimBroker |
| `launa-integration-tests` | 140+ tests exercising SpaApp through sim pipeline |
| `launa-server` | MQTT broker (rumqttd) + web UI server + SQLite history database |
| `app/` | ESP32 firmware binary (esp-hal + embassy) — excluded from workspace |
| `xtask/` | Host-side tooling (flash, monitor, OTA, sniffer, sim, self-test) |

All workspace crates are `no_std`, pure Rust, and desktop-testable. The `app/` crate is ESP32-only (`esp-hal` 1.0 + `embassy` + `esp-radio` + `rust-mqtt` + `esp-nvs`). No ESP-IDF C SDK — pure Rust throughout.

## License

MIT

## Acknowledgments

- [ccutrer/balboa_worldwide_app](https://github.com/ccutrer/balboa_worldwide_app) — protocol docs
- [NorthernMan54/esp32_balboa_spa](https://github.com/NorthernMan54/esp32_balboa_spa) — reference implementation
- [cribskip/esp8266_spa](https://github.com/cribskip/esp8266_spa) — original protocol reference

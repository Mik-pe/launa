# Launa - Task Tracker

## MQTT / Home Assistant

- [x] **Add missing HA discovery entities** (`launa-mqtt/src/discovery.rs`): Added 6 new entities: Heat Mode select, Circ Pump switch, Temperature Range select, Hold Mode switch, Mister switch, Fault sensor. Total: 14 entities.
- [x] **Add heating_mode/temp_range/hold_mode commands** (`launa-mqtt/src/command_parser.rs`): Added heat_mode, temp_range, hold_mode toggle subtopics.
- [x] **Add mister/hold_mode to state JSON** (`launa-mqtt/src/state.rs`): Added mister, hold_mode, last_fault fields to status JSON output.
- [ ] **Birth/last-will messages**: Publish `online`/`offline` to availability topic on connect/disconnect. Set LWT (Last Will and Testament) in MQTT connect options. Subscribe to `homeassistant/status` and re-publish discovery when HA restarts.
- [ ] **Set retain flag on discovery payloads**: HA auto-discovery messages should be published with `retain: true` so they survive broker restarts.

## ESP32 Firmware (`app/`) -- First Draft Complete

All modules implemented. Requires ESP-IDF toolchain to compile. Workspace tests (186) pass.

- [x] **Light color cycling**: Documented in `docs/light-colors.md`. No protocol changes needed -- each toggle advances color. Existing `ToggleItem::Light1` does the right thing.
- [x] **Timed pump toggle (P1 mode)** (`app/src/pump_timer.rs`): `PumpTimer` and `PumpTimerManager` track duration, auto-toggle off on expiry, cancel on manual off.
- [x] **UART/RS-485 transport** (`app/src/transport.rs`): `Rs485Transport` implements `launa_hal::Transport` with UART1 + DE direction pin.
- [x] **WiFi connectivity** (`app/src/wifi.rs`): Connects via `EspWifi` + `BlockingWifi` with SSID/password from NVS config.
- [x] **MQTT client** (`app/src/mqtt_client.rs`): `EspMqttClient` with LWT, discovery publish, state publish, command subscribe.
- [x] **Main event loop** (`app/src/main.rs`): UART read → frame decode → registration → status parse → MQTT publish. MQTT commands → frame encode → UART write. Pump timer ticking.
- [x] **Configuration storage** (`app/src/config.rs`): NVS-backed `AppConfig` for WiFi, MQTT, UART pins, device ID.
- [x] **OTA stub** (`app/src/ota.rs`): Placeholder implementing `OtaUpdate` trait. Real implementation needs ESP-IDF OTA APIs.
- [ ] **OTA real implementation**: Replace stub with actual `esp-idf-svc` OTA partition management. Needs dual OTA partition table (ota_0 + ota_1 + otadata). Firmware downloads new image via HTTP, writes to alternate partition, sets boot partition, reboots. This enables remote flashing (sniffer FW <-> full FW) without USB.
- [ ] **OTA partition table for `app/`**: Create `app/partitions.csv` with dual OTA slots (ota_0, ota_1, otadata). Required for any OTA updates. First flash via USB must use `--partition-table partitions.csv`.
- [ ] **OTA HTTP server on dev PC (`scripts/ota-serve.py`)**: Tiny Python HTTP server that serves firmware .bin files. ESP32 downloads from this over WiFi. Usage: (1) build firmware with `cargo espflash save-image`, (2) start `ota-serve.py` serving the .bin, (3) trigger ESP32 to download via MQTT command.
- [ ] **OTA trigger via MQTT**: Add an MQTT command topic `launa/<device_id>/ota` that accepts a JSON payload with a firmware URL. ESP32 receives it, downloads from the URL via HTTP, writes to alternate OTA partition, reboots into new firmware. If new firmware is broken, auto-rollback on next boot (ESP-IDF supports this natively).
- [ ] **One-command remote flash script (`scripts/ota-flash.ps1`)**: End-to-end script that: (1) runs `cargo test` to verify workspace passes, (2) builds `app/` for ESP32, (3) runs `cargo espflash save-image` to produce .bin, (4) starts `ota-serve.py` serving the .bin, (5) publishes MQTT OTA command to the ESP32 with the firmware URL, (6) waits for ESP32 to come back online on MQTT. Agent can call this to deploy new firmware remotely. Example: `scripts/ota-flash.ps1 -feature sniff` or `scripts/ota-flash.ps1 -feature default`.
- [ ] **Boot validation + auto-rollback**: On every boot, firmware must call `EspOta::mark_valid()` after successfully connecting to WiFi + MQTT. If it crashes or fails before that, ESP-IDF bootloader auto-rolls back to the previous partition. This ensures a bad OTA flash can't brick the device -- it always falls back to the last known-good firmware.
- [ ] **Temperature safety clamping in command builder** (`crates/launa-protocol/src/command.rs`): The `SetTemperature` command must reject values outside the spa's safe range. Per Balboa protocol: **Fahrenheit high range 80-104°F, Fahrenheit low range 50-80°F, Celsius high range 26-40°C, Celsius low range 10-26°C**. Add a `validate_set_temperature(temp, scale, range) -> Result<(), TempError>` function that clamps or rejects out-of-range values. This prevents a buggy MQTT command or Home Assistant misconfiguration from setting the heater to a dangerous temperature. Also add an optional **hard upper limit** (e.g., max 42°C / 108°F) that can never be exceeded regardless of range, as a backstop.
- [ ] **Command allowlist for MQTT commands** (`crates/launa-mqtt/src/command_parser.rs`): Only accept known command types. Reject any unrecognized or malformed MQTT payloads silently (log warning, don't send to spa). This prevents accidental bus traffic from garbage input.
- [ ] **Hold mode safety timeout** (`app/src/main.rs`): If the spa enters hold mode (which stops heating and circulation), auto-clear it after a configurable timeout (e.g., 60 minutes) unless explicitly renewed. Prevents forgetting the spa in hold mode and finding cold/unsafe water later.

## Hardware Testing & Flashing (ESP-WROOM-32)

### Architecture: What Can Be Tested Where

The `app/` crate (ESP32 glue) **cannot compile for desktop** -- it depends on `esp-idf-sys`.
But all logic lives in workspace crates that are fully desktop-testable via mocks.

| Layer | Desktop (cargo test) | ESP32 Board | With RS-485 HW |
|-------|---------------------|-------------|----------------|
| Protocol parsing/encoding | MockTransport + SpaSimulator | -- | -- |
| MQTT state JSON / discovery | MockNetwork | -- | -- |
| Command round-trip (cmd -> frame -> parse) | MockTransport | -- | -- |
| Boot / WiFi / MQTT connect | -- | Serial output | -- |
| UART + RS-485 real bus | -- | -- | Auto-dir RS485 + USB-RS485 |
| Full stack with real spa | -- | -- | At the spa |

**Strategy**: Catch all logic bugs on desktop. Use the ESP32 board to verify glue code boots and connects. RS-485 bench/field testing for protocol timing.

### Phase 0: Dev Environment Setup

- [ ] **Install USB driver**: Identify USB-to-UART chip on ESP-WROOM-32 dev board (CP210x or CH340). Install appropriate Windows VCP driver. Verify COM port appears in Device Manager.
- [ ] **Install `cargo-espflash`**: Run `cargo install cargo-espflash --locked`. Verify with `cargo espflash board-info --chip esp32`.
- [ ] **Create `xtask/` crate**: Standard cargo-xtask pattern for project tooling. Desktop-only workspace crate with `launa-protocol` as dependency (reuse frame parsing/encoding directly, no reimplementation). All host tools live here as subcommands. Usage: `cargo xtask <command> [args]`.
- [ ] **`cargo xtask flash`**: Runs `cargo espflash flash --chip esp32` (without `--monitor`), captures exit code. Non-blocking, agent-callable.
- [ ] **`cargo xtask monitor [--port COM3] [--duration 10]`**: Opens serial port at 115200 baud using `serialport` crate, reads for N seconds, prints output, exits. Agent calls this after flashing to inspect boot logs or crashes.
- [ ] **`cargo xtask flash-monitor`**: Combines flash + monitor in one command. Agent calls this to flash and see results.
- [ ] **`cargo xtask sniff-decode [--host localhost] [--port 1883]`**: Subscribes to MQTT sniff topic `launa/+/sniff`, decodes frames in real-time using `launa-protocol::StatusUpdate::parse()` directly. Shows message type, parsed fields, raw hex, CRC status. Saves session to JSON for offline analysis. Agent can run this to inspect real spa traffic remotely.
- [ ] **`cargo xtask spa-sim [--port COM5]`**: Talks to USB-to-RS485 adapter via `serialport`. Uses `launa-protocol::FrameEncoder` and `SpaSimulator` frame generation logic to send real Balboa frames. Repeatedly sends status updates at 1-second intervals. Optionally responds to commands. Agent can run this for bench testing.
- [ ] **`cargo xtask ota-serve [--firmware path/to/fw.bin] [--port 8080]`**: Tiny HTTP server (using `tiny_http` or `actix-web`) that serves firmware .bin files. ESP32 downloads from this over WiFi. Used by `ota-flash` below.
- [ ] **`cargo xtask ota-flash [--feature sniff|default] [--device-id launa_spa]`**: End-to-end remote flash: (1) runs `cargo test` to verify workspace, (2) builds `app/` for ESP32 with given feature, (3) runs `cargo espflash save-image` to produce .bin, (4) starts `ota-serve` in background, (5) publishes MQTT OTA command to ESP32 with firmware URL, (6) waits for ESP32 to come back online on MQTT. Agent calls this to deploy new firmware remotely. Auto-rollback if new firmware fails.
- [ ] **`cargo xtask self-test`**: Builds `app/` with `--features hw-test`, flashes via USB, captures serial output, parses `TEST_PASS`/`TEST_FAIL:<reason>` lines, reports summary. Agent uses this to validate hardware.
- [ ] **Local config via `launa.toml` (gitignored)**: All secrets and device-specific config live in `launa.toml` at project root (gitignored). Contains WiFi SSID/password, MQTT broker host/port/user/password, ESP32 serial port, device ID, OTA server port. **All xtask commands that need config must parse this file first and exit with a clear error if it's missing or has empty required fields** -- no silent defaults, no placeholder values in firmware. Commit a `launa.example.toml` with placeholder values so the format is documented. Example:
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
- [ ] **`cargo xtask config-flash`**: Reads `launa.toml` and writes WiFi/MQTT/device config to ESP32 NVS via serial. Only needed on first setup or when changing credentials. After this, the ESP32 has its config stored in NVS and doesn't need `launa.toml` to boot.
- [ ] **Document xtask commands in AGENTS.md**: Add a "Project Commands" section listing all `cargo xtask` subcommands with examples. Document the `launa.toml` config format and that it must be created from `launa.example.toml`.

### Phase 2: Desktop End-to-End Test (No HW Needed)

Expand existing `launa-integration-tests` to simulate the full data pipeline on PC. This catches logic bugs before any flashing.

- [ ] **Full pipeline integration test**: Test the complete data flow that the ESP32 would execute: SpaSimulator generates status frame -> FrameDecoder parses -> StatusUpdate extracted -> `status_to_json()` produces MQTT payload -> assert JSON fields match simulator state. This simulates what `app/main.rs` does without needing the board.
- [ ] **Command round-trip test**: MQTT command string -> `parse_command()` -> `Command` -> `encode()` -> frame bytes -> SpaSimulator `process_incoming()` -> verify state change -> generate new status -> verify updated JSON.
- [ ] **HA discovery validation test**: Generate all 14 discovery payloads, validate they are valid JSON with correct `~` HA discovery topic format, correct `unique_id`, `command_topic`, `state_topic` patterns.
- [ ] **Registration flow test**: Simulate full client ID registration: SpaSimulator sends query -> RegistrationStateMachine processes -> sends ID request -> receives assignment -> sends ack -> `is_registered()` returns true.

### Phase 3: Protocol Sniffer (First Thing at the Spa)

**DO NOT skip this step.** Before sending any commands, we need to verify that our protocol
understanding matches the real BP6013G1. The protocol docs are reverse-engineered by the
community and may have subtle errors in byte offsets or flag positions.

**Remote workflow**: The ESP32 stays at the spa permanently (powered by USB charger). It publishes
raw frames to MQTT over WiFi. You sit at your desk and run `scripts/sniff-decode.py` which subscribes
to the MQTT sniff topic and decodes everything live. No need to be physically at the spa.

- [ ] **Build sniffer firmware (`app/src/main.rs` with `#[cfg(feature = "sniff")]`)**: When enabled, the firmware: (1) connects to WiFi + MQTT, (2) reads ALL frames from RS-485 passively, (3) never sends anything on the bus (no registration, no commands), (4) for each frame: publishes raw hex + decoded message type to MQTT topic `launa/<device_id>/sniff` as JSON (includes timestamp, raw bytes, message type, CRC pass/fail), (5) also logs to serial for debugging, (6) runs indefinitely.
- [ ] **Build sniffer dashboard/decoder (`scripts/sniff-decode.py`)**: Python script that subscribes to the MQTT sniff topic remotely and decodes frames in real-time on your PC: shows message type, parsed fields (temperature, pumps, flags), raw hex dump, CRC status. Highlights any frames that fail CRC or have unrecognized types. Can save session to JSON file for offline analysis. Agent can run this to inspect real spa traffic.
- [ ] **First field session: passive sniff (safe)**: Flash sniffer FW via USB at your desk, then take ESP32 + RS-485 module + USB charger to the spa. Connect A/B to the controller's bus. Plug in USB charger. Go back to your desk. Run `sniff-decode.py` remotely. Collect 30+ seconds of frames. Verify: (1) we see valid 0x7E-delimited frames (not garbage = A/B polarity correct), (2) status updates arrive every ~1s, (3) byte offsets match our parser assumptions, (4) message types match what we expect (FF AF 13, FE BF, etc.).
- [ ] **Validate parser against real frames**: Take the sniffed raw hex data (saved JSON from decoder), feed it through `launa-protocol::StatusUpdate::parse()` in a desktop test, verify the parsed values make sense (temperature, pump states, etc. match what the spa display shows).
- [ ] **Document real protocol findings**: After sniffing, update `docs/protocol.md` and `docs/bp6013g1.md` with any differences found between the reference docs and real behavior. Fix any parser bugs discovered.

### Phase 4: RS-485 Bench Testing (Requires USB-RS485 Adapter)

With the protocol validated from sniffing, now test sending commands on the bench.
You have an **auto-direction RS-485 module** (VCC/GND/TX/RX/A/B only -- no DE pin).

**Bench setup -- just two wires between the modules:**
```
PC (Python script simulating spa)
  |
  | USB cable
  |
[USB-to-RS485 adapter]        <-- ~$5-10 on Amazon
  |           |
  A           B               (just A-A, B-B jumper wires)
  |           |
[Your auto-direction RS-485 module]
  |       |
  TX      RX                  (direct to ESP32 GPIO16/17)
  |       |
[ESP-WROOM-32 dev board]     (also: VCC=3.3V, GND=GND)
```

- [ ] **Order USB-to-RS485 adapter for PC**: Any USB-to-RS485 dongle works (search "USB to RS485 adapter" on Amazon, ~$5-10). This lets your PC talk RS-485 to simulate the spa controller.
- [ ] **Update `Rs485Transport` to support auto-direction modules**: Make the DE pin optional. When no DE pin is configured (or set to -1), skip the GPIO toggle logic. The auto-direction module handles it in hardware.
- [ ] **Wire the bench setup**: ESP32 GPIO16 (TX) -> module TX, GPIO17 (RX) -> module RX, 3.3V -> VCC, GND -> GND. USB-RS485 adapter A -> module A, adapter B -> module B. That's 6 wires total.
- [ ] **Build PC-side RS-485 spa simulator (`scripts/spa-sim.py`)**: Python script using `pyserial` that talks to USB-to-RS485 adapter. Sends real Balboa frames (port `SpaSimulator` frame generation logic to Python). Repeatedly sends status updates at 1-second intervals. Optionally responds to commands. Agent can run this to test the full stack.
- [ ] **RS-485 loopback integration test**: With bench setup connected, run: (1) flash launa-app to ESP32, (2) start `spa-sim.py` on PC, (3) ESP32 parses frames over real UART, (4) ESP32 publishes to MQTT, (5) validate MQTT payload matches what spa-sim sent. Agent can automate this end-to-end.

### Phase 5: Active Field Testing (Real Spa, Sending Commands)

- [ ] **Field test at spa (full stack)**: With protocol validated from sniffing and bench-tested, take ESP32 + RS-485 module to the spa. This time the firmware runs the full stack: registration, status parsing, MQTT publishing, and accepting commands from Home Assistant. Verify temperature readings, pump control, and all entities work correctly.

## Done

- [x] Project structure and workspace setup
- [x] Git repo initialized and pushed to GitHub
- [x] Balboa CRC-8 implementation with tests
- [x] Frame encode/decode with streaming decoder
- [x] Status update parser (temperature, pumps, lights, heating, mister, hold, priming)
- [x] Command builder with correct sub-type bytes (toggle, set temp, set time, settings, etc.)
- [x] Spa configuration parser (pump/light/blower/circ capabilities)
- [x] Client ID registration state machine
- [x] Hardware abstraction traits with mock implementations
- [x] Home Assistant MQTT auto-discovery builder (14 entities: sensor, number, binary_sensor, 3x switch, light, fan, 2x select, 3x switch, sensor)
- [x] OTA update trait with mock
- [x] ESP32 app skeleton
- [x] Protocol documentation, BP6013G1 notes, architecture docs
- [x] Information response parser (`0A BF 24`)
- [x] Fault log response parser (`0A BF 28`) with 18 fault codes
- [x] Filter cycles response parser (`0A BF 23`)
- [x] `0A BF` message dispatcher with sub-type discrimination
- [x] State serialization (`status_to_json`) with all fields
- [x] Command parsing (pump1/2/3, light1, blower, set_temperature)
- [x] Topic builder (state, command, availability, discovery, OTA)
- [x] Integration tests with SpaSimulator (49 tests)
- [x] Fuzz tests (27 tests) and property tests (17 tests)
- [x] All 186 tests passing

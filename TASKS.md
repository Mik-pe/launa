# Launa - Task Tracker

## MQTT / Home Assistant

- [x] **Add missing HA discovery entities** (`launa-mqtt/src/discovery.rs`): Added 6 new entities: Heat Mode select, Circ Pump switch, Temperature Range select, Hold Mode switch, Mister switch, Fault sensor. Total: 14 entities.
- [x] **Add heating_mode/temp_range/hold_mode commands** (`launa-mqtt/src/command_parser.rs`): Added heat_mode, temp_range, hold_mode toggle subtopics.
- [x] **Add mister/hold_mode to state JSON** (`launa-mqtt/src/state.rs`): Added mister, hold_mode, last_fault fields to status JSON output.
- [x] **Birth/last-will messages**: Added `LwtConfig`, `BirthConfig`, `lwt_config()`, `birth_config()` in `launa-mqtt/src/topics.rs`. LWT publishes "offline" to availability topic on ungraceful disconnect (retain=true, QoS 1). Birth publishes "online" after connect (retain=true, QoS 1). Added `ha_status_topic()` for subscribing to `homeassistant/status` to re-publish discovery on HA restart. (Actual MQTT connect wiring deferred to `app/` crate.)
- [x] **Set retain flag on discovery payloads**: Added `DiscoveryMessage` struct with `retain` field and `build_with_retain()` method on `DiscoveryBuilder`. All 14 discovery messages can now be published with `retain=true` via `build_with_retain()`.

## Architecture: esp-hal + embassy (pure Rust, no_std)

The `app/` crate uses `esp-hal` + `embassy` — a pure Rust, no_std stack for ESP32. No C SDK dependency. Embassy provides `select!` for concurrent UART + MQTT. Workspace crates (launa-protocol, launa-hal, launa-mqtt, launa-ota) are unaffected.

### Dependency map:

| Need | Crate | Notes |
|---|---|---|
| HAL (UART, GPIO) | `esp-hal` 1.0+ | Stable |
| WiFi | `esp-radio` | `unstable` feature, works on ESP32 |
| TCP/IP | `embassy-net` (smoltcp) | Async network stack |
| TLS | `esp-mbedtls` | no_std mbedtls wrapper |
| MQTT | `rust-mqtt` | MQTTv5, no_std |
| OTA | `launa-esp-ota` (TODO) | Custom: esp-storage + partition mgmt + rollback |
| NVS | `esp-nvs` | ESP-IDF compatible format, bare metal |
| Async executor | `embassy` + `esp-rtos` | esp-rtos provides scheduler (required by esp-radio) + embassy bridge |
| Time | `embassy-time` 0.5 | Via esp-hal timer driver (aligned with esp-rtos 0.2) |

### Implementation tasks:

- [x] **`app/Cargo.toml`**: `esp-hal`, `esp-hal-embassy`, `esp-radio`, `embassy-*`, `rust-mqtt`, `esp-hal-ota`, `esp-nvs`. Embassy `#[main]` macro. Target `xtensa-esp32-none-elf`.
- [x] **UART transport** (`app/src/transport.rs`): `esp_hal::uart::Uart` with async mode. Embassy `#[main]` passes peripherals (no `Peripherals::take()`). Optional DE pin for auto-direction RS-485 modules.
- [x] **WiFi** (`app/src/wifi.rs`): `esp_radio` + `embassy_net` async WiFi stack. DHCP via `embassy_net::Config::dhcpv4`.
- [x] **MQTT client** (`app/src/mqtt_client.rs`): Custom MQTT v5 over `embassy_net` TCP. Connect, publish, subscribe, LWT. Hand-rolled MQTT packets for no_std. 14-entity HA discovery generation. Command parsing via `launa-mqtt` no_std APIs.
- [x] **OTA** (`app/src/ota.rs`): `esp_hal_ota::Ota` with `esp_storage::FlashStorage`. OTA URL parsing, partition table. HTTP download pending embassy-net TCP wiring.
- [x] **Config** (`app/src/config.rs`): `esp_nvs::Nvs` for key-value storage.
- [x] **Main event loop** (`app/src/main.rs`): Embassy `#[main]` with `spawner`. `select!` to concurrently: (1) read UART frames, (2) handle MQTT messages, (3) publish state, (4) track commands, (5) tick pump/hold timers.
- [x] **Pump timers as embassy tasks** (`app/src/pump_timer.rs`): `PumpTimer` and `PumpTimerManager` track duration, auto-toggle off on expiry, cancel on manual off. Uses embassy `Instant` and `Duration`.
- [x] **`launa-hal` async traits**: `Transport::read()` uses `embedded-io-async::Read` since embassy UART is async. Workspace crates use `embedded-io` / `embedded-io-async` traits.
- [x] **`launa-mqtt` no_std compatibility**: `command_parser`, `state` (status_to_json), and `topics` modules now work in no_std without serde/serde_json. Manual JSON generation for status serialization. Compiles with `--no-default-features`.

### Risks:

- `esp-radio` API may change (behind `unstable` feature flag) — pin versions in Cargo.lock
- `rust-mqtt` is community-maintained — may need to patch or contribute upstream

### Migration: esp-hal-embassy -> esp-rtos (completed)

- [x] **Replace `esp-hal-embassy` with `esp-rtos`**: Removed deprecated `esp-hal-embassy 0.9.1`, added `esp-rtos 0.2.0` (required by `esp-radio` for WiFi scheduler + embassy bridge). Updated `#[esp_hal_embassy::main]` -> `#[esp_rtos::main]`, `esp_hal_embassy::init(timer)` -> `esp_rtos::start(timer, sw_int)`.
- [x] **Bump `embassy-executor` to 0.9**: Matches `esp-rtos 0.2.0` dependency.
- [x] **Remove `esp-hal-ota` (broken)**: `esp-hal-ota 0.4.6` uses `concat_idents` removed in Rust 1.90. Stubbed out `app/src/ota.rs`, will be replaced by `launa-esp-ota` crate.
- [x] **Fix `byteorder` for no_std**: Added `default-features = false` to workspace dep.
- [x] **Fix `thiserror 2` for no_std**: Added `default-features = false` (thiserror 2 supports no_std with that flag).
- [x] **Fix `launa-ota` missing `#![no_std]`**: Added `#![no_std]` to crate root.
- [x] **Fix `launa-hal` network module behind `std` feature**: Gated `network` module behind `#[cfg(feature = "std")]`.
- [x] **Delete stale `build.rs`**: Removed old `embuild`-based build script from esp-idf-svc era.
- [x] **Fix `esp-storage` opt-level**: Override `opt-level = 3` for `esp-storage` in dev profile.

### Done: Fix app/ API mismatches (all resolved)

The app code was written against older esp-radio/esp-nvs/embassy-net/embassy-sync APIs. After the esp-rtos migration, dependency resolution succeeds but the app code has API drift:

- [x] **`app/src/wifi.rs`**: Updated for esp-radio 0.17 API: `wifi::new()` now takes `&Controller` from `esp_radio::init()`, uses `controller.set_config(&ModeConfig::Client(...))` + `start_async()` + `connect_async()`, `interfaces.sta` instead of `.station`, removed `wait_for_disconnect_async()`, `Runner<'static, WifiDevice<'static>>` for net task.
- [x] **`app/src/transport.rs`**: Updated for esp-hal 1.0: `GpioPin` -> `AnyPin`, `mode::Async` -> `esp_hal::Async`, `Uart::new()` returns `Result` (handled with `.expect()`), `embedded_io_async::Write::write` returns `Result<usize>`.
- [x] **`app/src/mqtt_client.rs`**: Updated for embassy-net 0.7: `TcpSocket::new(stack, &mut rx_buf, &mut tx_buf)` with explicit buffers, `IpAddress::Ipv4(Ipv4Address::from_bytes(...))` for IP endpoint construction.
- [x] **`app/src/config.rs`**: Updated for esp-nvs 0.4: `Nvs::new(partition_offset, partition_size, flash)` with `FlashStorage` as `Platform` impl, `Key::from_str()` for key construction, `nvs.get::<T>(&ns_key, &key)` and `nvs.set(&ns_key, &key, value)` for typed get/set.
- [x] **`app/src/main.rs`**: `NoopRawMutex` -> `CriticalSectionRawMutex`, `esp_rtos::start(timer)` on xtensa (no software interrupt), `esp_radio::init()` before WiFi, `Rng::new()` takes no params, `Uart::new()` result handled, GPIO4 DE pin converted with `.into()`.
- [x] **Resolve `embassy-time` version split**: Unified to `0.5` in `app/Cargo.toml`.
- [x] **Resolve `embassy-sync` version split**: Unified to `0.7` in `app/Cargo.toml`.
- [x] **Add `esp-backtrace` + panic handler**: Added `esp-backtrace = "0.15"` with `esp32`, `panic-handler`, `exception-handler`, `print-uart` features.

## ESP32 Firmware (`app/`) -- In Progress

Built on esp-hal + embassy (pure Rust, no_std). Workspace tests pass.

- [x] **Light color cycling**: Documented in `docs/light-colors.md`. No protocol changes needed -- each toggle advances color. Existing `ToggleItem::Light1` does the right thing.
- [x] **Timed pump toggle (P1 mode)** (`app/src/pump_timer.rs`): `PumpTimer` and `PumpTimerManager` track duration, auto-toggle off on expiry, cancel on manual off. Will be rewritten as embassy task.

### OTA tasks (apply to new esp-hal stack):

- [x] **OTA partition table for `app/`**: Created `app/partitions.csv` with dual OTA slots (ota_0 at 0x20000/1.75MB, ota_1 at 0x1E0000/1.75MB, otadata at 0x10000). Required for OTA. First flash via USB must use `--partition-table partitions.csv`.
- [ ] **`launa-esp-ota` crate**: Custom ESP32 OTA implementation replacing `esp-hal-ota` (broken with nightly >=1.90 due to removed `concat_idents` feature). Use `esp-storage` directly for flash writes. Implement: partition table parsing (read `partitions.csv` offsets), MMU address mapping for ESP32, flash erase/write to inactive partition, boot partition swap via otadata, `mark_app_valid()` for rollback prevention. ~200-300 lines. Add as `crates/launa-esp-ota/` with `OtaUpdate` trait impl from `launa-ota`.
- [ ] **OTA real implementation**: Use `launa-esp-ota` with `esp_storage::FlashStorage`. HTTP download via embassy-net, write chunks to alternate partition, verify CRC, reboot. OTA module has URL parsing and partition write skeleton; HTTP download over embassy-net TCP still pending.
- [x] **OTA HTTP server on dev PC** (`cargo xtask ota-serve`): Already implemented in xtask. Serves firmware .bin files over HTTP for ESP32 to download.
- [x] **OTA trigger via MQTT**: MQTT subscribes to `launa/<device_id>/ota` topic. Accepts JSON with firmware URL (`{"url":"http://..."}`). Simple JSON parser extracts URL. OTA update initiated from MQTT task. Auto-rollback if new firmware is broken.
- [x] **One-command remote flash script** (`cargo xtask ota-flash`): Already implemented in xtask. Build + serve + trigger OTA remotely.
- [x] **Boot validation + auto-rollback**: On every boot, `EspOta::mark_valid()` called after WiFi + MQTT connect succeeds. If firmware crashes before marking valid, bootloader auto-rolls back.

### Safety & robustness (stack-independent):

- [x] **Command ACK / status verification for SET commands** (`app/src/command_tracker.rs`): `CommandTracker` struct maps pending commands to expected state transitions. When a SET command is sent, the expected change (e.g., pump on/off, temperature set) is recorded with a timestamp. Each incoming `StatusUpdate` is checked against pending commands. If confirmed within 5 seconds, the command is removed. If timeout occurs, the command is retried up to 2 times. After max retries, the command is dropped and a warning is logged. This prevents stale HA UI state when commands are lost on the bus or rejected by the spa.
- [x] **Temperature safety clamping in command builder** (`crates/launa-protocol/src/command.rs`): Added `validate_set_temperature(temp, scale, range) -> Result<u8, TempError>` function that validates against Balboa safe ranges (F° high: 80-104, F° low: 50-80, C° high: 26-40, C° low: 10-26) with a hard upper limit of 108°F / 42°C. Also added `parse_set_temperature_validated()` in `command_parser.rs` that validates temperature MQTT commands against the current scale/range before producing a `Command`.
- [x] **Command allowlist for MQTT commands** (`crates/launa-mqtt/src/command_parser.rs`): Replaced `parse_command` returning `Option<Command>` with `ParseResult` enum (`Valid`, `TemperatureOutOfRange`, `UnknownSubtopic`, `InvalidPayload`). Added explicit `ALLOWED_SUBTOPICS` list — only 9 known subtopics are accepted. Unknown subtopics return `UnknownSubtopic` instead of silently dropping. Non-UTF8 payloads are rejected. Backward-compatible `parse_command_ok()` provided.
- [x] **Hold mode safety timeout** (`app/src/pump_timer.rs`): `HoldModeTimer` auto-clears hold mode after 60 minutes (configurable). Tracks when hold mode is entered and checks on each status update. If the spa remains in hold mode beyond the timeout, sends a toggle command to clear it. Prevents forgetting the spa in hold mode and finding cold/unsafe water later.

## Hardware Testing & Flashing (ESP-WROOM-32)

### Architecture: What Can Be Tested Where

The `app/` crate (ESP32 glue) **cannot compile for desktop** -- it targets `xtensa-esp32-none-elf`.
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
- [x] **Create `xtask/` crate**: Standard cargo-xtask pattern for project tooling. Desktop-only workspace crate with `launa-protocol` as dependency (reuse frame parsing/encoding directly, no reimplementation). All host tools live here as subcommands. Usage: `cargo xtask <command> [args]`.
- [x] **`cargo xtask flash`**: Runs `cargo espflash flash --chip esp32` (without `--monitor`), captures exit code. Non-blocking, agent-callable.
- [x] **`cargo xtask monitor [--port COM3] [--duration 10]`**: Opens serial port at 115200 baud using `serialport` crate, reads for N seconds, prints output, exits. Agent calls this after flashing to inspect boot logs or crashes.
- [x] **`cargo xtask flash-monitor`**: Combines flash + monitor in one command. Agent calls this to flash and see results.
- [x] **`cargo xtask sniff-decode [--host localhost] [--port 1883]`**: Subscribes to MQTT sniff topic `launa/+/sniff`, decodes frames in real-time using `launa-protocol::StatusUpdate::parse()` directly. Shows message type, parsed fields, raw hex, CRC status. Saves session to JSON for offline analysis. Agent can run this to inspect real spa traffic remotely.
- [x] **`cargo xtask spa-sim [--port COM5]`**: Talks to USB-to-RS485 adapter via `serialport`. Uses `launa-protocol::FrameEncoder` and `SpaSimulator` frame generation logic to send real Balboa frames. Repeatedly sends status updates at 1-second intervals. Optionally responds to commands. Agent can run this for bench testing.
- [x] **`cargo xtask ota-serve [--firmware path/to/fw.bin] [--port 8080]`**: Tiny HTTP server (using `tiny_http` or `actix-web`) that serves firmware .bin files. ESP32 downloads from this over WiFi. Used by `ota-flash` below.
- [x] **`cargo xtask ota-flash [--feature sniff|default] [--device-id launa_spa]`**: End-to-end remote flash: (1) runs `cargo test` to verify workspace, (2) builds `app/` for ESP32 with given feature, (3) runs `cargo espflash save-image` to produce .bin, (4) starts `ota-serve` in background, (5) publishes MQTT OTA command to ESP32 with firmware URL, (6) waits for ESP32 to come back online on MQTT. Agent calls this to deploy new firmware remotely. Auto-rollback if new firmware fails.
- [x] **`cargo xtask self-test`**: Builds `app/` with `--features hw-test`, flashes via USB, captures serial output, parses `TEST_PASS`/`TEST_FAIL:<reason>` lines, reports summary. Agent uses this to validate hardware.
- [x] **Local config via `launa.toml` (gitignored)**: All secrets and device-specific config live in `launa.toml` at project root (gitignored). Contains WiFi SSID/password, MQTT broker host/port/user/password, ESP32 serial port, device ID, OTA server port. **All xtask commands that need config must parse this file first and exit with a clear error if it's missing or has empty required fields** -- no silent defaults, no placeholder values in firmware. Commit a `launa.example.toml` with placeholder values so the format is documented. Example:
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
- [x] **`cargo xtask config-flash`**: Reads `launa.toml` and writes WiFi/MQTT/device config to ESP32 NVS via serial. Only needed on first setup or when changing credentials. After this, the ESP32 has its config stored in NVS and doesn't need `launa.toml` to boot.
- [ ] **Document xtask commands in AGENTS.md**: Add a "Project Commands" section listing all `cargo xtask` subcommands with examples. Document the `launa.toml` config format and that it must be created from `launa.example.toml`.

### Phase 2: Desktop End-to-End Test (No HW Needed)

Expand existing `launa-integration-tests` to simulate the full data pipeline on PC. This catches logic bugs before any flashing.

- [x] **Full pipeline integration test**: `test_full_pipeline_status_frame_to_mqtt_json` — SpaSimulator generates status frame -> FrameDecoder parses -> StatusUpdate extracted -> `status_to_json()` produces MQTT payload -> assert all JSON fields match simulator state.
- [x] **Command round-trip test**: `test_command_round_trip_pump_toggle` and `test_command_round_trip_set_temperature` — MQTT command string -> `parse_command_ok()` -> `Command` -> `encode()` -> frame bytes -> SpaSimulator `process_incoming()` -> verify state change -> generate new status -> verify updated JSON.
- [x] **HA discovery validation test**: `test_ha_discovery_full_validation` — Generate all 14 discovery payloads, validate they are valid JSON with correct `homeassistant/<component>/<device_id>/<object_id>/config` topic format, correct `unique_id`, `command_topic`, `state_topic` patterns, no duplicate topics.
- [x] **Registration flow test**: `test_registration_flow_with_state_machine` — Simulate full client ID registration: RegistrationStateMachine processes FE BF 00 query -> SendIdRequest -> receives FE BF 02 assignment -> SendIdAck -> `is_registered()` returns true.

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
- [x] **Update `Rs485Transport` to support auto-direction modules**: DE pin is already optional (`Option<GpioPin>`). When no DE pin is configured, GPIO toggle logic is skipped. The auto-direction module handles it in hardware.
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
- [x] **Status parser byte offsets corrected** (`crates/launa-protocol/src/status.rs`): Fixed all byte offsets to match real Balboa BP6013G1 hardware (verified against NorthernMan54/esp32_balboa_spa). Hold=offset 0, Priming=offset 1, Heating Mode=offset 5, flags=offset 9/10, pumps=offset 11, circ/blower=offset 13, lights=offset 14, mister=offset 15.
- [x] **HDLC byte stuffing** (`crates/launa-protocol/src/frame.rs`): Added escape handling for `0x7E` and `0x7D` bytes in frame encoder/decoder to prevent CRC/data bytes from being interpreted as frame markers.
- [x] **Spa simulator offsets corrected** (`crates/launa-sim/src/spa_sim.rs`, `crates/launa-integration-tests/src/spa_simulator.rs`): Updated `generate_status_frame()` to use correct real-hardware byte offsets.
- [x] **All 278 tests passing** (18 HAL + 54 integration + 30 sim + 44 MQTT + 67 protocol + 27 fuzz + 17 property + 21 sim-unit)
- [x] **Temperature safety clamping** (`crates/launa-protocol/src/command.rs`): Added `validate_set_temperature()` with Balboa-safe ranges and hard upper limit (108°F / 42°C). 13 new tests.
- [x] **Command allowlist + ParseResult** (`crates/launa-mqtt/src/command_parser.rs`): `parse_command()` now returns `ParseResult` with `Valid`, `TemperatureOutOfRange`, `UnknownSubtopic`, `InvalidPayload` variants. `parse_command_ok()` for backward compat. 10 new tests.
- [x] **Discovery retain support** (`crates/launa-mqtt/src/discovery.rs`): Added `DiscoveryMessage` struct and `build_with_retain()`. 4 new tests (retain, topics, unique_ids, command_topics).
- [x] **Birth/last-will MQTT config** (`launa-mqtt/src/topics.rs`): Added `LwtConfig`, `BirthConfig`, `lwt_config()`, `birth_config()`, `ha_status_topic()`. 10 new tests.
- [x] **Phase 2 desktop e2e tests** (`crates/launa-integration-tests/src/lib.rs`): 4 new tests covering full pipeline, command round-trip, HA discovery validation, and registration flow.
- [x] **`.cargo/config.toml`**: Added `cargo xtask` alias for standard cargo-xtask workflow.
- [x] **`launa-mqtt` no_std compatible**: `command_parser`, `state` (status_to_json), and `topics` modules work without `std` feature. Uses `alloc` for `String`/`Vec` and manual JSON generation (no serde_json). Compiles with `--no-default-features`. Boolean values in JSON now proper `true`/`false` instead of string `"true"`/`"false"`.
- [x] **MQTT state publishing wired up** (`app/src/main.rs`): `STATE_CHANNEL` carries `StatusUpdate` from main event loop to MQTT task, which serializes via `status_to_json()` and publishes to `launa/<device_id>/state`.
- [x] **HA discovery in no_std** (`app/src/mqtt_client.rs`): Hardcoded JSON generation for all 14 HA discovery configs. Published with retain=true on startup and when HA comes back online (subscribed to `homeassistant/status`).
- [x] **OTA partition table** (`app/partitions.csv`): Dual OTA slots (ota_0/ota_1, 1.75MB each) + otadata + nvs + phy_init.
- [x] **OTA MQTT trigger** (`app/src/mqtt_client.rs`, `app/src/ota.rs`): Subscribes to `launa/<device_id>/ota`, parses `{"url":"http://..."}` payload, initiates OTA update. URL parsing implemented; HTTP download over embassy-net pending.
- [x] **Boot validation + auto-rollback** (`app/src/main.rs`): `EspOta::mark_valid()` called after WiFi + MQTT connect. Prevents rollback on successful boot.
- [x] **Command ACK / status verification** (`app/src/command_tracker.rs`): `CommandTracker` verifies SET commands against subsequent status updates. 5-second timeout, 2 retries max. Tracks expected state changes (pump on/off, temperature set, toggles).
- [x] **Hold mode safety timeout** (`app/src/pump_timer.rs`): `HoldModeTimer` auto-clears hold mode after 60 minutes. Prevents cold/unsafe water from forgotten hold mode.
- [x] **HA status subscription for re-publishing discovery**: MQTT task subscribes to `homeassistant/status`. When HA publishes "online", discovery configs + availability are re-published.
- [x] **Fix app/ API mismatches** (`app/src/*.rs`, `app/Cargo.toml`): Fixed all API drift for esp-radio 0.17, esp-hal 1.0, esp-nvs 0.4, embassy-net 0.7, embassy-time 0.5, embassy-sync 0.7. Added `esp-backtrace` for panic handling. All 278 workspace tests still pass.

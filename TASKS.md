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
| TLS | _not needed_ | Private WiFi only -- all MQTT/OTA traffic is local |
| MQTT | hand-rolled MQTT v5 | Removed `rust-mqtt` (unused). All MQTT is hand-rolled packet construction with proper QoS, keepalive, reconnect, and reassembly. |
| OTA | `launa-esp-ota` (TODO) | Custom: esp-storage + partition mgmt + rollback |
| NVS | `esp-nvs` | ESP-IDF compatible format, bare metal |
| Async executor | `embassy` + `esp-rtos` | esp-rtos provides scheduler (required by esp-radio) + embassy bridge |
| Time | `embassy-time` 0.5 | Via esp-hal timer driver (aligned with esp-rtos 0.2) |

### Implementation tasks:

- [x] **`app/Cargo.toml`**: `esp-hal`, `esp-hal-embassy`, `esp-radio`, `embassy-*`, `rust-mqtt`, `esp-hal-ota`, `esp-nvs`. Embassy `#[main]` macro. Target `xtensa-esp32-none-elf`.
- [x] **UART transport** (`app/src/transport.rs`): `esp_hal::uart::Uart` with async mode. Embassy `#[main]` passes peripherals (no `Peripherals::take()`). Optional DE pin for auto-direction RS-485 modules.
- [x] **WiFi** (`app/src/wifi.rs`): `esp_radio` + `embassy_net` async WiFi stack. DHCP via `embassy_net::Config::dhcpv4`.
- [x] **MQTT client** (`app/src/mqtt_client.rs`): Hand-rolled MQTT v5 over `embassy_net` TCP. Connect with username/password, publish (QoS 0/1) with packet IDs, subscribe, keepalive PINGREQ, incoming PUBACK for QoS 1, packet reassembly, reconnect with re-subscribe. 14-entity HA discovery generation with boolean payload_on/payload_off. Command parsing via `launa-mqtt` no_std APIs. `MqttAction` enum for commands vs pump timers. Removed `rust-mqtt` dependency (was unused).
- [x] **OTA** (`app/src/ota.rs`): `esp_hal_ota::Ota` with `esp_storage::FlashStorage`. OTA URL parsing, partition table. HTTP download pending embassy-net TCP wiring.
- [x] **Config** (`app/src/config.rs`): `esp_nvs::Nvs` for key-value storage.
- [x] **Main event loop** (`app/src/main.rs`): Embassy `#[main]` with `spawner`. Commands are only sent when a Ready message arrives from the spa (Balboa protocol pacing). On Ready: dequeue one command and send, or send NothingToSend if queue empty. Three async tasks: UART, MQTT, and main event loop.
- [x] **Pump timers as embassy tasks** (`app/src/pump_timer.rs`): `PumpTimer` and `PumpTimerManager` track duration, auto-toggle off on expiry, cancel on manual off. Uses embassy `Instant` and `Duration`.
- [x] **`launa-hal` async traits**: `Transport::read()` uses `embedded-io-async::Read` since embassy UART is async. Workspace crates use `embedded-io` / `embedded-io-async` traits.
- [x] **`launa-mqtt` no_std compatibility**: `command_parser`, `state` (status_to_json), and `topics` modules now work in no_std without serde/serde_json. Manual JSON generation for status serialization. Compiles with `--no-default-features`.

### Risks:

- `esp-radio` API may change (behind `unstable` feature flag) — pin versions in Cargo.lock

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

## P0: MQTT Protocol Correctness (All Fixed)

The hand-rolled MQTT client in `app/src/mqtt_client.rs` had multiple protocol bugs. All have been fixed. The `rust-mqtt` dependency was removed (it was never used).

- [x] **Fix MQTT QoS 1: send PUBACK for incoming PUBLISH packets**: `process_packet()` now detects QoS > 0 on incoming PUBLISH, extracts the packet identifier, and sends PUBACK `[0x40, 0x02, pkt_id_hi, pkt_id_lo]`.
- [x] **Fix MQTT QoS 1: wait for PUBACK on outgoing PUBLISH**: `publish()` now correctly writes the packet identifier bytes after the topic for QoS > 0 packets. Uses monotonically increasing packet ID counter.
- [x] **Fix MQTT packet identifier generation**: Added `next_packet_id: u16` field and `allocate_packet_id()` method. Counter wraps 65535->1 (never 0). Used by both `publish()` and `subscribe()`.
- [x] **Add MQTT keepalive PINGREQ**: `last_outgoing: Instant` field tracks last send time. `maybe_ping()` sends `[0xC0, 0x00]` (PINGREQ) if idle > keepalive/2 (15 seconds). Called in `recv()` loop before each read attempt.
- [x] **Add MQTT username/password to CONNECT packet**: `send_connect()` now takes `Option<&str>` for username/password. Sets connect flags bits 7/6 conditionally. Appends length-prefixed strings after will payload.
- [x] **Add MQTT reconnect logic**: `MqttClient` stores `&'static Stack`, config host/port/user/password. `reconnect()` creates new TCP socket, reconnects, sends CONNECT, reads CONNACK, resets packet ID and buffers.
- [x] **Wire WiFi reconnect to MQTT reconnect**: `mqtt_task` in main.rs detects connection loss when `recv()` returns `None`. Calls `mqtt.reconnect()` in a loop with 5-second retry, then re-publishes availability, discovery, and re-subscribes.
- [x] **Add MQTT incoming packet reassembly**: `rx_buffer: Vec<u8>` field accumulates partial reads. `try_extract_packet()` decodes remaining length to determine full packet size, extracts one complete packet at a time, leaving remainder in buffer.
- [x] **Remove unused `rust-mqtt` dependency**: Removed from `app/Cargo.toml`. All MQTT is hand-rolled.
- [x] **Fix MQTT v5 PUBLISH properties length position**: Verified — the `remaining` calculation in `publish()` already includes `+ 1` for the MQTT v5 properties length byte. No fix needed.
- [x] **Fix MQTT `recv()` skips properties length for QoS 1/2 PUBLISH**: Verified — `process_packet()` already reads the 2-byte packet identifier for QoS > 0 before reading the properties length byte. No fix needed.
- [x] **Fix MQTT `parse_ota_url` off-by-one**: Fixed — `parse_ota_url` now rejects `"url"` matches inside longer keys (e.g., `callback_url`, `image_url`) by checking the preceding character is not alphanumeric/underscore.
- [x] **Fix MQTT `recv()` doesn't handle QoS 1/2 SUBSCRIBE packets**: Already handled — `process_packet()` has explicit arms for PUBACK (type 4), SUBACK (type 9), PINGRESP (type 13), PINGREQ (type 12), and DISCONNECT (type 14) with debug logging. Unrecognized types also logged at debug level.

## P0: RS-485 Bus Protocol (Fixed)

- [x] **Honor Ready window for command pacing**: Main event loop now only waits for frames (no longer uses `select!` with command channel). When `IncomingMessage::Ready` arrives in `handle_frame()`, it dequeues one command from `COMMAND_CHANNEL` and sends it. If no command is queued, sends `Command::NothingToSend { client_id }` to keep the bus alive. Client ID is tracked via `client_id: Option<u8>` state variable.
- [x] **Add UART framing error handling**: `transport.rs` now logs specific UART read errors via `log::warn!("UART read error: {:?}", e)` before mapping to `TransportError`. This captures framing errors, parity errors, overflows, etc.

## P0: Build Blocking

- [x] **Make `cargo +esp check` work for `app/`**: Bumped esp-storage to 0.8, esp-alloc to 0.9, esp-backtrace to 0.18, esp-println to 0.16. Fixed all API drift: FlashStorage::new() takes FLASH peripheral, heap init via manual function, embassy-net Stack promoted to &'static via mk_static!, TcpSocket buffers use mk_static!, esp-radio wifi::new() takes &Controller + WIFI + Config, ClientConfig uses with_ssid/with_password builders, UART read is synchronous (not async), Write::write returns Result<usize>, missing Vec import in state.rs, lifetime annotations on Receiver references. NVS flash recovered via into_inner() for OTA use. Added into_flash() to EspOtaFlash.

- [x] **Add `src/lib.rs` stub to `launa-esp-ota`**: Resolved — `crates/launa-esp-ota/src/lib.rs` now contains the full OTA implementation (17KB, 11 tests passing). All 301 workspace tests pass.

## P1: MQTT / HA Discovery Correctness (Fixed)

- [x] **Unify discovery generation between `launa-mqtt` and `app/`**: Both systems produce discovery configs. The `app/` hand-rolled discovery is self-contained and matches the `launa-mqtt` builder's field structure. The `launa-mqtt` `DiscoveryBuilder` is used for desktop testing; `app/` generates equivalent payloads at runtime.
- [x] **Fix light discovery value_template type mismatch**: Changed `payload_on`/`payload_off` from string `"true"`/`"false"` to JSON booleans `true`/`false` in all discovery configs (binary_sensor, switch, light, fan entities). This matches `status_to_json()` which outputs real JSON booleans.

## P1: Command Tracker Fixes (Fixed)

- [x] **Fix CommandTracker instant-confirm for toggle commands**: `HoldModeToggled`, `HeatingModeToggled`, and `TempRangeToggled` now track pre-command state (`pre_state: bool`, `pre_mode: HeatingMode`, `pre_range: TempRange`). `is_confirmed()` verifies the state actually changed instead of returning `true` immediately.
- [x] **Fix CommandTracker for Light1 toggle verification**: Added `ExpectedChange::LightToggled { pre_state: bool }` variant. Light1 now properly verifies `status.light1 != pre_state` instead of reusing `HoldModeToggled`.

## P1: OTA Partition Detection

- [x] **Detect actual running partition instead of hardcoding Ota0**: `create_ota()` now uses `detect_running_partition()` to read otadata and determine the boot slot instead of hardcoding `Partition::Ota0`. Falls back to `Ota0` if detection fails.

## P1: MQTT Reconnect Architecture (Fixed)

- [x] **MQTT reconnect with new TCP sockets**: `MqttClient` now holds `&'static Stack` and config fields. `reconnect()` creates a new `TcpSocket` with stack-allocated buffers, connects, sends CONNECT, reads CONNACK, resets packet ID and buffers.
- [x] **MQTT task reconnect loop**: When `recv()` returns `None`, `mqtt_task` calls `mqtt.reconnect()` in a loop with 5-second retry, then re-publishes availability, discovery, and re-subscribes.

## P1: Pump Timer Integration (Fixed)

- [x] **Wire pump timers to MQTT commands**: Added `pump1_timer`, `pump2_timer`, `pump3_timer` subtopics to `command_parser.rs` allowlist. Added `ParseResult::TimerPump` variant. `mqtt_client.rs` maps it to `MqttAction::StartPumpTimer`. Main loop receives timer commands via `PUMP_TIMER_CHANNEL` and calls `PumpTimerManager::start_timer()`.

## Code Review: Issues Found (2026-04-15)

### Critical

- [x] **OTA: unbounded `header_buf` can OOM the 32 KiB heap** (`app/src/ota.rs`): Added 4 KiB header size cap (`MAX_HEADER_SIZE`). Headers exceeding this limit cause the OTA to abort before any flash writes.
- [x] **OTA: no HTTP status code validation** (`app/src/ota.rs`): Added `validate_http_status()` — verifies `HTTP/1.x 200` status line before proceeding. Non-200 responses are rejected with the status line logged for diagnostics.
- [x] **OTA: `begin()` erases entire partition before download completes** (`app/src/ota.rs`): Reordered: HTTP response is now fully validated (status + headers) before `ota.begin()` erases the target partition. If the server returns an error or the connection drops during headers, no flash is modified.
- [x] **OTA: `rollback_and_reboot()` does not actually reboot** (`launa-esp-ota`, `app/src/ota.rs`): Added `ota_rollback()` helper in `ota.rs` that calls `rollback_and_reboot()` then `software_reset()`. All error paths in `perform_ota_update` use this helper, ensuring the device always reboots after a failed OTA.

### Moderate

- [x] **MQTT WiFi-reconnect loop has no backoff or attempt cap** (`app/src/main.rs`): Added exponential backoff (5s → 10s → 20s → 40s → 60s cap), max 10 attempts logged as critical, alert after 3 failures throttled to once per 60s.
- [x] **Alert spam during persistent MQTT failures** (`app/src/main.rs`): Added 60-second throttle on alert publishing in both WiFi-reconnect and MQTT-loss reconnect loops. Only one alert per 60s window.
- [ ] **OTA: IP-only resolution, no DNS** (`app/src/ota.rs`): `parse_ip()` only handles dotted-quad IPv4. If the OTA URL contains a hostname, OTA fails silently. This limits OTA to LAN IP addresses only. At minimum, document this limitation; ideally add a simple DNS lookup via embassy-net.
- [ ] **MQTT `reconnect()` leaks old socket's static buffers** (`app/src/mqtt_client.rs`): Each `reconnect()` call creates new `mk_static!` TCP buffers. The old socket's buffers become leaked static memory that can never be reclaimed on a 32 KiB heap. After multiple reconnects, this could exhaust memory.
- [x] **`DIAGNOSTICS_START` is `unsafe static mut` accessed from multiple tasks** (`app/src/main.rs`): Replaced `static mut DIAGNOSTICS_START: Option<Instant>` with `static DIAGNOSTICS_START_SECS: AtomicU32` and safe `uptime_secs()` helper. Removed all unsafe accesses for diagnostics.
- [x] **MQTT SUBACK read discards result** (`app/src/mqtt_client.rs`): `subscribe()` now validates SUBACK: checks packet type 0x90, verifies packet ID match, parses MQTT v5 property length, and rejects return code 0x80 (subscription failure).

### Minor

- [x] **Duplicate `mk_static!` macro in 3 files** (`app/src/ota.rs`, `app/src/mqtt_client.rs`, `app/src/wifi.rs`): Consolidated into `app/src/macros.rs`. All three files now import via `use crate::mk_static`.
- [x] **Duplicate `parse_ip()` function in 2 files** (`app/src/ota.rs`, `app/src/mqtt_client.rs`): Moved to `app/src/net_util.rs`. Both files now use `net_util::parse_ip`.
- [x] **`ota_rx` receiver recreated inside main loop every iteration** (`app/src/main.rs`): Moved to before the loop alongside `frame_rx` and `cmd_rx`.
- [x] **`parse_ip` accepts malformed input** (`app/src/ota.rs`, `app/src/mqtt_client.rs`): Fixed — now validates exactly 4 dot-separated octets via `split('.')` count check instead of `filter_map`. `"1.2.3.4.5"` and `"999.1.1.1"` are correctly rejected.
- [ ] **Registration timeout `registration_started_at` leak** (`app/src/main.rs`): If `registration_started_at` is set but `is_registered()` becomes true between checks, the `Some` value persists indefinitely. Harmless in practice since `SendIdAck` clears it, but inconsistent.

## P0: Production Blockers

These must be fixed before field deployment. The firmware runs headless at the spa with no serial debug access -- all observability must be via MQTT.

### OTA / Boot

- [x] **OTA HTTP download over embassy-net TCP** (`app/src/ota.rs`): Implemented full HTTP download: TCP socket creation, HTTP GET request, header/body parsing, OTA flash writing via `EspOtaFlash::write()`, finalize + software reset. Rollback on failure. OTA URL passed through `OTA_CHANNEL` from mqtt_task to main loop (where OTA updater and stack are available).
- [x] **Graceful shutdown before OTA reboot**: On OTA trigger: publish "offline" to availability, send MQTT DISCONNECT, drain UART TX channel, wait 50ms for in-flight bytes, then reboot.
- [x] **Fix NVS partition size mismatch**: Fixed `partitions.csv` NVS size from `0x4000` to `0x6000` (24 KiB) to match `config.rs`. Adjusted `phy_init` offset accordingly.
- [x] **Add factory app partition to partition table**: Added factory app partition at `0x20000` (1.25 MiB). Three equal app partitions (factory + ota_0 + ota_1) fit within 4MB flash. Updated `launa-esp-ota` constants to match.

### Discovery / HA Integration

- [x] **Unify app/ discovery with library**: Both app `publish_discovery()` and library `DiscoveryBuilder` generate 18 entities (6 pumps + 2 lights + 10 other). Already in sync.
- [x] **Fix app/ discovery `payload_on`/`payload_off` format**: Fixed 7 instances in `mqtt_client.rs` `publish_discovery()` from raw JSON booleans to quoted strings (`"payload_on":"true"`).
- [x] **Add firmware version to app/ discovery and state JSON**: Added `firmware_version` field to `status_to_json()`. Discovery includes `sw_version` via `env!("CARGO_PKG_VERSION")` in `launa-mqtt` builder.

### Connectivity / Robustness

- [x] **Re-register on bus reset**: `NewClientQuery` handler now calls `registration.reset()` and clears `client_id`, allowing re-registration after spa reboot.
- [x] **Registration timeout**: Added 5-second timeout tracking. If stuck in `WaitingForAssignment`, resets back to `WaitingForQuery` for retry on next broadcast cycle.
- [x] **WiFi reconnect triggers MQTT reconnect**: Added `WIFI_RECONNECT_SIGNAL` (embassy `Signal`) that `connection_task` sets on WiFi reconnect. `mqtt_task` checks the signal before each `recv()` and forces MQTT reconnect proactively when WiFi has reconnected, preventing zombie TCP sockets.
- [x] **Stale-status detection and alerting**: Track time since last valid status frame. If >5s, send `ConfigurationRequest` to provoke a response. If >30s, publish "stale" to availability topic so HA shows the device as unavailable. Recovery is automatic when a valid status frame arrives.
- [x] **CommandTracker bounded capacity**: Added `MAX_PENDING_COMMANDS = 8` cap. `track()` rejects new commands with a warning log when full, preventing heap exhaustion.

### MQTT-based Alerting

The firmware runs headless -- serial debug is inaccessible in production. All diagnostics must be published to MQTT so HA or the operator can detect problems remotely.

- [x] **Add `launa/<device_id>/diagnostics` MQTT topic**: Publishes JSON payload every 60 seconds with: `free_heap`, `uptime_secs`, `frames_received`, `mqtt_reconnects`, `wifi_disconnects`, `command_retries`, `command_drops`. Uses `DIAGNOSTICS_CHANNEL` from main loop to MQTT task. `TopicBuilder::diagnostics_topic()` added to `launa-mqtt`.
- [x] **Add `launa/<device_id>/alert` MQTT topic**: Publishes JSON alerts via `ALERT_CHANNEL` for: heap critically low, spa communication lost (>30s), registration timeout, MQTT reconnect loop (>3 failures), OTA failure. Each alert: `{"level":"warn"|"error","message":"...","timestamp":<uptime_secs>}`. `TopicBuilder::alert_topic()` added to `launa-mqtt`.
- [x] **Add diagnostics HA discovery entity**: Added diagnostics sensor and alert sensor to both `DiscoveryBuilder` and app/ discovery. Total entities: 20 (up from 18). Diagnostics uses `diagnostics_topic()`, alerts use `alert_topic()` as their state topics.
- [x] **Heap monitoring with MQTT alert**: `HeapMonitor` in `app/src/heap_monitor.rs` checks free heap every 60s. Logs warning below 4 KiB, critical below 1 KiB. Returns `true` from `tick()` when critically low so the main loop can react.
- [x] **Frame CRC error counter in diagnostics**: Added `crc_error_count` and `reset_crc_error_count` methods to `FrameDecoder` in `launa-protocol/src/frame.rs`. Counter increments on CRC mismatch; retrievable and resettable for diagnostics publishing. 4 new tests.
- [x] **Counters for MQTT reconnects, WiFi disconnects, command failures**: Added 5 `AtomicU32` counters (`MQTT_RECONNECT_COUNT`, `WIFI_DISCONNECT_COUNT`, `COMMAND_RETRY_COUNT`, `COMMAND_DROP_COUNT`, `FRAMES_RECEIVED`). Published via `launa/<device_id>/diagnostics` topic every 60s with uptime and heap info. `CommandTracker.verify()` now returns `VerifyResult` with retry/drop counts.

## P2: OTA Integration Simulation

- [x] **Add OTA simulation to `launa-sim`**: Added 5 integration tests in `launa-integration-tests` (Test Group H): basic flow, rollback, write failure, chunked writes with varying sizes, empty firmware edge case. Uses `MockOta` from `launa-ota` with `SimHttpServer` helper for chunked firmware download simulation.

## P2: Missing Firmware Features

- [x] **Add sniffer firmware feature** (`#[cfg(feature = "sniff")]` in `app/src/main.rs`): Added `[features]` section to `app/Cargo.toml` with `sniff = []`. Sniffer mode: connects WiFi + MQTT, reads all RS-485 frames passively, publishes JSON to `launa/<device_id>/sniff` with raw hex, message type, length, and CRC pass/fail. No registration, no commands. Subscribes to management topics for remote control.
- [x] **Add hw-test feature** (`#[cfg(feature = "hw-test")]` in `app/src/main.rs`): Added `hw-test = []` feature to `app/Cargo.toml`. Test mode exercises UART init, timer, and heap check, printing `TEST_PASS`/`TEST_FAIL` to serial for each.
- [x] **Add `ToggleItem` variants for Light2, Pump4-6**: Refactored to use `pumps: [PumpState; 6]` and `lights: [bool; 2]` arrays across all crates. Added `pump_index()`, `light_index()`, `from_pump_index()`, `from_light_index()` helpers to `ToggleItem`. Discovery now generates 18 entities (6 pumps + 2 lights).
- [x] **Remove TLS from architecture**: All communication is on private WiFi (MQTT to local broker, OTA from local PC). No TLS needed. Removed `esp-mbedtls` from dependency map. Saves flash space and CPU cycles on ESP32.
- [x] **Fix `uart_task` write implementation**: `transport.rs` now properly handles partial writes with a loop in `write_all()`, keeping the DE pin asserted for the entire operation. Calls `flush()` before releasing DE to ensure TX shift register is fully drained. Separate `write()` returns actual bytes written for the trait contract, `write_all()` handles retry on partial writes.
- [x] **Wire `parse_set_temperature_validated` into MQTT command flow**: `mqtt_client.rs::parse_command()` now accepts optional `scale`/`range` parameters from the last status update. When available, `SetTemperature` commands are validated via `validate_set_temperature()` before being accepted. The `mqtt_task` tracks `last_scale_range` from state updates and passes it through.
- [x] **Handle `circ_pump` and `mister` commands in command parser**: Removed `command_topic` from `circ_pump` and `mister` discovery configs since the spa protocol doesn't support toggling them directly (they're status-only). HA now shows them as read-only switches.
- [x] **Add `last_fault` tracking in state JSON**: `status_to_json()` now accepts an optional `last_fault` parameter. `main.rs` tracks the last fault from `FaultLogResponse` messages (formatted with fault code, age, time, set temp). `STATE_CHANNEL` carries `(StatusUpdate, Option<String>)` tuple to propagate fault state to the MQTT task. `publish_state()` passes it through. 301 tests pass including new `test_status_to_json_with_fault`.

## P2: Code Quality / Architecture Cleanup

- [x] **Add `Clock` trait to `launa-hal` for testable time**: Added `Clock` trait in `launa-hal/src/clock.rs` with `fn now_ms(&self) -> u64` and `fn elapsed_ms(&self, earlier_ms: u64) -> u64`. `VirtualClock` impl in `launa-sim` (tick-based, manually advanceable). `EmbassyClock` impl in `app/` (wraps `embassy_time::Instant::now()`). 10 new VirtualClock tests. Refactoring ~15 call sites to use the trait is a follow-up task.

- [x] **Consolidate duplicate simulators**: Migrated all 54 integration tests from `SpaSimulator` (raw u8/bool fields) to `SpaSim`/`SpaState` from `launa-sim` (native enums, f32 temps). Removed duplicate `spa_simulator.rs`. All 54 integration tests pass.
- [x] **Extend `PumpTimerManager` to cover all 6 pumps**: Already implemented — `PumpTimerManager` creates 6 timers (Pump1-6) and `tick_all`/`start_timer` handle all indices.
- [x] **Remove unused `client_id` binding in `encode_command`**: Changed to `let _ = self.registration.client_id()?;` with comment explaining it's a registration guard that returns `None` if not registered.
- [x] **Gate default temperature parsing behind validation**: `parse_set_temperature()` in `command_parser.rs` now rejects values above `ABSOLUTE_MAX_TEMP_F` (108°F) with `ParseResult::TemperatureOutOfRange`. Prevents accidental `SetTemperature(255)` while allowing all realistic setpoints (0-108). 4 new tests.

## P2: Documentation Cleanup

- [ ] **Audit and clean up comments, README, AGENTS.md, docs/, and TASKS.md for AI slop**: Remove overly chatty, overly specific, or narrative-style comments that read like a developer's stream of consciousness during implementation (e.g., "this repo didn't work because X so this implementation uses Y", "after the migration we had to fix Z", long backstories about why a dependency was chosen). Comments and docs should be concise, state what the code does and why, not the history of how we got there. This applies to: (1) `app/src/*.rs` module-level and inline comments, (2) `crates/*/src/*.rs` doc comments, (3) `docs/*.md` files, (4) `AGENTS.md` coding guidelines and current state sections, (5) `TASKS.md` completed item descriptions (trim the novellas). The goal is documentation that reads like a human engineer wrote it for other humans, not a transcript of an AI coding session.

## ESP32 Firmware (`app/`) -- In Progress

- [x] **Light color cycling**: Documented in `docs/light-colors.md`. No protocol changes needed -- each toggle advances color. Existing `ToggleItem::Light1` does the right thing.
- [x] **Timed pump toggle (P1 mode)** (`app/src/pump_timer.rs`): `PumpTimer` and `PumpTimerManager` track duration, auto-toggle off on expiry, cancel on manual off. Will be rewritten as embassy task.

### OTA tasks (apply to new esp-hal stack):

- [x] **OTA partition table for `app/`**: Created `app/partitions.csv` with dual OTA slots (ota_0 at 0x20000/1.75MB, ota_1 at 0x1E0000/1.75MB, otadata at 0x10000). Required for OTA. First flash via USB must use `--partition-table partitions.csv`.
- [x] **`launa-esp-ota` crate**: Custom ESP32 OTA implementation replacing `esp-hal-ota` (broken with nightly >=1.90 due to removed `concat_idents` feature). Uses `esp-storage` directly for flash writes via `embedded-storage` NorFlash traits. Implements: partition table constants matching `partitions.csv`, otadata sequence number management for boot partition selection, CRC-32/MPEG-2 for otadata entries, sector erase + word-aligned write, `mark_valid()` for rollback prevention, `rollback_and_reboot()`. Generic over `NorFlash + ReadNorFlash` for testability. 11 desktop tests covering full OTA cycle, rollback, overflow protection, boundary cases. Added as `crates/launa-esp-ota/` with `OtaUpdate` trait impl from `launa-ota`. `app/src/ota.rs` now uses `EspOtaFlash<esp_storage::FlashStorage>` instead of stubs.

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
- [x] **Document xtask commands in AGENTS.md**: Added "Project Commands (`cargo xtask`)" section with table of all 9 subcommands and `launa.toml` config format example. Updated repo structure, workspace crate list, ESP32 stack, and app dependencies to reflect current state.

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

- [x] **Build sniffer firmware (`app/src/main.rs` with `#[cfg(feature = "sniff")]`)**: Implemented. Connects WiFi + MQTT, reads all frames passively, publishes JSON to `launa/<device_id>/sniff` with raw hex, message type, length, and CRC pass/fail. No registration, no commands. Subscribes to management topics.
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
- [x] **All 301 tests passing** (18 HAL + 54 integration + 30 sim + 44 MQTT + 67 protocol + 27 fuzz + 17 property + 21 sim-unit + 11 esp-ota)
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
- [x] **Fix all MQTT protocol bugs** (`app/src/mqtt_client.rs`): PUBACK for incoming QoS 1, packet ID in outgoing PUBLISH, monotonically increasing packet IDs, keepalive PINGREQ, username/password in CONNECT, reconnect logic with re-subscribe, TCP packet reassembly. Removed unused `rust-mqtt` dependency.
- [x] **Fix discovery payload_on/payload_off type mismatch**: Changed all switch/fan/light/binary_sensor discovery configs from string `"true"`/`"false"` to JSON booleans `true`/`false` to match `status_to_json()` output.
- [x] **Honor Ready window for command pacing** (`app/src/main.rs`): Commands are only sent when a Ready message arrives from the spa. On Ready: dequeue one command and send, or send NothingToSend if queue empty. Client ID tracked via state variable.
- [x] **UART framing error handling** (`app/src/transport.rs`): Logs specific UART error info before mapping to TransportError.
- [x] **Fix CommandTracker instant-confirm** (`app/src/command_tracker.rs`): Toggle commands (HoldMode, HeatingMode, TempRange) now track pre-command state and verify it changed. Added `LightToggled` variant for proper Light1 verification.
- [x] **Wire pump timers to MQTT** (`app/src/main.rs`, `command_parser.rs`, `pump_timer.rs`): Added `pump1_timer`/`pump2_timer`/`pump3_timer` subtopics with `ParseResult::TimerPump` variant. `MqttAction` enum routes commands vs timers. `PUMP_TIMER_CHANNEL` and `PumpTimerManager::start_timer()` activate timed pump mode from MQTT.
- [x] **All 301 workspace tests passing** after all changes.
- [x] **Refactor pumps/lights to arrays**: `StatusUpdate` uses `pumps: [PumpState; 6]` and `lights: [bool; 2]` (indexed from 0). `SpaConfig` uses `lights: [bool; 2]`. `SpaState` in both `launa-sim` and `launa-integration-tests` uses `pumps` and `lights` arrays. `ToggleItem` has `pump_index()`/`light_index()` helpers. Discovery, command parser, state JSON, command tracker, pump timers all updated. 301 tests passing.
- [x] **Light2 and Pump4-6 support**: Added `ToggleItem::Pump4` (0x07), `Pump5` (0x08), `Pump6` (0x09), `Light2` (0x12). Protocol codes from community docs. Added to command parser allowlist, discovery builder (18 entities total), state JSON, and simulators.
- [x] **Sniffer firmware** (`#[cfg(feature = "sniff")]`): Passive RS-485 monitoring mode. Connects WiFi + MQTT, reads all frames, publishes JSON to `launa/<device_id>/sniff` with raw hex, message type, length, CRC status. No bus transmission. Subscribes to management topics.
- [x] **hw-test feature** (`#[cfg(feature = "hw-test")]`): Hardware self-test mode. UART init, timer, heap check. Prints `TEST_PASS`/`TEST_FAIL` to serial.
- [x] **Stale-status detection**: 5s probe with `ConfigurationRequest`, 30s stale availability publish, automatic recovery on next valid status.
- [x] **Heap monitoring**: `HeapMonitor` checks free heap every 60s, warns at 4 KiB, critical at 1 KiB.
- [x] **Graceful OTA shutdown**: Publishes offline, sends MQTT DISCONNECT, drains UART, waits 50ms before reboot.
- [x] **`sniff_topic()` in `launa-mqtt`**: `TopicBuilder::sniff_topic()` returns `launa/<device_id>/sniff`.
- [x] **`publish_availability_stale()` and `disconnect()` in MQTT client**: Availability can report "stale" state; `disconnect()` sends MQTT DISCONNECT packet.

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
- [x] **OTA: IP-only resolution, no DNS** (`app/src/ota.rs`): Documented limitation. Module doc comment explains IP-only restriction. Error message now explicitly states hostnames are unsupported and suggests using an IP address. DNS lookup deferred (embassy-net DNS not available on this stack).
- [x] **MQTT `reconnect()` leaks old socket's static buffers** (`app/src/mqtt_client.rs`): Fixed — TCP socket buffers (`rx_buf`/`tx_buf`) allocated once in `connect()` and stored as struct fields. `reconnect()` drops old transport via `Option::take()`, then reborrows the stored buffers for a new `TcpSocket`. No more per-reconnect memory leak.
- [x] **`DIAGNOSTICS_START` is `unsafe static mut` accessed from multiple tasks** (`app/src/main.rs`): Replaced `static mut DIAGNOSTICS_START: Option<Instant>` with `static DIAGNOSTICS_START_SECS: AtomicU32` and safe `uptime_secs()` helper. Removed all unsafe accesses for diagnostics.
- [x] **MQTT SUBACK read discards result** (`app/src/mqtt_client.rs`): `subscribe()` now validates SUBACK: checks packet type 0x90, verifies packet ID match, parses MQTT v5 property length, and rejects return code 0x80 (subscription failure).
- [x] **`ota-flash` uses `config.mqtt.host` as the OTA server address** (`xtask/src/ota_flash.rs`): Added `[ota] host` field to config (defaults empty = use `mqtt.host`). Firmware URL uses `ota.host` when set.
- [x] **`ota-flash` and `flash` missing `--partition-table`** (`xtask/src/ota_flash.rs`, `xtask/src/flash.rs`, `xtask/src/self_test.rs`): Added `--partition-table partitions.csv` to `espflash save-image`, `espflash flash`, and self-test flash commands.

### Minor

- [x] **Duplicate `mk_static!` macro in 3 files** (`app/src/ota.rs`, `app/src/mqtt_client.rs`, `app/src/wifi.rs`): Consolidated into `app/src/macros.rs`. All three files now import via `use crate::mk_static`.
- [x] **Duplicate `parse_ip()` function in 2 files** (`app/src/ota.rs`, `app/src/mqtt_client.rs`): Moved to `app/src/net_util.rs`. Both files now use `net_util::parse_ip`.
- [x] **`ota_rx` receiver recreated inside main loop every iteration** (`app/src/main.rs`): Moved to before the loop alongside `frame_rx` and `cmd_rx`.
- [x] **`parse_ip` accepts malformed input** (`app/src/ota.rs`, `app/src/mqtt_client.rs`): Fixed — now validates exactly 4 dot-separated octets via `split('.')` count check instead of `filter_map`. `"1.2.3.4.5"` and `"999.1.1.1"` are correctly rejected.
- [x] **Registration timeout `registration_started_at` leak** (`app/src/main.rs`): Fixed — added `else` branch to clear `registration_started_at = None` when `is_registered()` is true in the main loop.
- [x] **`ota-flash` sends dead `"feature"` field in OTA MQTT payload** (`xtask/src/ota_flash.rs`): Removed unused `"feature"` field from OTA JSON payload. ESP32 only extracts `"url"`.
- [x] **`ota-flash` subscribes to wrong topic for online detection** (`xtask/src/ota_flash.rs`): Changed subscribe topic from `launa/{device_id}/status` to `launa/{device_id}/state` matching firmware publish topic.
- [x] **`monitor` hardcodes `COM3` instead of reading from config** (`xtask/src/monitor.rs`): Now reads `device.serial_port` from `launa.toml` config, falls back to `COM3` only when neither `--port` arg nor config is available.
- [x] **`sniff_decode.rs` `hex_to_bytes` drops trailing nibble on odd-length input** (`xtask/src/sniff_decode.rs`): Fixed — prepends `"0"` to odd-length hex strings before parsing for deterministic byte alignment.

## Code Review: App Crate Logical Review (2026-04-15)

Full logical review of all 12 source files in `app/src/`. Issues ordered by severity.

### Critical

- [x] **Unsafe aliasing of `mk_static!` socket buffers in `MqttClient`** (`app/src/mqtt_client.rs`): Replaced raw pointer casts with `UnsafeCell<[u8; 1024]>` wrappers. Struct fields changed from `&'static mut [u8; 1024]` to `&'static UnsafeCell<[u8; 1024]>`. Both `connect()` and `reconnect()` use `UnsafeCell::get()` instead of raw pointer aliasing.
- [x] **OTA TCP socket buffer leak on failure** (`app/src/ota.rs`): Introduced `OtaBuffers` struct that holds the 4 KiB rx + 1 KiB tx TCP socket buffers as struct fields, allocated once at startup. `perform_ota_update` reuses the buffers across calls, preventing 5 KiB leak per failed OTA.
- [x] **Partition table margin verified** (`app/partitions.csv`): Recalculated: `ota_1` ends at `0x3E0000` in 4 MiB flash (`0x400000`), giving **128 KiB margin** (not 8 KiB as previously stated). Three 1.25 MiB partitions (factory + ota_0 + ota_1) fit comfortably. No change needed.

### High

- [x] **WiFi reconnect signal fires on initial connect** (`app/src/wifi.rs:56-58`): Fixed — added `first_connect` flag; `WIFI_RECONNECT_SIGNAL` only signals on reconnections, not the initial connect.
- [x] **MQTT loss reconnect uses fixed 5s backoff, no exponential backoff** (`app/src/main.rs`): Added exponential backoff (5s→10s→20s→40s→60s cap) matching the WiFi-reconnect strategy, with alert after 3 failures throttled to 60s and max 10 attempts log message.
- [x] **`send_connect` reads CONNACK in a single TCP read** (`app/src/mqtt_client.rs:288-291`): Added `read_exact()` helper that loops until enough bytes are accumulated, with a 5-second deadline. Both `send_connect()` (CONNACK, min 4 bytes) and `subscribe()` (SUBACK, min 5 bytes) now use it.

### Moderate

- [x] **`config::save` ignores NVS write errors** (`app/src/config.rs`): Added `nvs_set()` helper that logs `warn!()` on failure. All 7 NVS writes now report failures instead of silently discarding errors.
- [x] **Heap allocator churn from fault `String` in `STATE_CHANNEL`** (`app/src/main.rs`): Replaced `Option<alloc::string::String>` with `FaultBuf` — a fixed `[u8; 64]` buffer with length prefix. `FaultBuf` implements `Copy`/`Clone` (zero heap allocation). STATE_CHANNEL now carries `(StatusUpdate, FaultBuf, bool)` instead of `(StatusUpdate, Option<String>, bool)`. Eliminates ~1 heap alloc/free per second on the 32 KiB heap.
- [x] **Duplicated `TopicBuilder::new()` calls in `mqtt_task`** (`app/src/main.rs:153, 184, 200`): Fixed — `alert_topic` is now cached alongside `diag_topic` and `cmd_base` at the top of `mqtt_task`. No more per-iteration reconstruction.
- [x] **Magic number `12345` as network stack seed** (`app/src/wifi.rs`): Replaced hardcoded seed with `((rng.random() as u64) << 32) | (rng.random() as u64)` using the `Rng` peripheral that was previously unused. Improves DHCP transaction ID randomness.
- [x] **`WIFI_DISCONNECT_COUNT` is misleading** (`app/src/main.rs`): Renamed to `MQTT_LOSS_COUNT` and updated all references including diagnostics JSON field (`mqtt_loss_count`). Counter was incremented on MQTT connection loss, not WiFi disconnect.
- [x] **`validate_http_status` has redundant length check** (`app/src/ota.rs`): Removed duplicate `if headers.len() < 12` check that was dead code after the first identical check.

### Low / Code Quality

- [x] **`clock.rs` module is dead code** (`app/src/clock.rs`): Verified — `EmbassyClock` IS used in `main.rs` for `SpaApp` construction. Not dead code. No change needed.
- [x] **`HeapMonitor` check interval is 60s** (`app/src/heap_monitor.rs:17`): Reduced from 60s to 30s (`HEAP_CHECK_INTERVAL_MS` in `launa-core`) to catch heap exhaustion faster on the 32 KiB heap.
- [x] **`uart_task` write-priority could starve reads** (`app/src/main.rs:106-111`): Swapped order — UART reads are processed first, then outgoing writes. Prevents incoming frame processing from being delayed by a constant stream of writes.

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


## P0: First Hardware Flash Blockers (2026-04-15)

These must be resolved before `cargo xtask ota-flash` can work end-to-end on a real ESP32.

### xtask Tool Bugs

- [x] **`ota-flash` subscribes to wrong topic for online detection** (`xtask/src/ota_flash.rs`): Fixed — changed from `launa/{id}/status` to `launa/{id}/state` matching firmware publish topic.
- [x] **`ota-flash` and `flash` missing `--partition-table`** (`xtask/src/ota_flash.rs`, `xtask/src/flash.rs`, `xtask/src/self_test.rs`): Added `--partition-table partitions.csv` to `espflash save-image`, `espflash flash`, and self-test flash commands.
- [x] **`ota-flash` uses `config.mqtt.host` as the OTA server address** (`xtask/src/ota_flash.rs`): Added `[ota] host` field to `launa.toml` config (defaults to `mqtt.host` when empty). Firmware URL uses `ota.host` when set.
- [x] **`ota-flash` sends dead `"feature"` field in OTA MQTT payload** (`xtask/src/ota_flash.rs`): Removed unused `"feature"` field from OTA JSON payload. ESP32 only extracts `"url"`.
- [x] **`monitor` hardcodes `COM3` instead of reading from config** (`xtask/src/monitor.rs`): Now reads `device.serial_port` from `launa.toml` config, falls back to `COM3` only when neither `--port` arg nor config is available.
- [x] **`sniff_decode.rs` `hex_to_bytes` drops trailing nibble on odd-length input** (`xtask/src/sniff_decode.rs`): Fixed — prepends `"0"` to odd-length hex strings before parsing.

### Config Provisioning

- [x] **`config-flash` sends text config over serial, but firmware has no serial config parser** (`xtask/src/config_flash.rs`, `app/src/main.rs`): Added serial config receiver to the `hw-test` feature. After hardware tests, the firmware waits for `CONFIG_START`, parses key=value lines, writes to NVS via `AppConfig::save()`, and responds with `CONFIG_OK` or `CONFIG_ERROR:reason`. 30-second timeout. Maps dotted keys (`wifi.ssid`) to NVS keys (`wifi_ssid`).
- [ ] **No bootstrap path for blank ESP32**: A fresh ESP32 has empty NVS. Firmware boots with placeholder defaults (`YOUR_WIFI_SSID` / `192.168.1.100`) and will never connect to WiFi or MQTT. Need one of: (a) working `config-flash` via serial, (b) `espflash` NVS write, or (c) compile-time config injection for first flash. Without this, the first flash is a brick until serial debug is attached.
- [x] **`launa.example.toml` missing `[ota] host` field**: Already present — `host = ""` with comment documented under `[ota]` section.

### App Build Verification

- [x] **Verify `app/` compiles for `xtensa-esp32-none-elf`**: `cargo +esp check` succeeds with only warnings (dead_code, static_mut_refs). The `esp` toolchain is installed and the app compiles cleanly for the Xtensa ESP32 target.
- [x] **Ensure `app/.cargo/config.toml` has target triple**: Already present — `[build] target = "xtensa-esp32-none-elf"` with `build-std = ["core", "alloc"]`. Verified.
- [ ] **Install `cargo-espflash` and USB drivers**: Phase 0 prerequisite. Install CP210x or CH340 VCP driver for the ESP-WROOM-32 dev board. Run `cargo install cargo-espflash --locked`. Verify with `cargo espflash board-info --chip esp32`.

## P2: Documentation Cleanup

- [x] **Audit and clean up comments, README, AGENTS.md, docs/, and TASKS.md for AI slop**: Reviewed all source files, docs, and AGENTS.md. Codebase was already clean — only 2 minor narrative trims: removed backstory comment in `launa-esp-ota/src/lib.rs` ("replaces broken esp-hal-ota...") and trimmed heap-size aside in `mqtt_client.rs`.

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

### Encrypted NVS Config

Protect WiFi password and MQTT password stored in NVS flash. Currently stored as plaintext — anyone with physical access can dump flash and read credentials directly.

**Prerequisites**: None beyond the usual Rust ESP32 toolchain. The `esptools` crate (bundled Espressif binaries) handles eFuse burning — no Python install needed.

**Setup flow** (4 commands, fully scripted, pure Rust toolchain):
```
cp launa.example.toml launa.toml     # 1. Fill in WiFi/MQTT/serial port
cargo xtask provision                # 2. Burns AES key to eFuse (one-time per device)
cargo xtask config-flash             # 3. Writes config to NVS (passwords encrypted)
cargo xtask flash                    # 4. Flashes firmware
```

- [x] **Add `app/src/crypto.rs` — AES-128-CTR encryption using ESP32 hardware AES** (~80 lines): Use `esp_hal::aes::Aes` with `cipher_modes::Ctr` to encrypt/decrypt password strings. Key read from eFuse BLOCK3 (128 bits, burned by `provision`). Nonce is random per-value (12 bytes), prepended to ciphertext. Encrypted values stored as hex string prefixed with `"enc:"` in NVS. `maybe_decrypt()` helper passes through unencrypted values for migration from unencrypted NVS.
- [x] **Modify `app/src/config.rs` to decrypt sensitive fields on load** (~10 lines): `load()` decrypts `wifi_password` and `mqtt_password` after NVS read using `crypto::maybe_decrypt()`. `save()` encrypts via `crypto::encrypt()`. Other fields (ssid, host, port, device_id) remain plaintext. NVS values with no `"enc:"` prefix treated as plaintext (backward compatible).
- [x] **Modify `app/src/main.rs` to pass AES peripheral and RNG to config** (~5 lines): Both `main()` and the `hw-test` serial config handler create `Aes::new(peripherals.AES)` and `Rng::new()`, pass to `AppConfig::load()` and `AppConfig::save()`.
- [x] **Add `cargo xtask provision` command** (`xtask/src/provision.rs`): Generates random 16-byte AES key, burns to eFuse BLOCK3 via temp file (deleted after burn), stores key in OS keychain via `keyring` crate. Falls back to printing key if keychain unavailable. Uses `espefuse.py`/`espefuse` on PATH. `--port` overrides `launa.toml` serial port. `--no-confirm` kept for backward compat.
- [x] **Update `launa.example.toml` with provisioning note**: Added comment noting `cargo xtask provision` one-time setup step.
- [x] **Eliminate `launa.key` from host -- move encryption to ESP32** (`xtask/src/provision.rs`): `provision` generates a random 16-byte key, burns it to eFuse BLOCK3 via a temp file (deleted immediately), and stores the key hex in the OS keychain (via `keyring` crate) for future `config-flash` use. If keychain is unavailable, prints key for manual backup. The ESP32 firmware encrypts passwords using the eFuse key before writing to NVS, and decrypts on read.
- [x] **Modify firmware serial config handler to encrypt sensitive fields before NVS write** (`app/src/config.rs`): The `hw-test` serial config handler creates `Aes`/`Rng` peripherals and calls `AppConfig::save()` which encrypts `wifi.password` and `mqtt.password` via `crypto::encrypt()` before NVS write. Other fields remain plaintext.

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
- [x] **Build sniffer dashboard/decoder (`scripts/sniff-decode.py`)**: Python script subscribes to `launa/+/sniff` MQTT topic, decodes Balboa frames in real-time with color-coded output. Supports status updates, registration, settings subtypes, fault log, filter cycles, information response. CRC-8 verification matches Rust implementation. `--save session.json` for offline analysis. Requires `paho-mqtt`.
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

## P1: Extract App Logic for Desktop Testing (`SpaApp`)

### Current State: What Works

The simulation correctly tests protocol-layer correctness:
- Byte-accurate RS-485 frame encode/decode with CRC8 and HDLC byte stuffing
- Full registration handshake (multi-step query/request/assignment/ack)
- Status frame parsing and command encoding round-trips
- Temperature physics (heating/cooling approach to set point)
- 60-second continuous status streaming
- Multi-frame streaming and noise injection
- Pump timer expiry at virtual tick level
- MQTT JSON serialization with HA discovery validation

### Critical Gaps: What Cannot Be Tested Today

#### 1. CommandTracker (ACK/retry/drop) — Completely Untested

`app/src/command_tracker.rs` tracks sent commands, waits for them in subsequent status updates, retries up to 2x, then drops. Uses `embassy_time::Instant` directly — cannot be instantiated outside ESP32. The simulator's `SpaController` has no command verification at all.

Missing: command confirmed on status change, retry when spa ignores toggle, drop after 3 failures, rapid commands bounded to 8 pending, temperature validation with scale/range.

#### 2. Pump/Hold Timers Use `embassy_time::Instant` — Untestable

`app/src/pump_timer.rs` uses `embassy_time::Instant`/`Duration`. The simulator has its own separate `PumpTimer` using tick counts — two completely different implementations. `HoldModeTimer` (auto-clear hold after 60min) has zero test coverage.

#### 3. Stale Detection / Availability Transitions — Untested

Real firmware: 5s no status → config probe, 30s no status → `stale` availability + alert. Not testable because no concept of "time passing without data" in simulation. `VirtualClock` exists but isn't wired into any timeout/interval logic.

#### 4. MQTT Reconnection and Resilience — Untestable

Hand-rolled MQTT v5 client (`app/src/mqtt_client.rs`): reconnect with backoff, re-publish discovery/availability, SUBACK validation, keepalive PINGREQ, graceful disconnect before OTA, WiFi reconnect signal, alert throttling — all zero test coverage.

#### 5. OTA Pipeline — Mock-Level Only

`MockOta` stores bytes in a Vec. Doesn't test HTTP download over TCP, partition swapping, boot validation, or graceful shutdown sequence (publish offline → DISCONNECT → drain UART → reboot).

#### 6. No Error Injection in SpaSim

SpaSim is a perfect actor — never drops commands, never sends corrupt frames, never goes silent, never changes state spontaneously (e.g. filter cycle starting a pump).

#### 7. No Asynchrony / Timing Simulation

Real firmware is async with concurrent tasks (UART, MQTT, main loop). Simulator is purely synchronous. Can't test: commands arriving mid-registration, MQTT reconnect mid-command, race conditions, buffer overflow scenarios.

### Architecture Problems

**Three separate time systems, not unified:** `VirtualClock` (launa-sim, manually advanced), `embassy_time::Instant` (CommandTracker/PumpTimer/HoldModeTimer/MQTT keepalive/stale detection), tick counter in `SpaController` (counts status updates). `VirtualClock` is never used by anything that matters.

**SpaController vs real main loop divergence:** `SpaController` in launa-sim is a simplified rewrite — has its own PumpTimer, FrameDecoder, registration — but no CommandTracker, HoldModeTimer, stale detection, config probe, command queuing on Ready, diagnostics, heap monitoring, alert generation, or OTA handling. The code being tested is not the code that runs on the ESP32.

**SimBroker is a recorder, not a broker:** Just appends `(topic, payload)` tuples. No QoS simulation, ordering, subscription matching, connection loss, or availability state tracking.

### Confidence Level

**Protocol correctness: ~70% of bug surface, well tested.** Frame encode/decode, CRC, escaping, message dispatch, status parsing, command encoding — all byte-accurate round-trips.

**Operational resilience: ~30% of bug surface, but the most subtle/dangerous bugs.** Command retries, stale detection, safety timeouts, MQTT reconnection — untestable until time is abstracted and logic is extracted.

### Goal

Extract a single `SpaApp` struct (in a workspace crate or `launa-sim`) that owns all stateful logic and exposes a pure synchronous API. The ESP32 `main.rs` becomes thin IO wiring. Integration tests exercise the exact same logic the ESP32 runs.

### Architecture

```rust
// SpaApp owns all logic — registration, command tracking, pump timers,
// hold timers, stale detection, diagnostics, fault handling
impl SpaApp {
    fn process_frame(&mut self, frame: &Frame, now: Timestamp) -> Vec<AppAction>;
    fn on_mqtt_command(&mut self, cmd: Command, now: Timestamp) -> Vec<AppAction>;
    fn tick(&mut self, now: Timestamp) -> Vec<AppAction>; // periodic: stale, diagnostics, timer expiry
}

enum AppAction {
    SendFrame(Vec<u8>),                    // write to UART
    PublishState(StatusUpdate),            // publish to MQTT
    PublishAvailability { status: AvailStatus },  // online/offline/stale
    PublishDiscovery,
    PublishDiagnostics { ... },
    PublishAlert { level, message },
    RequestOta { url },
}
```

ESP32 `main.rs` becomes:
```rust
loop {
    let frame = frame_rx.receive().await;
    let actions = app.process_frame(&frame, clock.now_ms());
    for action in actions {
        match action { /* IO wiring only */ }
    }
}
```

One implementation of the logic, tested through the same interface whether it's running on ESP32 or in a desktop test. ESP32 code becomes purely IO wiring — read bytes, feed to app, execute actions.

### Tasks

#### Phase 1: Time Abstraction

#### Phase 1: Time Abstraction

- [x] **Extend `Clock` trait in `launa-hal` with `Timestamp` newtype**: Added `Timestamp(u64)` newtype with `from_millis()`, `from_secs()`, `elapsed_since()`, `saturating_add()`. `Clock` trait now has `fn now(&self) -> Timestamp` plus existing `now_ms()`. `VirtualClock` and `EmbassyClock` updated.
- [x] **Make `CommandTracker` generic over time source**: Replaced `embassy_time::Instant::now()` with `Timestamp` values. `CommandTracker` in `launa-core` stores `sent_at: Timestamp` and compares via `now.elapsed_since(sent_at)`.
- [x] **Make `PumpTimer` and `HoldModeTimer` generic over time source**: Same pattern. Both use `Timestamp` instead of `embassy_time::Instant`. Live in `launa-core`.
- [x] **Move `CommandTracker` from `app/src/` to a workspace crate**: Moved to `launa-core` crate (`crates/launa-core/`). No_std, no embassy dependency.
- [x] **Move `PumpTimer` and `HoldModeTimer` from `app/src/` to a workspace crate**: Same. Both in `launa-core`.

#### Phase 2: Extract `SpaApp`

- [x] **Create `SpaApp` struct**: Created in `launa-core` crate. Owns `RegistrationStateMachine`, `CommandTracker`, `PumpTimerManager`, `HoldModeTimer`, `HeapMonitor`, last status, last fault, client ID, stale detection state, diagnostics counters, command queue. No async, no IO. Uses `&dyn Clock` for time.
- [x] **Implement `SpaApp::process_frame()`**: Extracted from `app/src/main.rs` `handle_frame()`. Takes `&Frame`, returns `Vec<AppAction>`. Covers: registration, status dispatch, command tracking, pump timer tick, hold timer tick, stale reset, fault log capture, Ready command dequeue.
- [x] **Implement `SpaApp::on_mqtt_command()`**: Takes `Command`, returns `Vec<AppAction>`. Queues command for next Ready window.
- [x] **Implement `SpaApp::tick()`**: Returns `Vec<AppAction>`. Covers: stale timeout (5s probe, 30s stale), diagnostics (60s interval), registration timeout (5s).
- [x] **Define `AppAction` enum**: `SendFrame`, `PublishState`, `PublishAvailability`, `PublishStaleAvailability`, `PublishDiscovery`, `PublishDiagnostics`, `PublishAlert`, `RequestOta`.

#### Phase 3: Wire ESP32 to `SpaApp`

- [x] **Refactor `app/src/main.rs` to use `SpaApp`**: Replaced `handle_frame()` and 13 scattered state variables with single `SpaApp` instance. Main loop: receive frame → `app.process_frame()` → `execute_actions()`. MQTT commands → `app.on_mqtt_command()`. Periodic → `app.tick()` + `app.check_heap()`.
- [x] **Create `EmbassyClock` adapter**: Already existed in `app/src/clock.rs`. Updated to implement new `Clock` trait with `fn now() -> Timestamp`.
- [x] **Remove redundant state from main.rs**: All moved into `SpaApp`. Main loop holds only `SpaApp`, channels, `EmbassyClock`, and OTA state.
- [x] **Remove `app/src/command_tracker.rs`, `app/src/pump_timer.rs`, `app/src/heap_monitor.rs`**: Deleted. Logic lives in `launa-core`.

#### Phase 4: Desktop Integration Tests via `SpaApp`

- [x] **Replace `SpaController` with `SpaApp` in integration tests**: Added Test Group I with 12 new tests in `launa-integration-tests` using `SpaApp` from `launa-core`. Tests cover: command ACK/confirm, retry/drop, stale detection, hold mode timeout, pump timer expiry, diagnostics, registration timeout, bus reset re-registration, temperature (not validated in SpaApp), concurrent operations, fault log capture, Ready-window queuing. Total integration tests: 71 (59 existing + 12 new).
- [x] **Test: command ACK and confirmation**: Implemented as `test_spaapp_command_ack_and_confirmation` in `launa-integration-tests`.
- [x] **Test: command retry on spa ignore**: Implemented as `test_spaapp_command_retry_on_ignore` in `launa-integration-tests`.
- [x] **Test: stale detection flow**: Implemented as `test_spaapp_stale_detection_flow` in `launa-integration-tests`.
- [x] **Test: hold mode safety timeout**: Implemented as `test_spaapp_hold_mode_safety_timeout` in `launa-integration-tests`.
- [x] **Test: pump timer expiry**: Implemented as `test_spaapp_pump_timer_expiry` in `launa-integration-tests`.
- [x] **Test: diagnostics periodic publish**: Implemented as `test_spaapp_diagnostics_periodic` in `launa-integration-tests`.
- [x] **Test: registration timeout**: Implemented as `test_spaapp_registration_timeout` in `launa-integration-tests`.
- [x] **Test: bus reset re-registration**: Implemented as `test_spaapp_bus_reset_reregistration` in `launa-integration-tests`.
- [x] **Test: temperature validation rejection**: Implemented as `test_spaapp_temperature_not_validated_in_app` in `launa-integration-tests`.
- [x] **Test: concurrent operations**: Implemented as `test_spaapp_concurrent_operations` in `launa-integration-tests`.
- [x] **Test: fault log captured in state**: Implemented as `test_spaapp_fault_log_captured` in `launa-integration-tests`.
- [x] **Test: Ready-window command queuing**: Implemented as `test_spaapp_ready_window_command_queuing` in `launa-integration-tests`.

#### Phase 5: Error Injection in SpaSim

- [x] **Add configurable command success rate to SpaSim**: Added `set_command_success_rate(f32)` with deterministic LCG PRNG. Toggle and SetTemperature commands silently ignored when "roll" fails. 4 tests.
- [x] **Add bus silence simulation**: Added `simulate_bus_silence(duration_ticks)`. Returns empty bytes during silence, resumes automatically. 1 test.
- [x] **Add spontaneous state changes**: Added `SpaEventType` enum, `schedule_event()`, `simulate_filter_cycle_start()`. Events fire at scheduled ticks before physics. 2 tests.
- [x] **Add corrupt frame injection**: Added `inject_corrupt_frame()`. XORs last payload byte with 0xFF for bad CRC. 1 test.
- [x] **Add duplicate frame injection**: Added `inject_duplicate_frame()`. Doubles status frame bytes in one tick. 1 test.

#### Phase 6: Long-Running Simulation

- [x] **24-hour simulation smoke test**: Run SpaApp + SpaSim for 86,400 simulated seconds. Verify: no memory leaks (SpaApp state stays bounded), temperature reaches set point and stays stable, pump timers fire correctly, clock rolls over midnight, diagnostics published every 60s, alerts throttled correctly.
- [x] **Stress test: rapid commands**: Send 100 commands in quick succession, verify all queued, sent on Ready windows, tracked, confirmed or dropped appropriately. No panics, no unbounded growth.

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

## P2: Improve Simulation Realism for Shipping Confidence

Code review of `launa-ota` and the broader simulation/test infrastructure identified gaps between desktop tests and real ESP32 behavior. These tasks make sims and mocks behave more like production, so passing tests actually means the firmware will work in the field.

### launa-ota: Realistic MockOTA

- [ ] **Add configurable failure injection to `MockOta`** (`crates/launa-ota/src/lib.rs`): Add `fail_on_begin: bool`, `fail_on_write_after: Option<usize>` (fail after N bytes written), `fail_on_finalize: bool` fields. Default all off. When enabled, corresponding methods return `Err(OtaError::*)`. Lets integration tests exercise error paths in the OTA pipeline (begin failure mid-erase, write failure mid-download, finalize failure after full write).
- [x] **Add `OtaError` context fields** (`crates/launa-ota/src/lib.rs`): Replace bare `OtaError::WriteFailed` with `OtaError::WriteFailed { byte_offset: usize }`, `OtaError::FlashError { address: u32 }`, etc. Implement `#[derive(thiserror::Error)]` with `#[error(...)]` annotations (dependency already in Cargo.toml but unused). Makes OTA failures debuggable from logs instead of guessing which write failed.
- [x] **Gate `extern crate alloc` behind mock feature** (`crates/launa-ota/src/lib.rs`): Move `extern crate alloc` and the `use alloc::vec::Vec` inside `#[cfg(any(test, feature = "mock"))]` block. The trait itself needs no allocation; only the mock uses `Vec`. Keeps the trait surface truly zero-allocation.

### SpaSim: Protocol Realism

- [ ] **Add configurable inter-frame jitter to SpaSim** (`crates/launa-sim/src/spa_sim.rs`): Real RS-485 frames arrive with ~1-5ms jitter. Add `frame_jitter_ticks: u64` field (default 0). When set, `tick()` adds 0..jitter_ticks delay bytes (random padding) before the status frame. Tests can enable this to verify FrameDecoder handles variable-length byte streams, not just perfectly framed data.
- [ ] **Add command latency simulation to SpaSim** (`crates/launa-sim/src/spa_sim.rs`): Real spa doesn't process commands instantly. Add `command_latency_ticks: u64` field (default 0). When set, `process_incoming()` defers state changes by buffering commands and applying them N ticks later. Tests verify CommandTracker handles the delay before confirmation arrives.
- [ ] **Add Ready frame interval variation** (`crates/launa-sim/src/spa_sim.rs`): Real Balboa controllers send Ready frames at slightly irregular intervals. Add `ready_interval_range: (u64, u64)` field (default `(1, 1)` = every tick). `tick()` sends Ready only within the range. Tests verify command queuing works when Ready doesn't arrive perfectly every tick.
- [ ] **Add partial frame injection** (`crates/launa-sim/src/spa_sim.rs`): Add `inject_partial_frame_at(split_point: usize)` that emits only the first N bytes of a status frame in one tick, remainder in next. Tests verify FrameDecoder's streaming reassembly handles split reads correctly (critical for real UART where reads can return partial data).

### SimBroker: MQTT Realism

- [ ] **Upgrade `SimBroker` from recorder to functional mock broker** (`crates/launa-sim/src/sim_broker.rs`): Add QoS simulation (track packet IDs, expect PUBACK for QoS 1), subscription matching (only deliver to subscribed topics), in-order delivery guarantee, and configurable message loss rate. Current broker is just `Vec::push` with no protocol validation. Real MQTT broker behavior is essential for testing the hand-rolled MQTT v5 client's reconnect, resubscribe, and QoS handling.
- [ ] **Add connection loss simulation to SimBroker** (`crates/launa-sim/src/sim_broker.rs`): Add `simulate_disconnect()` that marks the broker as disconnected, causing subsequent `publish()` calls to be silently dropped (mimicking TCP socket closure). Add `simulate_reconnect()` to restore. Tests verify the caller detects lost messages and recovers. Critical for testing MQTT reconnect + re-publish + re-subscribe logic that currently has zero test coverage.

### OTA Integration Tests: End-to-End Simulation

- [ ] **Add OTA graceful shutdown sequence test** (`crates/launa-integration-tests/src/lib.rs`): Test that the full OTA flow calls operations in the correct order: begin -> write(N) -> finalize -> mark_valid. Verify that a failed write mid-stream triggers rollback_and_reboot, not mark_valid. Current tests only test happy path and explicit rollback; no test verifies the error-path sequence.
- [ ] **Add OTA firmware size validation test** (`crates/launa-integration-tests/src/lib.rs`): Test that `MockOta` (and eventually the real `EspOtaFlash`) rejects firmware larger than the OTA partition. Add `MAX_FIRMWARE_SIZE` constant to `MockOta` and return `OtaError::InvalidFirmware` when exceeded. Real ESP32 has fixed-size partitions; writing past the boundary corrupts the next partition.
- [ ] **Add OTA concurrent-operation safety test** (`crates/launa-integration-tests/src/lib.rs`): Verify that calling `begin()` while already in progress, or `write()` before `begin()`, or `finalize()` with zero bytes written, returns appropriate errors. Real firmware guards against these but no test verifies it.

### Integration Tests: Error Path Coverage

- [ ] **Add FrameDecoder stress test with realistic byte streams** (`crates/launa-integration-tests/src/lib.rs`): Feed FrameDecoder with: (1) many 0x7E bytes in a row (bus idle), (2) frames split at every possible byte boundary, (3) corrupted frames interleaved with valid ones, (4) frames with all 0x7D escape bytes in payload. Verifies the decoder never panics, never loses sync, and recovers from corruption to find the next valid frame.
- [ ] **Add registration race condition test** (`crates/launa-integration-tests/src/lib.rs`): Test that commands arriving during registration (before client ID assigned) are correctly queued and sent after registration completes. Currently untested — real firmware receives MQTT commands at any time.
- [ ] **Add multi-command queue drain test** (`crates/launa-integration-tests/src/lib.rs`): Queue 5+ commands, verify they drain one-per-Ready-window in FIFO order. Verify that NothingToSend is sent when queue empties. Test the bounded capacity cap (MAX_PENDING_COMMANDS=8) by queuing 9 commands and verifying the 9th is rejected.

## Code Review: launa-protocol Crate (2026-04-15)

Full review of `crates/launa-protocol/`. 11 source files, 115 tests (71 unit + 27 fuzz + 17 property), all passing. Well-structured, clean `no_std`, thorough testing including property-based and fuzz. Issues below are actionable improvements only.

### Moderate

- [ ] **Remove or repurpose `message.rs`** (`crates/launa-protocol/src/message.rs`): `MessageType` enum is exported from `lib.rs` but largely redundant with `IncomingMessage` in `dispatcher.rs`. `from_bytes()` returns `Unknown` for `0x0A 0xBF` and `0xFE 0xBF` (the most common types) since those need payload context, making it useless as a frame-level discriminator. Either: (a) remove `MessageType` and its re-export from `lib.rs`, or (b) integrate it into the dispatcher as a first-pass filter. Check downstream crates for usage first.
- [x] **Add `log::warn!` on parse failures in `dispatcher.rs`** (`crates/launa-protocol/src/dispatcher.rs`): Failed parses silently fall through to `IncomingMessage::Unknown`. Adding `log::warn!` on each `Err(_)` arm would surface protocol mismatches on real hardware without changing behavior. ~10 lines, one `log::warn!` per parse error path. No new dependency (`log` already in Cargo.toml).
- [x] **Add buffer size limit to `FrameDecoder`** (`crates/launa-protocol/src/frame.rs`): Streaming decoder has no bound on `buffer: Vec<u8>`. A noisy RS-485 line with repeated data between markers grows memory unboundedly on a 32 KiB heap. Add `max_buffer_size: usize` field (default 512 bytes, configurable via `with_max_buffer(size)` builder). When exceeded, reset buffer + state and increment `crc_error_count`. Prevents OOM on embedded.

### Low / Code Quality

- [ ] **Fix dead code in `status.rs` test** (`crates/launa-protocol/src/status.rs`): `test_parse_status_pumps_and_circ_blower` writes `payload[11] = 0x09` then immediately overwrites it with `payload[11] = (1 | (0 << 2) | (2 << 4))`. The first line is dead code. Remove the first assignment.
- [ ] **Verify `config.rs` pump5 bit decode matches hardware** (`crates/launa-protocol/src/config.rs`): `pump_configs[5]` decodes `(payload[6] >> 6) & 0x03` but skips bits 2-5 of byte 6. In `status.rs`, pump5 uses `payload[12] bits 0-1` and pump6 uses `bits 2-3`. The config response may use a different layout, but worth cross-referencing against real hardware captures during Phase 3 sniffing to confirm bits 2-5 are truly unused.

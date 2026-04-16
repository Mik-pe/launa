# Launa - Task Tracker

## Architecture

`app/` uses `esp-hal` + `embassy` (pure Rust, no_std, no C SDK). Workspace crates (launa-protocol, launa-hal, launa-mqtt, launa-ota, launa-core, launa-sim) are desktop-testable.

| Need | Crate | Notes |
|---|---|---|
| HAL (UART, GPIO) | `esp-hal` 1.0+ | Stable |
| WiFi | `esp-radio` | `unstable` feature |
| TCP/IP | `embassy-net` (smoltcp) | Async network stack |
| TLS | _not needed_ | Private WiFi only |
| MQTT | hand-rolled MQTT v5 | QoS, keepalive, reconnect, reassembly |
| OTA | `launa-esp-ota` | Custom: esp-storage + partition mgmt + rollback |
| NVS | `esp-nvs` | ESP-IDF compatible format |
| Async executor | `embassy` + `esp-rtos` | esp-rtos provides scheduler + embassy bridge |
| Time | `embassy-time` 0.5 | Via esp-hal timer driver |

## Hardware Testing & Flashing (ESP-WROOM-32)

### Dev Environment Setup

- [ ] **Install USB driver**: CP210x or CH340 VCP driver for ESP-WROOM-32 dev board. Verify COM port in Device Manager.
- [ ] **Install cargo-espflash**: cargo install cargo-espflash --locked. Verify with cargo espflash board-info --chip esp32.

### Config Provisioning

- [ ] **No bootstrap path for blank ESP32**: Fresh ESP32 has empty NVS, boots with placeholder defaults, never connects. Need: (a) config-flash via serial, (b) spflash NVS write, or (c) compile-time config injection.

### Protocol Sniffer (First Thing at the Spa)

**DO NOT skip.** Protocol docs are reverse-engineered -- verify against real BP6013G1 before sending commands.

Remote workflow: ESP32 at the spa publishes raw frames to MQTT. Run sniff-decode.py from your desk.

- [ ] **First field session: passive sniff**: Flash sniffer FW, take ESP32 + RS-485 module to spa, connect A/B to controller bus. Collect 30+ seconds of frames. Verify 0x7E-delimited frames, ~1s status updates, byte offsets match parser, message types correct.
- [ ] **Validate parser against real frames**: Feed sniffed hex through StatusUpdate::parse(), verify parsed values match spa display.
- [ ] **Document real protocol findings**: Update docs/protocol.md with any differences. Fix parser bugs.

### RS-485 Bench Testing (Requires USB-RS485 Adapter)

PC -> USB cable -> [USB-to-RS485 adapter] -> A/B wires -> [auto-dir RS-485 module] -> TX/RX -> [ESP32]

- [ ] **Order USB-to-RS485 adapter**: ~-10 on Amazon.
- [ ] **Wire bench setup**: 6 wires (TX, RX, VCC, GND, A, B).
- [ ] **Build scripts/spa-sim.py**: Python script sending real Balboa frames via USB-RS485.
- [ ] **RS-485 loopback test**: Flash -> spa-sim -> MQTT publish -> validate payload.

### Active Field Testing

- [ ] **Field test at spa (full stack)**: Registration, status parsing, MQTT publishing, HA command acceptance. Verify temperature, pump control, all entities.

## Production Readiness (Code Review 2026-04-16)

Full crate-by-crate review identified 7 critical, 19 high, 25 medium, 23 low issues. Critical and high items listed below.

### CRITICAL — Blocks Field Deployment

- [x] **Add hardware watchdog timer** (`app/src/main.rs`): TIMG1 WDT configured with 30s timeout, fed every main loop iteration. Resets device on stall.
- [x] **Fix main loop blocking on `frame_rx.receive().await`** (`app/src/main.rs`): Uses `embassy_futures::select::select()` to multiplex UART frames, MQTT commands, and 1-second tick timer. No longer blocks when spa is off.
- [x] **Cap MQTT `rx_buffer` at fixed size** (`app/src/mqtt_client.rs`): `rx_buffer: Vec<u8>` has no bound. Cap at 2 KiB; if exceeded without a complete packet, treat as protocol error and reconnect.
- [x] **Fix circ_pump/mister HA entities** (`launa-mqtt`): Changed from writable switches (with command topics) to read-only sensors — protocol doesn't support toggling these.
- [x] **Add firmware integrity verification to OTA** (`launa-esp-ota`, `app/src/ota.rs`): Validates ESP32 image header magic (0xE9) on first write. Accumulates CRC-32/MPEG-2 across all chunks. Supports expected hash via `?crc=HEX` URL parameter and Content-Length validation against partition size.
- [x] **Fix JSON escaping in `status_to_json`** (`launa-mqtt/src/state.rs`): Added `escape_json_string()` helper escaping `\`, `"`, `\n`, `\r`, `\t`, and control chars U+0000-U+001F → `\uXXXX`.
- [ ] **Set up CI pipeline** (`.github/workflows`): At minimum: `cargo test` + `cargo check` + `cargo fmt --check` on PRs to main.

### HIGH — Should Fix Before Production

- [x] **`Frame::encode()` silent truncation on payloads >253 bytes** (`launa-protocol/src/frame.rs`): Returns `Err(FrameError::PayloadTooLarge(len))` when `2 + payload.len() > 255`. All callers updated.
- [x] **`FrameDecoder` miscounts all parse failures as CRC errors** (`launa-protocol/src/frame.rs`): Renamed `crc_error_count` → `frame_error_count` (field, methods, tests, callers).
- [x] **`ClientIdAssignment` defaults to 0 on missing byte** (`launa-protocol/src/dispatcher.rs`): Returns `IncomingMessage::Unknown` when ID byte is missing instead of silently assigning 0.
- [x] **Panic on initial MQTT connect failure — no retry** (`app/src/main.rs`): Replaced panic with retry loop (up to 10 attempts, exponential backoff 5s-60s), falls back to `software_reset()` if all attempts fail.
- [ ] **UnsafeCell socket buffer reuse — document safety argument** (`app/src/mqtt_client.rs`): Add formal SAFETY comment explaining single-task context, or use `MaybeUninit` pattern.
- [x] **Discovery publishes ~20 JSON strings on heap — OOM burst risk** (`app/src/mqtt_client.rs`): Publish one at a time using DiscoveryBuilder, each payload dropped after publish.
- [ ] **Sniffer mode allocates unbounded String per frame** (`app/src/main.rs`): Replace `format!("{:02X}")` collect with `write!()` into pre-allocated buffer.
- [ ] **No firmware size / Content-Length validation during OTA** (`app/src/ota.rs`): Parse `Content-Length` from HTTP headers, validate against partition size before `begin()`.
- [x] **Light entity state boolean vs string mismatch** (`launa-mqtt`): Not a bug — HA compares `value_template` result as string against `payload_on`, so boolean `true` matches `"true"` at runtime. No code change needed.
- [x] **`DiscoveryBuilder` uses crate version not firmware version** (`launa-mqtt`): Accept `sw_version` as builder parameter instead of `env!("CARGO_PKG_VERSION")`.
- [x] **`finalize()` missing empty-image check** (`launa-esp-ota`): Added `bytes_written == 0` guard in `finalize()` — refuses to set boot to empty partition.
- [x] **Otadata both slots may share same 4 KiB sector** (`launa-esp-ota`): Fixed with read-modify-write pattern — reads full sector, patches target entry, erases, writes back. Both slots survive.
- [x] **Write offset tracking on unaligned chunks** (`launa-esp-ota`): Fixed with partial-word buffering. `write_offset` now advances by actual data length, with a `pending_bytes` buffer for incomplete words. `finalize()` flushes remaining bytes. Consecutive unaligned writes produce contiguous data.
- [x] **Verify CRC32 matches ESP-IDF bootloader expectations** (`launa-esp-ota`): CRC-32/MPEG-2 verified against standard test vector ("123456789" → 0x0376E6E7). Incremental CRC matches one-shot. Empty input returns 0xFFFFFFFF.
- [x] **Transport trait is sync but production is async** (`launa-hal`): Unified Transport trait to async using `async fn` in trait. MockTransport (std feature), SimTransport, and Rs485Transport all implement the unified `launa_hal::Transport` trait directly. Tests use a lightweight `block_on` helper to poll async methods synchronously.
- [ ] **Integration tests use SpaController not real SpaApp** (`launa-integration-tests`): Tests validate sim framework, not production logic. Rewrite to exercise `SpaApp` through sim pipeline.
- [ ] **No OTA / reconnection / stale-detection integration tests** (`launa-integration-tests`): Critical production paths untested at integration level.
- [ ] **`ota-flash` does not verify firmware version after update** (`xtask`): Device could roll back to factory and still appear online. Check reported version.
- [x] **Command queue in SpaApp is unbounded** (`launa-core`): Capped at 32 commands; overflow increments `dropped_count` via `record_dropped()`.
- [x] **Celsius temperature commands never confirm in CommandTracker** (`launa-core/src/lib.rs:266`): `ExpectedChange::TemperatureSet` compares `(status.set_temp as u8) == *temp`. In Celsius mode, `set_temp` is `raw/2.0` (e.g. `77/2 = 38.5`), so `38.5 as u8` truncates to `38 != 77`. Every odd-wire-value Celsius set-temp command will fail confirmation and retry until dropped. Fix: compare raw wire values by multiplying back (`(status.set_temp * temp_divisor) as u8`), or store the raw value alongside `set_temp`.
- [x] **Duplicate divergent HA discovery implementations** (`app/src/mqtt_client.rs` vs `launa-mqtt/src/discovery.rs`): Refactored `publish_discovery()` to use `DiscoveryBuilder::build_with_retain()`. Discovery module now works in `no_std` (manual JSON, no serde_json). All 20 configs include `origin`, `sw_version`, and `entity_category: "diagnostic"` on diagnostics/alert sensors.
- [x] **MQTT LWT Will Retain flag not set** (`app/src/mqtt_client.rs` `send_connect()`): Set bit 5 (`| (1 << 5)`) in `connect_flags` for Will Retain. LWT "offline" is now retained by the broker.
- [x] **First temperature command accepted without scale/range validation** (`app/src/mqtt_client.rs`): Rejects temperature commands when `scale`/`range` is `None` (before first status received). Returns `None` with a `warn!` log.

### MEDIUM — Recommended

- [ ] **Add panic-reboot handler** (`app/`): `esp-backtrace` panic handler loops forever. Add custom handler that logs backtrace, waits, then `software_reset()`.
- [ ] **Add MQTT command rate limiting** (`app/src/mqtt_client.rs`): Max N commands per 10 seconds. Drop excess to protect spa bus.
- [ ] **Validate OTA URL scheme** (`app/src/mqtt_client.rs`): Require `http://`, reject arbitrary schemes.
- [x] **Add firmware version to diagnostics JSON** (`app/src/main.rs`): Include `env!("CARGO_PKG_VERSION")` in periodic diagnostics payload.
- [ ] **Mask sensitive fields in hw-test logging** (`app/src/main.rs`): Don't log WiFi SSID/MQTT host in production.
- [x] **`SpaApp::device_id` stored but never read** (`launa-core`): Removed dead `device_id` field and `new()` parameter — callers pass it separately.
- [x] **Missing entity_category "diagnostic" on alert/diagnostics sensors** (`launa-mqtt`): Should appear in HA diagnostics section, not alongside primary sensors.
- [x] **Sim responses are static/hardcoded** (`launa-sim`): Fault log, filter cycles, information, config all return fixed data. Add configurability for testing edge cases.
- [ ] **SpaController ignores config/fault/filter/info responses** (`launa-sim`): Only handles StatusUpdate, Ready, NewClientQuery. Other responses discarded in integration tests.
- [ ] **Config validation gaps in xtask** (`xtask/src/config.rs`): No validation of `device.id` format/length, serial port existence, or MQTT port range.
- [ ] **xtask argument parsing panics on missing flag values** (`xtask/src/*.rs`): `--feature` as last arg = index out of bounds. Add bounds checks.
- [x] **No firmware versioning mechanism** (cross-cutting): No build hash or version embedded in binary or reported via MQTT.
- [ ] **Cargo.lock gitignored at workspace root** (`.gitignore`): Workspace library builds not reproducible across machines.
- [x] **Stale probe sends ConfigurationRequest instead of lightweight probe** (`launa-core/src/lib.rs:755`): When the spa is stale, the 5-second probe sends `[0x0A, 0xBF, 0x04]` (ConfigurationRequest). This is heavier than necessary and triggers an unwanted full configuration response. Fix: use a no-op or status-specific request instead.
- [ ] **MQTT `try_extract_packet()` heap-churns every inbound packet** (`app/src/mqtt_client.rs`): `self.rx_buffer = Vec::from(&self.rx_buffer[total_size..])` allocates a new `Vec` for every MQTT packet. On a 32 KiB heap with frequent traffic, this contributes to fragmentation. Fix: use `Vec::drain()` or a rotating-index buffer to avoid per-packet allocation.
- [ ] **`EspOtaFlash::set_boot_partition()` erases 4 KiB sector for 32-byte otadata entry** (`launa-esp-ota`): Both otadata entries (32 bytes each) may share the same 4 KiB flash sector. Erasing slot 1 could destroy slot 0 on power loss. Fix: read-modify-write the sector (read full sector, modify the 32-byte entry, erase, rewrite), or verify slots are on independent sectors per the partition table.

## Completed Work

All software development is complete. The firmware compiles for xtensa-esp32-none-elf, all workspace tests pass, and the codebase has been through two full code reviews (2026-04-15).

### Key Milestones

- **Protocol**: CRC-8, HDLC framing, status parser, command builder, registration state machine, fault/filter/info parsers, temperature safety clamping
- **MQTT**: Hand-rolled MQTT v5 client with QoS 1, keepalive, reconnect with backoff, packet reassembly, 20-entity HA auto-discovery, command parsing, diagnostics/alert topics
- **OTA**: Custom launa-esp-ota crate, HTTP download over embassy-net TCP, dual partition slots, boot validation + auto-rollback, graceful shutdown sequence
- **Robustness**: CommandTracker (ACK/retry/drop), stale-status detection (5s probe, 30s stale), heap monitoring, HoldModeTimer (60min safety), bounded command queue, exponential backoff on reconnect, alert throttling
- **Security**: Encrypted NVS config (AES-128-CTR via ESP32 hardware AES, eFuse key), cargo xtask provision
- **Testing**: SpaSim with error injection (command failure, bus silence, corrupt frames), SimBroker with connection loss simulation, SpaApp architecture (all logic in launa-core, ESP32 is thin IO wiring), 24-hour simulation, stress tests
- **xtask**: flash, monitor, flash-monitor, sniff-decode, spa-sim, ota-serve, ota-flash, self-test, config-flash, provision
- **Features**: sniffer mode, hw-test mode, pump timers, light color cycling, Pump1-6 + Light1-2 support

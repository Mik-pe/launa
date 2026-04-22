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

Remote workflow: ESP32 at the spa publishes raw frames to MQTT. Run `cargo xtask sniff-decode` from your desk.

- [ ] **First field session: passive sniff**: Flash sniffer FW, take ESP32 + RS-485 module to spa, connect A/B to controller bus. Collect 30+ seconds of frames. Verify 0x7E-delimited frames, ~1s status updates, byte offsets match parser, message types correct.
- [ ] **Validate parser against real frames**: Feed sniffed hex through StatusUpdate::parse(), verify parsed values match spa display.
- [ ] **Document real protocol findings**: Update docs/protocol.md with any differences. Fix parser bugs.

### RS-485 Bench Testing (Requires USB-RS485 Adapter)

PC -> USB cable -> [USB-to-RS485 adapter] -> A/B wires -> [auto-dir RS-485 module] -> TX/RX -> [ESP32]

- [ ] **Order USB-to-RS485 adapter**: ~-10 on Amazon.
- [ ] **Wire bench setup**: 6 wires (TX, RX, VCC, GND, A, B).
- [ ] **Build xtask spa-sim**: Rust tool sending real Balboa frames via USB-RS485 (`cargo xtask spa-sim`).
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
- [x] **Set up CI pipeline** (`.github/workflows`): At minimum: `cargo test` + `cargo check` + `cargo fmt --check` on PRs to main.

### HIGH — Should Fix Before Production

- [x] **`Frame::encode()` silent truncation on payloads >253 bytes** (`launa-protocol/src/frame.rs`): Returns `Err(FrameError::PayloadTooLarge(len))` when `2 + payload.len() > 255`. All callers updated.
- [x] **`FrameDecoder` miscounts all parse failures as CRC errors** (`launa-protocol/src/frame.rs`): Renamed `crc_error_count` → `frame_error_count` (field, methods, tests, callers).
- [x] **`ClientIdAssignment` defaults to 0 on missing byte** (`launa-protocol/src/dispatcher.rs`): Returns `IncomingMessage::Unknown` when ID byte is missing instead of silently assigning 0.
- [x] **Panic on initial MQTT connect failure — no retry** (`app/src/main.rs`): Replaced panic with retry loop (up to 10 attempts, exponential backoff 5s-60s), falls back to `software_reset()` if all attempts fail.
- [x] **UnsafeCell socket buffer reuse — document safety argument** (`app/src/mqtt_client.rs`): Add formal SAFETY comment explaining single-task context, or use `MaybeUninit` pattern.
- [x] **Discovery publishes ~20 JSON strings on heap — OOM burst risk** (`app/src/mqtt_client.rs`): Publish one at a time using DiscoveryBuilder, each payload dropped after publish.
- [x] **Sniffer mode allocates unbounded String per frame** (`app/src/main.rs`): Replace `format!("{:02X}")` collect with `write!()` into pre-allocated buffer.
- [x] **No firmware size / Content-Length validation during OTA** (`app/src/ota.rs`): Parse `Content-Length` from HTTP headers, validate against partition size before `begin()`.
- [x] **Light entity state boolean vs string mismatch** (`launa-mqtt`): Not a bug — HA compares `value_template` result as string against `payload_on`, so boolean `true` matches `"true"` at runtime. No code change needed.
- [x] **`DiscoveryBuilder` uses crate version not firmware version** (`launa-mqtt`): Accept `sw_version` as builder parameter instead of `env!("CARGO_PKG_VERSION")`.
- [x] **`finalize()` missing empty-image check** (`launa-esp-ota`): Added `bytes_written == 0` guard in `finalize()` — refuses to set boot to empty partition.
- [x] **Otadata both slots may share same 4 KiB sector** (`launa-esp-ota`): Fixed with read-modify-write pattern — reads full sector, patches target entry, erases, writes back. Both slots survive.
- [x] **Write offset tracking on unaligned chunks** (`launa-esp-ota`): Fixed with partial-word buffering. `write_offset` now advances by actual data length, with a `pending_bytes` buffer for incomplete words. `finalize()` flushes remaining bytes. Consecutive unaligned writes produce contiguous data.
- [x] **Verify CRC32 matches ESP-IDF bootloader expectations** (`launa-esp-ota`): CRC-32/MPEG-2 verified against standard test vector ("123456789" → 0x0376E6E7). Incremental CRC matches one-shot. Empty input returns 0xFFFFFFFF.
- [x] **Transport trait is sync but production is async** (`launa-hal`): Unified Transport trait to async using `async fn` in trait. MockTransport (std feature), SimTransport, and Rs485Transport all implement the unified `launa_hal::Transport` trait directly. Tests use a lightweight `block_on` helper to poll async methods synchronously.
- [x] **Integration tests use SpaController not real SpaApp** (`launa-integration-tests`): Tests validate sim framework, not production logic. Rewrite to exercise `SpaApp` through sim pipeline.
- [x] **No OTA / reconnection / stale-detection integration tests** (`launa-integration-tests`): Critical production paths untested at integration level.
- [x] **`ota-flash` does not verify firmware version after update** (`xtask`): Device could roll back to factory and still appear online. Check reported version.
- [x] **Command queue in SpaApp is unbounded** (`launa-core`): Capped at 32 commands; overflow increments `dropped_count` via `record_dropped()`.
- [x] **Celsius temperature commands never confirm in CommandTracker** (`launa-core/src/lib.rs:266`): `ExpectedChange::TemperatureSet` compares `(status.set_temp as u8) == *temp`. In Celsius mode, `set_temp` is `raw/2.0` (e.g. `77/2 = 38.5`), so `38.5 as u8` truncates to `38 != 77`. Every odd-wire-value Celsius set-temp command will fail confirmation and retry until dropped. Fix: compare raw wire values by multiplying back (`(status.set_temp * temp_divisor) as u8`), or store the raw value alongside `set_temp`.
- [x] **Duplicate divergent HA discovery implementations** (`app/src/mqtt_client.rs` vs `launa-mqtt/src/discovery.rs`): Refactored `publish_discovery()` to use `DiscoveryBuilder::build_with_retain()`. Discovery module now works in `no_std` (manual JSON, no serde_json). All 20 configs include `origin`, `sw_version`, and `entity_category: "diagnostic"` on diagnostics/alert sensors.
- [x] **MQTT LWT Will Retain flag not set** (`app/src/mqtt_client.rs` `send_connect()`): Set bit 5 (`| (1 << 5)`) in `connect_flags` for Will Retain. LWT "offline" is now retained by the broker.
- [x] **First temperature command accepted without scale/range validation** (`app/src/mqtt_client.rs`): Rejects temperature commands when `scale`/`range` is `None` (before first status received). Returns `None` with a `warn!` log.
- [x] **Fix JSON escaping in discovery builder** (`launa-mqtt/src/discovery.rs`): `json_device_block()` and `json_origin()` now escape `device_name`, `manufacturer`, `model`, and `sw_version` via shared `escape_json_string()` from new `escape.rs` module. All 27 discovery configs produce valid JSON with special characters (quotes, backslashes, control chars). Added `manufacturer()` builder method.

### MEDIUM — Recommended

- [x] **Add panic-reboot handler** (`app/`): Custom panic handler replaces esp-backtrace infinite loop. Logs panic location via `esp_println!`, busy-waits ~500ms for UART flush, then calls `software_reset()`.
- [x] **Add MQTT command rate limiting** (`app/src/mqtt_client.rs`): Max N commands per 10 seconds. Drop excess to protect spa bus.
- [x] **Validate OTA URL scheme** (`app/src/mqtt_client.rs`): Require `http://`, reject arbitrary schemes.
- [x] **Add firmware version to diagnostics JSON** (`app/src/main.rs`): Include `env!("CARGO_PKG_VERSION")` in periodic diagnostics payload.
- [x] **Mask sensitive fields in hw-test logging** (`app/src/main.rs`): WiFi SSID and MQTT host masked (first 2 chars + "***") in hw-test log output. device_id and port remain visible for debugging.
- [x] **`SpaApp::device_id` stored but never read** (`launa-core`): Removed dead `device_id` field and `new()` parameter — callers pass it separately.
- [x] **Missing entity_category "diagnostic" on alert/diagnostics sensors** (`launa-mqtt`): Should appear in HA diagnostics section, not alongside primary sensors.
- [x] **Sim responses are static/hardcoded** (`launa-sim`): Fault log, filter cycles, information, config all return fixed data. Add configurability for testing edge cases.
- [x] **SpaController ignores config/fault/filter/info responses** (`launa-sim`): Only handles StatusUpdate, Ready, NewClientQuery. Other responses discarded in integration tests.
- [x] **Config validation gaps in xtask** (`xtask/src/config.rs`): No validation of `device.id` format/length, serial port existence, or MQTT port range.
- [x] **xtask argument parsing panics on missing flag values** (`xtask/src/*.rs`): `--feature` as last arg = index out of bounds. Add bounds checks.
- [x] **No firmware versioning mechanism** (cross-cutting): No build hash or version embedded in binary or reported via MQTT.
- [x] **Cargo.lock gitignored at workspace root** (`.gitignore`): Removed from .gitignore and committed for reproducible builds.
- [x] **Stale probe sends ConfigurationRequest instead of lightweight probe** (`launa-core/src/lib.rs:755`): When the spa is stale, the 5-second probe sends `[0x0A, 0xBF, 0x04]` (ConfigurationRequest). This is heavier than necessary and triggers an unwanted full configuration response. Fix: use a no-op or status-specific request instead.
- [x] **MQTT `try_extract_packet()` heap-churns every inbound packet** (`app/src/mqtt_client.rs`): Replaced double `Vec::from()` with `Vec::drain()` in `launa-mqtt/src/packet.rs`. Single allocation per packet extraction, shifts tail in-place.
- [x] **`EspOtaFlash::set_boot_partition()` erases 4 KiB sector for 32-byte otadata entry** (`launa-esp-ota`): Fixed with read-modify-write pattern — reads full sector, patches target entry, erases, writes back. Both slots survive sequential and alternating writes.

## Deployment Readiness (Review 2026-04-16)

Code is feature-complete. Remaining items are deployment infrastructure and operational hardening needed to flash and run on a real ESP32.

### CRITICAL — Blocks First Flash to Hardware

- [x] **Create `app/partitions.csv`**: Every xtask flash command passes `--partition-table partitions.csv`. Must match hardcoded offsets: NVS at `0x9000` (24 KiB), otadata at `0x10000` (8 KiB), factory at `0x20000` (1.25 MiB), ota_0 at `0x160000` (1.25 MiB), ota_1 at `0x2A0000` (1.25 MiB).
- [x] **Pin and document ESP32 Rust toolchain**: No `rust-toolchain.toml`. The `xtensa-esp32-none-elf` target is set in `app/.cargo/config.toml` but the toolchain channel (nightly, esp-rs rustup component, etc.) is not documented. A fresh developer has no setup guide.
- [x] **Add `app/Cargo.lock` to git**: Currently untracked (`?? app/Cargo.lock` in git status). The app is excluded from the workspace and has its own lockfile — must be committed for reproducible ESP32 builds.

### HIGH — Should Fix Before Production Deployment

- [x] **Add DNS resolution**: Added `dns` feature to embassy-net, `resolve_host()` in net_util.rs (tries IPv4 parse first, then DNS A-record query via `Stack::dns_query()`). MQTT connect/reconnect and OTA now support hostnames. `StackResources` increased to 4 for DNS socket.
- [x] **Add ESP32 cross-compilation to CI**: Added `esp-check` job to `.github/workflows/ci.yml` using `esp-rs/xtensa-toolchain@v1.5` to verify `app/` compiles for `xtensa-esp32-none-elf`.
- [x] **Add firmware versioning strategy**: Added `app/build.rs` that captures Git short SHA via `git rev-parse --short HEAD`. `FIRMWARE_VERSION` now includes it: `"0.1.0 (abc1234)"`. Falls back to `GITHUB_SHA` in CI or `"unknown"`.
- [x] **Fix sniffer mode MQTT connect panic**: In `#[cfg(feature = "sniff")]` main, MQTT connect failure calls `panic!("MQTT connect failed")` unlike the main firmware's retry loop. Should use the same retry-with-backoff pattern.
- [x] **Pin exact versions for `esp-radio` and `esp-hal` unstable features**: Both use `unstable` feature flag, meaning API may change between versions without warning. Verify exact pins in `app/Cargo.lock`.

### MEDIUM — Operational Gaps

- [x] **Add WiFi reconnection integration test**: `wifi.rs` has `connection_task` handling disconnect/reconnect + MQTT signal. This critical path has no integration test.
- [ ] **Add OTA end-to-end test on hardware**: OTA flow (HTTP download → flash partition → reboot → mark valid) cannot be tested on desktop. Integration tests use `MockOta`. No hardware test harness exists.
- [ ] **Evaluate `esp-rtos` maturity**: v0.2.0 is relatively new. Check for known issues around task scheduling and timer accuracy before relying on it in production.
- [ ] **Heap fragmentation analysis**: 32 KiB heap is tight for MQTT + JSON + OTA. Long-term `Vec` churn could fragment. No fragmentation analysis exists.
- [x] **Add remote logging capability**: Logs only visible over serial (`log` + `esp-println`). No remote logging (e.g., sending log messages to MQTT or a collector) for production diagnostics.
- [ ] **No RS-485 transceiver reset capability**: No GPIO to hardware-reset the MAX485/MAX3485 if it gets into a bad state.

### LOW — Nice-to-Have

- [x] **Add firmware binary signing for OTA**: OTA accepts any HTTP-served binary with correct ESP32 image header magic (`0xE9`). CRC is optional (only if `?crc=` in URL). An attacker on the LAN could serve malicious firmware.
- [x] **Add unit tests for `app/` modules**: `mqtt_client.rs` (800+ lines), `ota.rs` (400+ lines), `wifi.rs` have no unit tests — only tested through `launa-core` integration pipeline. Desktop-testable logic (parse_ota_url, command_parser, topic construction, status JSON, packet extraction) now has 30+ additional tests in `launa-mqtt`.
- [x] **Add `xtask` dependency pin check to CI**: `xtask` depends on `serialport`, `keyring`, `rand`, `toml` but CI does not verify xtask builds.

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

## Logic Flaws & Bugs (Deep Audit 2026-04-16)

Comprehensive code audit across all crates plus online protocol reference comparison. Findings merged from three parallel worker audits and manual review.

### CRITICAL — Bugs That Will Cause Incorrect Behavior

- [x] **Celsius set-temperature sends display value instead of wire value** (`app/src/mqtt_client.rs`): Fixed in `parse_command()` — after validation, Celsius display values are multiplied by 2 to produce wire values (`saturating_mul(2)`). Fahrenheit passes through unchanged.
- [x] **MQTT v5 PUBLISH property length parsed as single byte** (`app/src/mqtt_client.rs`): Fixed — replaced single-byte read with `decode_remaining_length()` for variable-byte property length decoding.
- [x] **MQTT SUBACK reads full packet** (`app/src/mqtt_client.rs`): Replaced `read_exact(buf, 5)` with proper SUBACK reader: reads fixed header, variable-byte remaining length, full payload. Property length now decoded with `decode_remaining_length()`. No bytes left in TCP stream.
- [x] **Hold mode timer re-fires on every status if spa is slow to respond** (`launa-core/src/lib.rs`): Added `fired` flag to `HoldModeTimer` — after firing, returns `None` until hold mode is released. Prevents toggle-command spam.
- [x] **Stale command state survives bus reset** (`launa-core/src/lib.rs`): On `NewClientQuery`, `command_queue.clear()` and `cmd_tracker.reset()` are now called. Added `reset()` method to `CommandTracker`.

### HIGH — Significant Issues

- [x] **Information response `software_id` uses standard Balboa format** (`launa-protocol/src/information.rs`): Changed from hex `"64DC_1100"` to standard `"M100_220 V17.0"` format matching pybalboa/NorthernMan54.
- [x] **HA discovery set-temperature supports °C mode** (`launa-mqtt/src/discovery.rs`): Added `celsius(bool)` builder method. Celsius mode: min=10, max=40, step=0.5, unit=°C. Temperature sensor unit also adapts. Default remains Fahrenheit.
- [x] **`OTA_CHANNEL.try_send()` silently drops OTA URL** (`app/src/main.rs`): Channel capacity is 1. If an OTA URL is queued while one is already pending, the new URL is dropped. User sees MQTT acknowledgment but no OTA occurs. Should use `.send()` (blocking) or handle the error.
- [x] **NVS init failure causes unrecoverable boot loop** (`app/src/config.rs` `open_nvs()`): `panic!("NVS init failed")` on corrupted NVS. Device resets → panics again → infinite loop. Should fall back to default config with a warning.
- [x] **UART flush error silently ignored during RS-485 DE release** (`app/src/transport.rs`): `let _ = self.uart.flush();` — if flush fails, DE pin drops before all bytes are on the wire, corrupting the last byte(s) of the RS-485 frame (CRC mismatch on spa side).
- [x] **Unbounded `line_buf` in hw-test config mode** (`app/src/main.rs`): `Vec::push(byte)` without size limit on a 32 KiB heap. Continuous serial data without newlines causes OOM panic → device reset.

### MEDIUM — Should Fix

- [x] **Missing 9 toggle item codes** (`launa-protocol/src/command.rs`): MISTER (0x0E), CIRCULATION_PUMP (0x3D), LIGHT_3 (0x13), LIGHT_4 (0x14), AUX_1 (0x16), AUX_2 (0x17), SOAK_MODE (0x1D), NORMAL_OPERATION (0x01), CLEAR_NOTIFICATION (0x03). Status parser already handles mister and circ_pump but can't toggle them.
- [x] **MQTT task diagnostics/alert `try_receive` + `continue` starves command processing** (`app/src/main.rs` mqtt_task): A burst of diagnostics/alert publishes prevents incoming MQTT commands from being processed. Should limit consecutive non-command receives.
- [x] **`from_hex()` in crypto.rs allocates without size limit** (`app/src/crypto.rs`): `Vec::with_capacity(hex.len() / 2)` — a malformed NVS value with a very long hex string causes OOM on 32 KiB heap during config loading.
- [x] **All-zeros eFuse key produces weak predictable encryption** (`app/src/crypto.rs` `read_key()`): On unprovisioned devices, BLOCK3 is all zeros. After XOR mixing, key = `[0xA5, 0x3C, 0x96, 0xF0]` repeated — identical across all unprovisioned devices. Should log warning.
- [x] **`RequestOta` action never tested through SpaApp** (`launa-integration-tests`): The OTA MQTT command → SpaApp → `AppAction::RequestOta` path has zero integration test coverage. If the URL propagation has a bug, it would only surface in production.
- [x] **OTA `header_buf` Vec allocates up to 4 KiB on 32 KiB heap** (`app/src/ota.rs`): Combined with other allocations during OTA (request string, HTTP response), could cause OOM. Should use fixed-size stack buffer.
- [x] **Pump timer auto-off when pump manually turned off — untested** (`launa-core/src/lib.rs` `PumpTimer::tick()`): `if !is_on { cancel }` path is never tested. If buggy, auto-off timer could re-start a pump the user intentionally turned off.
- [x] **Validated temperature integration untested end-to-end** (`launa-integration-tests`): `parse_set_temperature_validated()` is unit-tested but never called from integration tests. The full path (MQTT payload → validated parse → SpaApp queue → Ready → wire frame) is untested.

### Simulator Fidelity & Adversarial Testing

The simulator has solid protocol-level fidelity and basic error injection, but tests are predominantly happy-path. No integration tests exist in `launa-integration-tests` (directory is empty). The two controller implementations (`SpaController` in launa-sim vs `SpaApp` in launa-core) diverge — sim tests exercise the simplified one, not production logic. Several categories of real-world misbehavior are untested.

#### Tier 1 — Integration Test Harness (Foundation)

Wire SpaSim + SimTransport + SpaApp + SimBroker together for end-to-end tests. These are the highest-value improvements because they test the actual production logic path.

- [x] **Create integration test harness in `launa-integration-tests`**: Test pipeline `SpaSim.tick() → FrameDecoder → SpaApp.process_frame() → verify AppActions`. SpaAppTestHarness wires SpaSim → FrameDecoder → SpaApp → SimBroker with VirtualClock.
- [x] **Test: full registration handshake end-to-end**: SpaSim sends registration query → SpaApp responds → ID assignment → ID ack → `is_registered()`. Verify SpaApp sends correct frames at each step.
- [x] **Test: status updates flow through to MQTT publish actions**: SpaSim ticks → SpaApp processes status frames → `AppAction::PublishState` emitted → verify state JSON matches sim state.
- [x] **Test: MQTT command → SpaApp queue → Ready → wire frame**: SimBroker receives command → SpaApp.on_mqtt_command() → Ready frame → SpaApp sends toggle → SpaSim receives and applies → status confirms change.
- [x] **Test: pump timer auto-off end-to-end**: Start pump timer → SpaApp sends toggle on → SpaSim turns pump on → advance virtual clock past duration → SpaApp sends toggle off → verify SpaSim pump is off.
- [x] **Test: hold mode timer auto-release**: SpaSim enters hold → advance clock past 60 min → SpaApp sends hold-mode toggle → verify hold released.
- [x] **Test: stale detection and recovery**: SpaSim goes silent (bus silence) → advance clock past 30s → SpaApp publishes stale alert → SpaSim resumes → SpaApp publishes recovering-from-stale state.

#### Tier 2 — Spa-Side Fault Scenarios

Add new error injection capabilities to SpaSim and integration tests that exercise them. These test the most critical firmware robustness behaviors.

- [x] **Add `SpaSim::simulate_spa_reboot()`**: Resets to unregistered state, re-sends registration query, clears all spa state. Tests whether SpaApp re-registers cleanly, flushes command queue, resets trackers.
- [x] **Add `SpaSim::simulate_fault_state(FaultCode)`**: Enters fault mode (init_mode=0x02), possibly forces pumps/heater off. Tests whether SpaApp correctly reports fault state to MQTT.
- [x] **Add `SpaSim::simulate_sensor_noise(temp_jitter: f32)`**: Adds random noise to current_temp each tick. Tests whether SpaApp/MQTT state remains stable (no flip-flopping reported values).
- [x] **Add `SpaSim::simulate_unknown_temp()`**: Reports 0xFF for current_temp. Tests whether SpaApp handles `current_temp: None` correctly in MQTT JSON.
- [x] **Add `SpaSim::simulate_spontaneous_state_change()`**: Spa changes state (pump, heating mode, temp range) without controller command. Tests whether CommandTracker correctly does NOT treat this as a confirmation of a queued command.
- [x] **Test: spa reboots mid-session**: SpaSim sends status normally → reboots → SpaApp detects NewClientQuery → re-registers → command queue flushed → state resets → normal operation resumes.
- [x] **Test: spa silently drops toggle command**: Set command_success_rate to 0.0 → SpaApp sends toggle → SpaSim ignores → SpaApp command tracker times out → retry → eventually drops. Verify MQTT reflects correct final state.
- [x] **Test: bus silence mid-session**: Normal operation → 15s silence → stale probe fires → 30s silence → stale alert → silence ends → recovery. Full lifecycle.
- [x] **Test: corrupt frame doesn't desync parser**: SpaSim injects corrupt frame → next valid status frame still parses → SpaApp continues normally. Verify FrameDecoder recovers.
- [x] **Test: spontaneous filter cycle starts while command pending**: SpaSim schedules filter cycle → pump turns on from filter → SpaApp had queued a different pump toggle → CommandTracker must not misattribute the filter cycle as confirmation.

#### Tier 3 — Protocol-Level Misbehavior

- [x] **Test: out-of-order frames**: SpaSim sends Ready before Status in a single tick. Verify SpaApp handles gracefully (Ready with no pending status shouldn't crash).
- [x] **Test: interleaved response and status**: Status frame arrives between settings request and settings response. Verify both parse correctly.
- [x] **Test: rapid re-registration**: SpaSim sends multiple registration queries in quick succession. Verify SpaApp doesn't double-register or assign wrong ID.
- [x] **Test: partial frame across tick boundary**: Use existing `inject_partial_frame_at()` in integration context. Verify SpaApp's FrameDecoder reassembles correctly.
- [x] **Test: duplicate status frame in one tick**: Use existing `inject_duplicate_frame()`. Verify SpaApp doesn't double-publish to MQTT.
- [x] **Test: multi-frame fault log walk**: Request fault entries 1..N sequentially. Verify each response is correctly captured and last_fault updates.

#### Tier 4 — Physics Improvements

- [x] **Realistic thermal model**: Replace linear +/-1°/tick with rate proportional to temp delta (cooling) and heater output (heating). ~0.5°F/min heating, ~0.1°F/min cooling.
- [x] **Temperature sensor noise**: Small random jitter (+/-0.5°F) on current_temp each tick. Real sensors are noisy.
- [x] **Heater/pump interlock**: `is_heating` should only be true when circ pump or at least one pump is running. Spa won't heat without water circulation.
- [x] **Temperature unknown (0xFF) on startup**: First N ticks after sim creation should report current_temp as 0xFF (unknown) until sensor stabilizes.
- [x] **Heater overshoot**: Allow temp to overshoot set_temp by 1-2°F before thermostat cuts off. Matches real spa behavior.

#### Tier 5 — Multi-Frame Protocol & Advanced

- [x] **Test: fault log walk (entries 1..N)**: SpaSim supports multiple fault entries. Test sequential request → response → request next → response cycle.
- [x] **Test: configuration request/response pairing**: Request config → verify SpaApp emits `ConfigurationResponse` event with correct pump/light/blower setup.
- [x] **Test: filter cycle edit commands**: Not just read — test setting filter cycle times if protocol supports it.
- [x] **Remove SpaController from launa-sim or mark deprecated**: Tests should use `SpaApp` from launa-core. `SpaController` is a simplified duplicate that diverges from production logic.
- [x] **Test: MQTT broker disconnect/reconnect during active session**: SimBroker.disconnect() → SpaApp publishes go silent → reconnect → verify state sync recovers.
- [x] **Test: rapid command flood exceeds queue cap**: Send 40 MQTT commands rapidly → verify SpaApp caps at 32 and increments dropped_count → verify remaining commands drain correctly.

### LOW — Minor Issues

## Code Quality Review (2026-04-20)

Crate-by-crate review for clean code, deduplication, bad tests, and UX. Five parallel worker audits covering protocol, MQTT, HAL/sim, app/integration-tests, and web/xtask.

### HIGH — Structural / Maintainability

- [ ] **Extract `dispatch_frame()` mega-function into message-type handlers** (`launa-protocol/src/dispatcher.rs`): 245-line function with 4+ levels of nesting. ~30 `Unknown` constructions and 8 duplicated parse-warn-fallback patterns. Split into `handle_status()`, `handle_config()`, `handle_registration()`, etc.
- [ ] **Break `SpaSim` god object into focused subsystems** (`launa-sim/src/spa_sim/mod.rs`): 39 fields, 500+ lines of impl. Extract `ErrorInjection`, `FaultManager`, `FrameSplitter` subsystems.
- [ ] **Break `main()` god function into named functions** (`app/src/main.rs`): ~250-line non-hw-test main handles init, WiFi, MQTT, OTA, event loop, self-test, sniff mode. Extract `init_wifi()`, `init_mqtt()`, `handle_ota_request()`, `run_event_loop()`.
- [ ] **Replace `STATE_CHANNEL` 5-tuple with named struct** (`app/src/main.rs` line 151): `(StatusUpdate, FaultBuf, bool, bool, bool)` — the three bools are unreadable. Use `struct StateMessage { status, fault, recovering_from_stale, self_test, sniff_mode }`.
- [ ] **Extract reconnect-and-sync helper** (`app/src/mqtt_task.rs`): Post-reconnect sequence (availability → discovery → subscribe → state) duplicated 3 times. Extract `reconnect_and_sync()`.
- [ ] **Extract shared `LoadingSpinner.vue` component** (`web/src/`): Identical spinner SVG copy-pasted in 7 components (App.vue, StatusDashboard, TemperatureChart, LogViewer, AlertsView, DiagnosticsView, SniffFramesView).
- [ ] **Split `useMqtt.ts` god composable** (`web/src/composables/useMqtt.ts`): 210+ lines handling connection, state, settings, commands, pending tracking, config fetching. Split into `useMqttConnection`, `useSpaState`, `useSettings`, `useAccessoryConfig`.

### MEDIUM — Deduplication

- [ ] **Deduplicate pump bit-decode logic** (`launa-protocol`): Pump bit-decoding duplicated between `status.rs` and `config.rs`. Extract shared helper.
- [ ] **Deduplicate UART register constants and write logic** (`app/src/main.rs` vs `app/src/logger.rs`): Both define `UART0_BASE`, `UART_STATUS_REG`, FIFO size constants, and byte-write-then-spin-loop. Extract shared `uart_raw` module.
- [ ] **Deduplicate hex encoding** (`app/src/main.rs` `publish_sniff_frame` vs `app/src/crypto.rs` `to_hex`): Reuse `crypto::to_hex()` in sniff frame publisher.
- [ ] **Extract LCG PRNG into shared utility** (`launa-sim`): Magic constants `6364136223846793005`/`1442695040888963407` appear in `sim_broker.rs` and `physics.rs`. Different constants in `mod.rs`. Extract `fn lcg_next(state: &mut u64) -> u64`.
- [ ] **Extract shared test helpers** (`tests/`): `full_registration()`, `sim_tick_to_app()`, `decode_first_frame()`, `make_new_client_query_frame()` copy-pasted across `registration_tests.rs`, `command_retry_tests.rs`, `spaapp_command_tests.rs`. Use `TestHarness` or `common/` module.
- [ ] **Extract status-frame decode helper** (`tests/`): Generate → decode → dispatch → match `StatusUpdate` pattern repeated ~40 times. Add `TestHarness::decode_status()` or `dispatch_status()`.
- [ ] **Deduplicate `timeAgo()` function** (`web/src/`): Identical function in LogViewer, AlertsView, DiagnosticsView, SniffFramesView. Extract to `composables/useTimeAgo.ts` or `utils.ts`.
- [ ] **Deduplicate `parsePayload()` function** (`web/src/`): Identical function in AlertsView, DiagnosticsView, SniffFramesView. Extract shared utility.
- [ ] **Extract `<PendingDot>` component** (`web/src/`): Animate-ping dot markup duplicated in App.vue, ToggleSwitch, SelectControl, TemperatureCard, ControlsPanel.
- [ ] **Deduplicate xtask CLI argument parsing** (`xtask/src/*.rs`): Every module reimplements the same while-loop arg parser. Extract shared `Args` helper.
- [ ] **Deduplicate xtask serial port resolution** (`xtask/src/`): "CLI arg → config → fallback" pattern in monitor.rs, self_test.rs, provision.rs. Extract `resolve_port()`.

### MEDIUM — Bad Tests

- [ ] **Fix `if let` assertions that silently pass on wrong message type** (`launa-sim/src/spa_sim/tests/`): Many tests use `if let IncomingMessage::StatusUpdate(s) = msg { assert!() }` — wrong message type silently passes. Use `let ... else { panic!() }`.
- [ ] **Remove trivial smoke test `test_set_ambient_temp_method_exists`** (`launa-sim/src/spa_sim/tests/physics_tests.rs`): Calls setter and asserts nothing — only tests compilation.
- [ ] **Remove duplicate test `test_simbroker_subscription_filtering_empty_means_all`** (`launa-sim/src/sim_broker.rs`): Duplicates `test_simbroker_default_identical_to_current`.
- [ ] **Remove duplicate test `test_filter_cycle_start_does_not_toggle_running_pump`** (`launa-sim/src/spa_sim/tests/state_tests.rs`): Same behavior already tested by `test_spontaneous_event_does_not_double_toggle`.
- [ ] **Add unit tests for `launa-hal`** (`crates/launa-hal/src/`): `Timestamp` arithmetic (`elapsed_since`, `saturating_add`), `Clock` trait defaults — zero tests exist.
- [ ] **Add missing test coverage for protocol edge cases** (`launa-protocol`): Undefined pump state `3`, undefined heating mode `2`, config parser negative tests, CRC edge cases, unknown fault codes through `parse()`.
- [ ] **Add missing integration test scenarios** (`launa-integration-tests`): OTA disconnect mid-download, concurrent OTA + MQTT command, NVS corruption fallback, OTA Content-Length exceeding partition, WiFi disconnect during MQTT publish.

### MEDIUM — Bad UX

- [ ] **Pending indicator spins forever on offline device** (`web/src/composables/useMqtt.ts`): `pendingKeys` only cleared on state message arrival. If device offline, spinner never stops. Add 5-second auto-clear timeout.
- [ ] **MQTT publish silently drops commands when disconnected** (`web/src/composables/useMqtt.ts`): User clicks toggle, nothing happens, no feedback. Show toast/alert when command is dropped.
- [ ] **Connection error hidden on mobile** (`web/src/components/ConnectionBar.vue`): `hidden sm:block` hides error details on mobile screens. Users see no error information.
- [ ] **Settings modal has no input validation** (`web/src/components/SettingsModal.vue`): Empty broker URLs, invalid ports, special chars in device ID all accepted. Save always "succeeds" then silently fails to connect.
- [ ] **Tab bar has no new-content indicators** (`web/src/App.vue`): New alerts/logs arrive but all tabs look identical. Add badge counts.
- [ ] **Config fallback has unclear error on blank ESP32** (`app/src/config.rs`): Placeholder WiFi creds cause infinite reboot loop with generic "WiFi init failed". Add specific log: "FATAL: No valid config in NVS. Use `cargo xtask config-flash` to write configuration."
- [ ] **Self-test silently discards real spa frames** (`app/src/main.rs`): Frame drain with no log. If self-test enabled while spa connected, all communication dropped silently. Log warning on first discard.
- [ ] **OTA `?test=1` URL hack is undocumented and fragile** (`app/src/main.rs`): Query parameter triggers TCP connectivity test. Accidental inclusion silently skips OTA. Use dedicated MQTT topic.
- [ ] **MQTT error messages lack actionable context** (`app/src/mqtt_client.rs`): Generic `ConnectionFailed` doesn't distinguish DNS vs TCP vs auth vs CONNACK. Include host:port and error detail.

### LOW — Clean Code

- [ ] **Name magic numbers in `generate_status_frame`** (`launa-sim/src/spa_sim/frame_gen.rs`): 11-parameter function. Bundle into `StatusFrameContext` struct or make method on `SpaSim`.
- [ ] **Name MQTT packet type magic numbers** (`app/src/mqtt_client.rs` lines 418-458): Raw `3`, `4`, `9`, `12`, `13`, `14` should be named constants.
- [ ] **Name OTA/App magic numbers** (`app/`): `5_000_000` panic delay, `0x140000` partition size, `1024`/`512` buffer sizes, `4096`/`1024` OTA buffer sizes. Extract named constants.
- [ ] **Fix misleading `frame_jitter_ticks` naming** (`launa-sim/src/spa_sim/mod.rs`): Field is byte count, not tick count. Rename to `jitter_padding_bytes` or similar.
- [ ] **Remove unused `diagnostics` ref from `useMqtt.ts`** (`web/src/composables/useMqtt.ts`): Defined and returned but never consumed — DiagnosticsView uses `useApi.ts` instead.
- [ ] **Rename `alert_` to `alertMessage`** (`web/src/composables/useMqtt.ts`): Trailing underscore is a code smell.
- [ ] **Remove inline Python script from `config_flash.rs`** (`xtask/src/config_flash.rs`): 60+ line Python string embedded in Rust. Move to separate file or rewrite in Rust.
- [ ] **Use protocol types in `sniff_decode.rs`** (`xtask/src/sniff_decode.rs`): `describe_frame()` reimplements protocol parsing with raw byte offsets. Use parsers from `launa-protocol`.
- [ ] **Fix `SpaState` type index signature disabling type safety** (`web/src/types.ts`): `[key: string]: unknown` defeats TypeScript excess property checking.
- [ ] **`StatusDashboard.vue` always renders 6 pumps / 4 lights** (`web/src/components/StatusDashboard.vue`): Should respect `AccessoryConfig` like `ControlsPanel` does.
- [ ] **`unsafe` `block_on` in SimTransport tests** (`launa-sim/src/sim_transport.rs`): Reimplements future executor from raw parts. Use `futures::block_on` or shared utility.
- [ ] **Replace `unwrap()` with `expect()` in frame generators** (`launa-sim/src/spa_sim/frame_gen.rs`): `FrameEncoder::encode().unwrap()` in every generator — use `.expect("reason")` for better diagnostics.
- [ ] **Add error injection to `SimTransport`** (`launa-sim/src/sim_transport.rs`): Currently always returns `Ok`. Can't test transport error handling. Add `set_read_error()` like `MockTransport`.
- [ ] **`Network` trait may be unused dead abstraction** (`launa-hal/src/network.rs`): Only has mock implementations. Verify usage in `app/` and `launa-mqtt`, remove if unused.

- [x] **`frame_error_count: u32` uses saturating_add** (`launa-protocol/src/frame.rs`): Both increment sites now use `saturating_add(1)` instead of `+= 1` to prevent wrap on noisy buses.
- [x] **Status message: missing panel_locked, notification_type, settings_lock, M8 cycle time fields** (`launa-protocol/src/status.rs`): Additional fields at offsets 9/18/19/21/24 that other implementations parse. Low priority — advanced/niche features.
- [x] **Missing message types: Preferences (0x26), Setup Parameters (0x25)** (`launa-protocol/src/dispatcher.rs`): Standard Balboa message types not handled. Non-essential but may appear on real spas.
- [x] **`protocol.md` says Pump 6 at bits 6-7 but code correctly uses bits 2-3** (`docs/protocol.md`): Documentation inconsistency inherited from NorthernMan54 header comment. Code is correct, docs are wrong. Fixed: added explicit P1/P2 byte bit layout tables and corrected Configuration Response byte 6.
- [x] **Temperature=0 accepted as valid set-temperature wire value** (`launa-mqtt/command_parser.rs`): "0" means "no temp set" per protocol. Now rejected with `TemperatureOutOfRange` error.
- [x] **Status message: missing sensor_a_temp, sensor_b_temp, hold_timer_minutes fields** (`launa-protocol/src/status.rs`): Offset 7 dual-interpretation: sensor_a_temp (normal mode) vs hold_timer_minutes (hold mode). Offset 8: sensor_b_temp in A/B temps mode (payload[0]==0x14). All three fields are Option types, mutually exclusive per mode. Temperature values respect scale divisor (÷2 for Celsius).
- [x] **Pump timers for pumps 4-6 and simultaneous timer operation untested** (`launa-integration-tests`): Timer manager supports 6 pumps but only pump 1 is tested. Added 13 tests: individual pump 4/5/6 timers, expiry, cancel, restart, and 3-pump simultaneous operation.

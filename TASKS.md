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

- [ ] **Add hardware watchdog timer** (`app/src/main.rs`): Configure TIMG1 as independent WDT. Main loop pets it; if stalled, WDT resets the device. Essential for headless operation.
- [ ] **Fix main loop blocking on `frame_rx.receive().await`** (`app/src/main.rs`): Use `embassy_futures::select::select()` to multiplex UART frames, MQTT commands, and a periodic `Timer::after()` for ticks. Currently blocks indefinitely when spa is off — no OTA, no commands, no diagnostics.
- [x] **Cap MQTT `rx_buffer` at fixed size** (`app/src/mqtt_client.rs`): `rx_buffer: Vec<u8>` has no bound. Cap at 2 KiB; if exceeded without a complete packet, treat as protocol error and reconnect.
- [x] **Fix circ_pump/mister HA entities** (`launa-mqtt`): Changed from writable switches (with command topics) to read-only sensors — protocol doesn't support toggling these.
- [ ] **Add firmware integrity verification to OTA** (`launa-esp-ota`, `app/src/ota.rs`): No CRC/hash of written firmware. Accept expected hash in OTA request (e.g., HTTP header or MQTT payload field). Verify after all writes, before `finalize()`. Also validate ESP32 image header magic (`\xE9`) on first write.
- [x] **Fix JSON escaping in `status_to_json`** (`launa-mqtt/src/state.rs`): Added `escape_json_string()` helper escaping `\`, `"`, `\n`, `\r`, `\t`, and control chars U+0000-U+001F → `\uXXXX`.
- [ ] **Set up CI pipeline** (`.github/workflows`): At minimum: `cargo test` + `cargo check` + `cargo fmt --check` on PRs to main.

### HIGH — Should Fix Before Production

- [x] **`Frame::encode()` silent truncation on payloads >253 bytes** (`launa-protocol/src/frame.rs`): Returns `Err(FrameError::PayloadTooLarge(len))` when `2 + payload.len() > 255`. All callers updated.
- [x] **`FrameDecoder` miscounts all parse failures as CRC errors** (`launa-protocol/src/frame.rs`): Renamed `crc_error_count` → `frame_error_count` (field, methods, tests, callers).
- [x] **`ClientIdAssignment` defaults to 0 on missing byte** (`launa-protocol/src/dispatcher.rs`): Returns `IncomingMessage::Unknown` when ID byte is missing instead of silently assigning 0.
- [ ] **Panic on initial MQTT connect failure — no retry** (`app/src/main.rs`): Replace `panic!("MQTT connect failed")` with retry loop + exponential backoff, or at least `software_reset()`.
- [ ] **UnsafeCell socket buffer reuse — document safety argument** (`app/src/mqtt_client.rs`): Add formal SAFETY comment explaining single-task context, or use `MaybeUninit` pattern.
- [ ] **Discovery publishes ~20 JSON strings on heap — OOM burst risk** (`app/src/mqtt_client.rs`): Publish one at a time with heap checks between, or use pre-allocated buffers.
- [ ] **Sniffer mode allocates unbounded String per frame** (`app/src/main.rs`): Replace `format!("{:02X}")` collect with `write!()` into pre-allocated buffer.
- [ ] **No firmware size / Content-Length validation during OTA** (`app/src/ota.rs`): Parse `Content-Length` from HTTP headers, validate against partition size before `begin()`.
- [ ] **Light entity state boolean vs string mismatch** (`launa-mqtt`): `value_template` extracts boolean `true`, but `payload_on` is string `"true"`. Inconsistent — standardize.
- [ ] **`DiscoveryBuilder` uses crate version not firmware version** (`launa-mqtt`): Accept `sw_version` as builder parameter instead of `env!("CARGO_PKG_VERSION")`.
- [x] **`finalize()` missing empty-image check** (`launa-esp-ota`): Added `bytes_written == 0` guard in `finalize()` — refuses to set boot to empty partition.
- [ ] **Otadata both slots may share same 4 KiB sector** (`launa-esp-ota`): Verify slot 1 offset is in its own sector (0x11000, not 0x10020). Erasing one destroys the other on power loss.
- [ ] **Write offset tracking on unaligned chunks** (`launa-esp-ota`): `write_offset` increments by raw chunk length, but flash writes are word-aligned. Padding bytes overlap with next write.
- [ ] **Verify CRC32 matches ESP-IDF bootloader expectations** (`launa-esp-ota`): MPEG-2 variant may not match. Validate against ESP-IDF source or hardware test.
- [ ] **Transport trait is sync but production is async** (`launa-hal`): False abstraction — production uses `embedded_io_async`, tests use sync trait. Document or align.
- [ ] **Integration tests use SpaController not real SpaApp** (`launa-integration-tests`): Tests validate sim framework, not production logic. Rewrite to exercise `SpaApp` through sim pipeline.
- [ ] **No OTA / reconnection / stale-detection integration tests** (`launa-integration-tests`): Critical production paths untested at integration level.
- [ ] **`ota-flash` does not verify firmware version after update** (`xtask`): Device could roll back to factory and still appear online. Check reported version.
- [ ] **Command queue in SpaApp is unbounded** (`launa-core`): Add cap (e.g., 32 commands) and reject overflow. MQTT commands faster than Ready windows = unbounded growth.

### MEDIUM — Recommended

- [ ] **Add panic-reboot handler** (`app/`): `esp-backtrace` panic handler loops forever. Add custom handler that logs backtrace, waits, then `software_reset()`.
- [ ] **Add MQTT command rate limiting** (`app/src/mqtt_client.rs`): Max N commands per 10 seconds. Drop excess to protect spa bus.
- [ ] **Validate OTA URL scheme** (`app/src/mqtt_client.rs`): Require `http://`, reject arbitrary schemes.
- [ ] **Add firmware version to diagnostics JSON** (`app/src/main.rs`): Include `env!("CARGO_PKG_VERSION")` in periodic diagnostics payload.
- [ ] **Mask sensitive fields in hw-test logging** (`app/src/main.rs`): Don't log WiFi SSID/MQTT host in production.
- [ ] **`SpaApp::device_id` stored but never read** (`launa-core`): Dead code wasting heap. Expose getter or remove.
- [ ] **Missing entity_category "diagnostic" on alert/diagnostics sensors** (`launa-mqtt`): Should appear in HA diagnostics section, not alongside primary sensors.
- [ ] **Sim responses are static/hardcoded** (`launa-sim`): Fault log, filter cycles, information, config all return fixed data. Add configurability for testing edge cases.
- [ ] **SpaController ignores config/fault/filter/info responses** (`launa-sim`): Only handles StatusUpdate, Ready, NewClientQuery. Other responses discarded in integration tests.
- [ ] **Config validation gaps in xtask** (`xtask/src/config.rs`): No validation of `device.id` format/length, serial port existence, or MQTT port range.
- [ ] **xtask argument parsing panics on missing flag values** (`xtask/src/*.rs`): `--feature` as last arg = index out of bounds. Add bounds checks.
- [ ] **No firmware versioning mechanism** (cross-cutting): No build hash or version embedded in binary or reported via MQTT.
- [ ] **Cargo.lock gitignored at workspace root** (`.gitignore`): Workspace library builds not reproducible across machines.

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

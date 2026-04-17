# Architecture

How the launa system works at a high level.

## Workspace Crates

- **launa-protocol** — Balboa spa RS-485 protocol: frame encode/decode (CRC-8 + HDLC byte stuffing), message dispatch, status/config/fault/filter/info parsing, command encoding, registration state machine
- **launa-hal** — Hardware abstraction traits: Transport (async, using embedded_io_async), Clock, Network. Mock impls behind `std` feature. Transport trait is async: `async fn read()`, `async fn write()`, `async fn flush()`.
- **launa-mqtt** — MQTT topics, discovery builder (27 HA entities), command parser (allowlist), state JSON serializer, packet extraction (try_extract_packet, decode_remaining_length), OTA URL parser (`parse_ota_url()` extracted from app/ for desktop testability). no_std compatible. DiscoveryBuilder generates all HA auto-discovery configs with origin, sw_version, entity_category. JSON construction uses manual alloc::format! with helper functions, not serde. **Important coupling:** discovery entity `value_template` fields must match `status_to_json()` output — both must be updated together when adding new entities.
- **launa-ota** — OTA firmware update trait + MockOta. no_std, mock behind `mock` feature. MockOta session state machine: `not_started → in_progress → finalized/rolled_back`. begin() sets in_progress=true, successful finalize() sets in_progress=false, rollback_and_reboot() resets state. Failed operations leave state unchanged. MAX_FIRMWARE_SIZE=1_835_008 bytes (1.75 MiB). Failure injection fields: fail_on_begin, fail_on_write_after(N), fail_on_finalize. **Extraction targets:** HTTP parsing pure functions from app/ota.rs (parse_http_url, validate_http_status, find_header_end, parse_crc_from_url, parse_content_length, find_header_value_start, extract_status_line).
- **launa-esp-ota** — ESP32 OTA implementation using esp-storage directly. CRC-32/MPEG-2 (polynomial 0x04C11DB7, init 0xFFFFFFFF). Otadata read-modify-write for shared sectors. Write offset tracks word-aligned positions. **HMAC-SHA256 signing:** `SigningKey` and `verify_signature()` added for firmware integrity verification. SHA-256 and HMAC-SHA256 implemented from scratch (no_std compatible). **Limitation:** `firmware_data: Vec<u8>` accumulates the entire OTA image in RAM — will OOM on the 32 KiB ESP32 heap for real firmware sizes (~1.25 MiB). Streaming HMAC over flash reads needed for production. `verify_signature()` is NOT yet wired into the app OTA download flow (app still only checks CRC).
- **launa-sim** — Spa simulator (SpaSim/SpaState), SimBroker (functional mock MQTT broker with QoS 1 tracking, subscription matching, loss rate, disconnect/reconnect), SimTransport (virtual RS-485), VirtualClock. **Note: launa-sim is NOT `#![no_std]`** — it's a desktop-only crate using `std`. SpaSim supports configurable responses for fault/filter/info/config — use `set_fault_log_config()` to configure meaningful fault log entries before generating fault log responses. **Note:** SpaSim's generate_config_response() always produces 0x2E (ControlConfiguration), never 0x94 (ConfigurationResponse), so the ConfigurationResponse path cannot be end-to-end tested with the sim. **inject_corrupt_frame() gotcha:** corrupting the end marker byte (last byte) doesn't cause CRC errors because the frame decoder finds a valid frame *before* the corrupted marker. The method now corrupts a payload byte (index 5) instead. **Physics model:** SpaSim has a realistic thermal model where heating rate is proportional to temp delta (~0.5°F/tick tapering), cooling rate ~0.1°F/tick toward ambient (configurable via `set_ambient_temp()`, default 70°F). Features heater/pump interlock (is_heating only when pump running, resumes automatically on pump restart), configurable heater overshoot past set_temp with hysteresis, unknown temp on startup (first N ticks report 0xFF), pump waste heat contribution (`set_pump_heat_contribution()`, default 0/off), and deterministic sensor noise. **Dual-noise mechanism:** legacy `simulate_sensor_noise(jitter)` and physics-model `set_physics_noise_amplitude(amp)` are separate and stack additively — both can be active simultaneously.
- **launa-core** — SpaApp: extracted app logic (registration, command tracking, pump/hold timers, stale detection, diagnostics, alerting). Pure synchronous, no IO. CommandTracker correctly handles both Fahrenheit and Celsius temperature confirmation by comparing raw wire values. Stale probe uses lightweight command (not ConfigurationRequest). **Extraction targets:** RateLimiter (from app/mqtt_client.rs, swap embassy_time::Instant for Clock trait), RemoteLogBuffer (from app/remote_log.rs, abstract Instant dependency).
- **launa-integration-tests** — Integration tests using SpaApp + SpaSim + VirtualClock. Tests exercise SpaApp through sim pipeline. Contains SimHttpServer helper for OTA tests (serves firmware in configurable chunks over TCP). **SpaAppTestHarness** wires SpaSim → FrameDecoder → SpaApp → SimBroker with VirtualClock for E2E tests. Key methods: `tick_spa()` (sim→app pipeline), `tick_app()` (periodic checks), `send_command()` (MQTT→app), `complete_registration()`, `advance_ms()`.
- **xtask** — Cargo xtask tooling (flash, monitor, spa-sim, OTA, sniffer). Config validation includes device.id format, serial port existence, MQTT port range. Argument parsing has bounds checks.

## app/ Crate

- ESP32 firmware binary using esp-hal 1.0 + embassy + esp-radio + esp-nvs + esp-storage
- 32 KiB heap via esp_alloc — all allocations must be bounded
- MqttClient uses DiscoveryBuilder from launa-mqtt for HA auto-discovery (27 entities published sequentially with retain=true)
- Firmware version (FIRMWARE_VERSION const from env!("CARGO_PKG_VERSION")) embedded in discovery, state, and diagnostics
- Panic handler logs + waits 500ms + software_reset() instead of infinite loop
- MQTT command rate limiting protects spa bus (10 commands per 10-second window, pump timers exempted)
- OTA URL validation requires http:// scheme
- Custom panic handler: esp-backtrace must use `println` feature only (NOT `panic-handler`) to avoid conflicting with the custom `#[panic_handler]` in main.rs
- OTA state is managed in the app/ main loop, NOT in SpaApp — SpaApp doesn't own OTA state. Integration tests exercise MockOta directly for OTA rollback scenarios.

## Key Data Flow

```
RS-485 bytes → FrameDecoder → Frame → SpaApp::process_frame() → Vec<AppAction>
MQTT command → SpaApp::on_mqtt_command() → queued
Periodic     → SpaApp::tick() → Vec<AppAction>

AppAction::SendFrame(bytes)     → UART TX
AppAction::PublishState(status) → MQTT publish
AppAction::PublishAlert/Diag    → MQTT publish
```

## Invariants

- All workspace crates except `launa-sim` are `#![no_std]` with `extern crate alloc`; `launa-sim` is a desktop-only `std` crate
- All tests run on desktop via `cargo test --workspace`; app/ verified with `cargo +esp check`
- SpaSim defaults produce deterministic output (new features default to off)
- CommandTracker bounded at MAX_PENDING_COMMANDS=8; command queue capped at 32
- Temperature validation: hard upper limit 108°F / 42°C
- FrameDecoder: CRC-8 + HDLC byte stuffing for 0x7E/0x7D, configurable max_buffer_size (default 512) with overflow protection
- ESP32 heap: 32 KiB — all allocations must be bounded, avoid Vec growth
- All unsafe blocks must have SAFETY comments
- StatusUpdate::parse() requires minimum 24-byte payload; all fields up to offset 23 are safely accessible within this guard
- Remote-log feature (optional, behind `remote-log` feature flag in app/) provides MQTT log forwarding with a ring buffer; tests use a simplified safe mirror, not the actual unsafe RemoteLogBuffer implementation

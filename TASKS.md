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
- [ ] **OTA real implementation**: Replace stub with actual `esp-idf-svc` OTA partition management.

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

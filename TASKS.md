# Launa - Task Tracker

## Critical Bugs

- [ ] **Fix command encoding** (`launa-protocol/src/command.rs`): Every `Command::encode()` variant is missing the protocol sub-type byte (0x04, 0x11, 0x20, 0x21, 0x22, 0x27, 0x07). The message type is `0A BF` for all outgoing messages, but the sub-type discriminator that tells the spa WHICH command it is needs to be the first byte of the payload. Currently `ToggleItem` sends `[item.code(), 0x00]` which is correct, but `ConfigurationRequest` sends an empty payload — it should send `[0x04]`. Similarly `SetTemperature` sends `[temp]` but should send `[0x20, temp]`. Verify against protocol doc and fix all variants.
- [ ] **Fix heating_mode offset** (`launa-protocol/src/status.rs`): `heating_mode` reads from `payload[5]` (Hold Mode flags) but the protocol doc says Heating Mode is at offset 7. Cross-reference with the real protocol and fix.
- [ ] **Add mister status parsing** (`launa-protocol/src/status.rs`): `payload[12]` bit 0 contains mister status, not currently parsed.

## Protocol Parser

- [ ] **Parse information response** (`0A BF 24`): Extract software ID, system model, heater voltage/type, DIP switch settings into a struct.
- [ ] **Parse fault log response** (`0A BF 28`): Extract fault count, entry number, message code, timestamps, temperatures.
- [ ] **Parse filter cycles response** (`0A BF 23`): Extract filter 1/2 start times and durations.
- [ ] **Disambiguate `0A BF` messages**: The `0A BF` type is shared by many message types. Need a dispatcher that looks at the first payload byte to determine the sub-type (0x04=config request, 0x11=toggle, 0x20=set temp, 0x22=settings, 0x23=filter cycles, 0x24=info, 0x28=fault, 0x2E=control config, 0x27=temp scale, 0x94=config response).

## MQTT / Home Assistant

- [ ] **State serialization**: Convert `StatusUpdate` to JSON matching the HA value_templates in discovery (current_temp, set_temp, is_heating, pump1_on, pump2_on, pump3_on, light1, blower, circ_pump).
- [ ] **Command parsing**: Parse incoming MQTT command messages and convert to `Command` variants (toggle pump N, set temperature, etc.).
- [ ] **Birth/last-will messages**: Publish `online`/`offline` to availability topic. Subscribe to `homeassistant/status` and re-publish discovery when HA restarts.

## HAL / Desktop Testing

- [ ] **Integration test**: Mock transport → feed real Balboa frames → verify parsed `StatusUpdate` → verify JSON serialization → verify MQTT topic construction.
- [ ] **Frame decoder test with real data**: Add test using actual captured Balboa spa frames (hex strings from protocol docs).

## ESP32 Firmware (`app/`)

- [ ] **UART/RS-485 transport**: Implement `launa_hal::Transport` using `esp-idf-hal` UART with direction pin control for the RS-485 transceiver.
- [ ] **WiFi connectivity**: Implement `launa_hal::Network` using `esp-idf-svc` WiFi + TCP stack. Add config for SSID/password.
- [ ] **MQTT client**: Use `esp-idf-svc` built-in MQTT client to connect to broker, publish discovery, subscribe to command topics.
- [ ] **Main event loop**: Wire together UART read → frame decode → status parse → MQTT publish. Handle incoming MQTT commands → encode to Balboa frames → UART write.
- [ ] **OTA integration**: Implement `launa_ota::OtaUpdate` using `esp-ota` or `esp-idf-svc` OTA APIs. Add partition table for dual-OTA slots.
- [ ] **Configuration storage**: Store WiFi credentials, MQTT broker address, device ID in NVS (non-volatile storage). Consider a simple config mechanism (compile-time env vars or runtime NVS).

## Done

- [x] Project structure and workspace setup
- [x] Git repo initialized and pushed to GitHub
- [x] Balboa CRC-8 implementation with tests
- [x] Frame encode/decode with streaming decoder
- [x] Status update parser (temperature, pumps, lights, heating)
- [x] Command builder (toggle, set temp, set time, settings requests)
- [x] Spa configuration parser (pump/light/blower/circ capabilities)
- [x] Client ID registration state machine
- [x] Hardware abstraction traits with mock implementations
- [x] Home Assistant MQTT auto-discovery builder (8 entities)
- [x] OTA update trait with mock
- [x] ESP32 app skeleton
- [x] Protocol documentation, BP6013G1 notes, architecture docs
- [x] All 14 unit tests passing

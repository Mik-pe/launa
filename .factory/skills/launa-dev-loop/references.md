# Launa Dev Loop -- References

## Protocol References

### Canonical (in-tree)
- `docs/protocol.md` -- Balboa spa control protocol reference (byte offsets, message types, CRC)
- `docs/bp6013g1.md` -- BP6013G1 controller hardware specs, RS-485 wiring, pin reference
- `docs/architecture.md` -- Crate structure, data flow, desktop testing strategy

### External
- **Balboa Worldwide App Protocol**: https://github.com/ccutrer/balboa_worldwide_app/blob/main/doc/protocol.md
- **ESP8266 Spa Implementation** (Arduino/C++): https://github.com/cribskip/esp8266_spa
- **ESP32 Balboa Spa** (Rust): https://github.com/jasta/esp32-balboa-spa
- **Home Assistant MQTT Discovery**: https://www.home-assistant.io/integrations/mqtt/#discovery-messages

## Key Code Paths

### Protocol Parsing
| Concern | File |
|---------|------|
| CRC-8 computation | `crates/launa-protocol/src/crc8.rs` |
| Frame encode/decode | `crates/launa-protocol/src/frame.rs` |
| Status update parser | `crates/launa-protocol/src/status.rs` |
| Command builder | `crates/launa-protocol/src/command.rs` |
| Configuration parser | `crates/launa-protocol/src/config.rs` |
| Registration state machine | `crates/launa-protocol/src/registration.rs` |
| Message dispatcher | `crates/launa-protocol/src/dispatcher.rs` |
| Information response | `crates/launa-protocol/src/information.rs` |
| Fault log parser | `crates/launa-protocol/src/fault.rs` |
| Filter cycles parser | `crates/launa-protocol/src/filter.rs` |
| Message type enum | `crates/launa-protocol/src/message.rs` |

### Hardware Abstraction
| Concern | File |
|---------|------|
| Transport trait | `crates/launa-hal/src/transport.rs` |
| Network trait | `crates/launa-hal/src/network.rs` |
| HAL module root | `crates/launa-hal/src/lib.rs` |

### MQTT / Home Assistant
| Concern | File |
|---------|------|
| Discovery builder | `crates/launa-mqtt/src/discovery.rs` |
| State serialization | `crates/launa-mqtt/src/state.rs` |
| Command parser | `crates/launa-mqtt/src/command_parser.rs` |
| Topic builder | `crates/launa-mqtt/src/topics.rs` |

### Testing
| Concern | File |
|---------|------|
| Spa simulator | `crates/launa-integration-tests/src/spa_simulator.rs` |
| Property tests | `crates/launa-protocol/tests/property_tests.rs` |
| Fuzz tests | `crates/launa-protocol/tests/fuzz_tests.rs` |
| HAL tests | `crates/launa-hal/tests/hal_tests.rs` |

### ESP32 Firmware
| Concern | File |
|---------|------|
| Main entry | `app/src/main.rs` |
| App Cargo.toml | `app/Cargo.toml` |
| Build script | `app/build.rs` |

## Message Type Quick Reference

| Type | Bytes | Description |
|------|-------|-------------|
| Status Update | `FF AF 13` | Sent every ~1 second |
| Ready Indicator | `10 BF 06` | RS-485 only, safe-to-send |
| Registration Query | `FE BF 00` | Spa asks for new clients |
| Client ID Request | `FE BF 01` | Client responds |
| Client ID Assign | `FE BF 02` | Spa assigns ID |
| Client ID Ack | `<ID> BF 03` | Client acknowledges |
| Config Request | `0A BF 04` | Outgoing |
| Toggle Item | `0A BF 11` | Outgoing, payload: `II 00` |
| Set Temperature | `0A BF 20` | Outgoing, payload: `TT` |
| Set Time | `0A BF 21` | Outgoing, payload: `HH MM` |
| Settings Request | `0A BF 22` | Outgoing, sub-type varies |
| Filter Cycles | `0A BF 23` | Response |
| Information | `0A BF 24` | Response |
| Temp Scale | `0A BF 27` | Outgoing |
| Fault Log | `0A BF 28` | Response |
| Control Config | `0A BF 2E` | Response |
| Nothing to Send | `<ID> BF 07` | No-op ack |
| Config Response | `0A BF 94` | Response |

# Task Worker -- Codebase References

## Workspace Crate Map

| Crate | Path | Description | Key Files |
|-------|------|-------------|-----------|
| launa-protocol | `crates/launa-protocol/` | Balboa protocol parser (no_std) | `src/status.rs`, `src/command.rs`, `src/config.rs`, `src/frame.rs`, `src/registration.rs`, `src/dispatcher.rs`, `src/information.rs`, `src/fault.rs`, `src/filter.rs`, `src/crc8.rs`, `src/message.rs` |
| launa-hal | `crates/launa-hal/` | Hardware abstraction traits + mocks | `src/transport.rs`, `src/network.rs`, `src/lib.rs` |
| launa-mqtt | `crates/launa-mqtt/` | MQTT + HA auto-discovery | `src/discovery.rs`, `src/state.rs`, `src/command_parser.rs`, `src/topics.rs` |
| launa-ota | `crates/launa-ota/` | OTA update trait + mock | `src/lib.rs` |
| launa-esp-ota | `crates/launa-esp-ota/` | ESP32 OTA via esp-storage | `src/lib.rs` |
| launa-sim | `crates/launa-sim/` | Spa simulator | `src/spa_sim.rs`, `src/sim_broker.rs`, `src/controller.rs` |
| launa-integration-tests | `crates/launa-integration-tests/` | Integration tests | `src/lib.rs`, `src/spa_simulator.rs`, `tests/sim_tests.rs` |
| xtask | `xtask/` | Cargo xtask tooling | `src/main.rs` |
| app | `app/` | ESP32 firmware (NOT in workspace) | `src/main.rs`, `src/transport.rs`, `src/wifi.rs`, `src/mqtt_client.rs`, `src/ota.rs`, `src/config.rs`, `src/command_tracker.rs`, `src/pump_timer.rs`, `src/heap_monitor.rs` |

## Build Commands

| Scope | Command | Notes |
|-------|---------|-------|
| Workspace typecheck | `cargo check` | From `C:\dev\launa` |
| Workspace tests | `cargo test` | From `C:\dev\launa` |
| Single crate tests | `cargo test -p <crate>` | e.g., `cargo test -p launa-protocol` |
| ESP32 firmware check | `cd C:\dev\launa\app && cargo +esp check` | Uses `xtensa-esp32-none-elf` target, `esp` toolchain |
| ESP32 firmware build | `cd C:\dev\launa\app && cargo +esp build` | Full firmware build (slow, use `check` for verification) |
| ESP32 flash | `cd C:\dev\launa\app && cargo espflash flash --chip esp32 --monitor` | Requires physical ESP32 connected via USB |
| Format | `cargo fmt` | From `C:\dev\launa` (formats workspace + app) |

### ESP32 Build Details

- **Toolchain**: `esp` (installed via rustup, provides `xtensa-esp32-none-elf` target)
- **Config**: `app/.cargo/config.toml` sets `target = "xtensa-esp32-none-elf"` and `build-std = ["core", "alloc"]`
- **No std**: `app/` is fully `no_std` — uses `esp-alloc` for heap, `embassy` for async
- **Verification**: `cargo +esp check` is the fast path for verifying `app/` compiles. Use `cargo +esp build` only when producing a flashable binary.
- **Known issue**: `esp-backtrace` version must use `println` feature (not `print-uart` which was removed in 0.16+)

## Task Category -> Typical Files

### Protocol parsing tasks
- `crates/launa-protocol/src/status.rs` -- status update parser
- `crates/launa-protocol/src/command.rs` -- command builder
- `crates/launa-protocol/src/config.rs` -- configuration parser
- `crates/launa-protocol/src/frame.rs` -- frame encode/decode
- `crates/launa-protocol/src/registration.rs` -- registration state machine
- Tests: `crates/launa-protocol/tests/property_tests.rs`, `crates/launa-protocol/tests/fuzz_tests.rs`

### MQTT / Home Assistant tasks
- `crates/launa-mqtt/src/discovery.rs` -- HA auto-discovery
- `crates/launa-mqtt/src/state.rs` -- status-to-JSON serialization
- `crates/launa-mqtt/src/command_parser.rs` -- MQTT command parsing
- `crates/launa-mqtt/src/topics.rs` -- topic builder

### Simulator / test tasks
- `crates/launa-sim/src/spa_sim.rs` -- SpaSim simulator
- `crates/launa-integration-tests/src/spa_simulator.rs` -- SpaSimulator (older)
- `crates/launa-integration-tests/src/lib.rs` -- integration tests

### ESP32 firmware tasks (requires `app/` but may have workspace deps)
- `app/src/main.rs` -- main firmware entry
- `app/src/mqtt_client.rs` -- hand-rolled MQTT client
- `app/src/command_tracker.rs` -- command ACK verification
- `app/src/pump_timer.rs` -- pump timer manager

## Task Priority Sections in TASKS.md

Tasks appear in priority order:

1. **P0: Production Blockers** -- OTA, NVS, discovery, connectivity, alerting
2. **P1: MQTT/HA Correctness** -- discovery unification, payload format
3. **P2: Code Quality** -- Clock trait, simulator consolidation, cleanup
4. **P2: Missing Features** -- sniffer, hw-test, pump timers
5. **Phase N: Hardware Testing** -- skip (requires physical hardware)

## Dependency Graph (for cross-crate tasks)

```
launa-protocol  <-- launa-hal
     |               |
     v               v
launa-mqtt      launa-sim
     |               |
     v               v
     +--> launa-integration-tests <--+
```

When a task changes types in `launa-protocol`, downstream crates may need updates
in this order: `launa-hal` -> `launa-mqtt` / `launa-sim` -> `launa-integration-tests`.

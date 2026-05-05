# ESP32 Device Mapping

Device | USB Serial | MAC Address        | Flash | RS-485    | Notes
-------|------------|--------------------|-------|-----------|---------------------
A      | 0001       | 1c:c3:ab:ba:83:c8 | 4MB   | TX+RX OK | Main app firmware
B      | 0001       | 1c:c3:ab:bc:12:bc | 4MB   | TX+RX OK | Spa emulator

> Both devices share the same USB serial number (`0001`, CP2102 adapters).
> Use `cargo xtask list-ports` to see current port assignments, then
> `--port-index <N>` or `--port <device>` to target a specific unit.

## xtask Commands

```
# Discover current port assignments
cargo xtask list-ports

# Flash (use --port-index to disambiguate when both are plugged in)
cargo xtask flash --port-index 1
cargo xtask config-flash --port-index 1
cargo xtask spa-emulator-flash --port-index 2

# Monitor
cargo xtask monitor --port-index 1 --duration 10
cargo xtask monitor --port-index 2 --duration 10
```

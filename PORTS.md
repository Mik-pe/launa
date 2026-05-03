# ESP32 Port Mapping

Device | Port                  | MAC Address        | Flash | RS-485    | Notes
-------|-----------------------|--------------------|-------|-----------|---------------------
A      | /dev/cu.usbserial-2   | 1c:c3:ab:ba:83:c8 | 4MB   | TX+RX OK | Main app firmware
B      | /dev/cu.usbserial-0001| 1c:c3:ab:bc:12:bc | 4MB   | TX+RX OK | Spa emulator

## xtask Commands

```
# Flash device A
cargo xtask flash --port /dev/cu.usbserial-2
cargo xtask config-flash --port /dev/cu.usbserial-2

# Flash device B
cargo xtask spa-emulator-flash --port /dev/cu.usbserial-0001

# Monitor
cargo xtask monitor --port /dev/cu.usbserial-2 --duration 10
cargo xtask monitor --port /dev/cu.usbserial-0001 --duration 10
```

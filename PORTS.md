# ESP32 Port Mapping

Device | Port                  | MAC Address        | Flash | RS-485    | Notes
-------|-----------------------|--------------------|-------|-----------|---------------------
A      | /dev/cu.usbserial-3   | 1c:c3:ab:ba:83:c8 | 4MB   | TX+RX OK |
B      | /dev/cu.usbserial-2   | 1c:c3:ab:bc:2d:bc | 4MB   | TX+RX OK |

## xtask Commands

```
# Flash device A
cargo xtask rs485-debugger-flash --port /dev/cu.usbserial-3

# Flash device B
cargo xtask rs485-debugger-flash --port /dev/cu.usbserial-2

# Monitor
cargo xtask monitor --port /dev/cu.usbserial-3 --duration 10
cargo xtask monitor --port /dev/cu.usbserial-2 --duration 10
```

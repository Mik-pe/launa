"""Config flash utility for Launa ESP32.

Reads a config payload file and sends it over serial to an ESP32
waiting for configuration via the serial config protocol.

Usage: python scripts/config_flash.py <port_name> <config_file>
"""

import serial
import time
import sys


def main():
    if len(sys.argv) < 3:
        print("Usage: config_flash.py <port_name> <config_file>", file=sys.stderr)
        sys.exit(1)

    port_name = sys.argv[1]
    config_file = sys.argv[2]

    with open(config_file, "r") as f:
        config_lines = f.read()

    print(f"Opening {port_name}...", file=sys.stderr)
    port = serial.Serial(port_name, 115200, timeout=1)

    # Toggle DTR to reset the ESP32, then catch the config window on boot
    print("Resetting ESP32 via DTR...", file=sys.stderr)
    port.dtr = False
    time.sleep(0.1)
    port.dtr = True
    time.sleep(0.1)
    port.dtr = False

    # Wait for ESP32 ready signal after reboot
    start = time.time()
    ready = False
    all_output = ""
    while time.time() - start < 35:
        data = port.read(4096)
        if data:
            text = data.decode("utf-8", errors="replace")
            all_output += text
            sys.stderr.write(text)
            sys.stderr.flush()
            if "Waiting for serial config" in all_output:
                ready = True
                break

    if not ready:
        print(
            f"ERROR: ESP32 not ready within 35s. Output so far: {all_output}",
            file=sys.stderr,
        )
        port.close()
        sys.exit(1)

    print("ESP32 ready, sending config...", file=sys.stderr)
    time.sleep(0.1)

    # Send each config line with CRLF line endings
    for line in config_lines.strip().split("\n"):
        port.write((line + "\r\n").encode("utf-8"))
        print(f"  Sent: {line}", file=sys.stderr)
    port.flush()

    # Wait for response
    start = time.time()
    response = b""
    while time.time() - start < 10:
        data = port.read(4096)
        if data:
            response += data
            sys.stderr.write(data.decode("utf-8", errors="replace"))
            sys.stderr.flush()
            if b"CONFIG_OK" in response or b"CONFIG_ERROR" in response:
                break

    port.close()

    if b"CONFIG_OK" in response:
        print("CONFIG_OK")
    elif b"CONFIG_ERROR" in response:
        idx = response.find(b"CONFIG_ERROR:")
        error = response[idx + len(b"CONFIG_ERROR:") :].decode("utf-8", errors="replace").strip()
        print(f"CONFIG_ERROR: {error}")
    else:
        decoded = response.decode("utf-8", errors="replace").strip()
        print(f"NO_RESPONSE: {decoded}")


if __name__ == "__main__":
    main()

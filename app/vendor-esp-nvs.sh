#!/bin/bash
# Set up the vendored esp-nvs dependency for the ESP32 app.
#
# esp-nvs v0.4 pins esp-hal = "1.0.0" and esp-storage = "0.8.1", but the
# project uses esp-hal 1.1.0-rc.0 and esp-storage 0.9. This script clones
# esp-nvs from GitHub and patches the version constraints.
#
# Run from project root: ./app/vendor-esp-nvs.sh
set -euo pipefail

VENDOR_DIR="app/vendor/esp-nvs-vendor/esp-nvs"

if [ -f "$VENDOR_DIR/Cargo.toml" ]; then
    echo "esp-nvs already vendored at $VENDOR_DIR"
    exit 0
fi

echo "Fetching esp-nvs v0.4.0 from GitHub..."
mkdir -p "$VENDOR_DIR"
git clone --depth 1 --branch v0.4.0 https://github.com/lhemala/esp-nvs.git /tmp/esp-nvs-clone
cp -r /tmp/esp-nvs-clone/esp-nvs/* "$VENDOR_DIR/"
rm -rf /tmp/esp-nvs-clone

# Patch version constraints for esp-hal and esp-storage compatibility
sed -i.bak 's/version = "0.8.1"/version = "0.9"/' "$VENDOR_DIR/Cargo.toml"
sed -i.bak 's/version = "1.0.0"/version = "=1.1.0-rc.0"/' "$VENDOR_DIR/Cargo.toml"
rm -f "$VENDOR_DIR/Cargo.toml.bak"

echo "Done. esp-nvs vendored with patched deps at $VENDOR_DIR"

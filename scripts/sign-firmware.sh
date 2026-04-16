#!/usr/bin/env bash
# sign-firmware.sh — Generate HMAC-SHA256 signature for an ESP32 firmware binary.
#
# Usage:
#   ./scripts/sign-firmware.sh <firmware.bin> [key_hex]
#
# The signing key is 32 bytes in hex (64 hex characters).
# If not provided, the default development key is used.
#
# Output:
#   Prints the truncated (first 4 bytes) signature as 8 hex characters.
#   Also creates <firmware.bin>.sig containing the full 32-byte hex signature.
#
# The firmware binary can include the signature by appending ?sig=XXXXXXXX
# to the OTA URL, or the signature can be embedded in the firmware metadata.
#
# Example:
#   ./scripts/sign-firmware.sh target/launa-app.bin
#   ./scripts/sign-firmware.sh target/launa-app.bin 0123456789ABCDEF...

set -euo pipefail

if [ $# -lt 1 ]; then
    echo "Usage: $0 <firmware.bin> [key_hex]" >&2
    echo "  key_hex: 64 hex chars (32 bytes). Default: built-in dev key." >&2
    exit 1
fi

FIRMWARE="$1"

if [ ! -f "$FIRMWARE" ]; then
    echo "Error: firmware file not found: $FIRMWARE" >&2
    exit 1
fi

# Default development signing key (matches EspOtaFlash::default_signing_key())
# MUST match the key in crates/launa-esp-ota/src/lib.rs
DEFAULT_KEY="0123456789ABCDEF FEDCBA9876543210 0123456789ABCDEF FEDCBA9876543210"
KEY_HEX="${2:-$(echo "$DEFAULT_KEY" | tr -d ' ')}"

# Validate key length
if [ ${#KEY_HEX} -ne 64 ]; then
    echo "Error: key must be exactly 64 hex characters (32 bytes), got ${#KEY_HEX}" >&2
    exit 1
fi

# Convert hex key to binary
KEY_BIN=$(echo "$KEY_HEX" | xxd -r -p)

# Compute HMAC-SHA256
# -nosalt and -bin ensure raw binary output
HMAC_HEX=$(openssl dgst -sha256 -mac HMAC -macopt hexkey:"$KEY_HEX" -hex "$FIRMWARE" | awk '{print $NF}')

if [ -z "$HMAC_HEX" ]; then
    echo "Error: failed to compute HMAC-SHA256" >&2
    exit 1
fi

# Full signature file
SIG_FILE="${FIRMWARE}.sig"
echo "$HMAC_HEX" > "$SIG_FILE"

# Truncated signature (first 4 bytes = 8 hex chars, matching truncate_signature())
TRUNCATED_SIG="${HMAC_HEX:0:8}"

echo "Firmware: $FIRMWARE"
echo "Full HMAC-SHA256: $HMAC_HEX"
echo "Truncated signature (u32): $TRUNCATED_SIG"
echo "Signature written to: $SIG_FILE"
echo ""
echo "To verify during OTA, include ?sig=0x${TRUNCATED_SIG} in the OTA URL."

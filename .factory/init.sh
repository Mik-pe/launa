#!/bin/bash
# Mission init script — idempotent
set -e
echo "Verifying Rust toolchain..."
cargo --version
echo "Running workspace check..."
cargo check --workspace
echo "Init complete."

#!/bin/bash
set -euo pipefail

PI_HOST="${PI_HOST:-mikpe@launa-server.local}"
PI_DIR="${PI_DIR:-/opt/launa}"
IMAGE_NAME="${IMAGE_NAME:-launa-builder}"

echo "=== Launa Server Deploy to RPi ==="
echo "Target: $PI_HOST:$PI_DIR"
echo ""

# Ensure the build environment image exists
if ! docker image inspect "$IMAGE_NAME" > /dev/null 2>&1; then
  echo "[0/5] Building builder image (first time only)..."
  docker build --platform linux/amd64 -t "$IMAGE_NAME" .
fi

# Step 1: Build web assets
echo "[1/5] Building web assets..."
docker run --rm --platform linux/amd64 \
  -v "$(pwd)/web:/project/web" \
  "$IMAGE_NAME" \
  bash -c "cd /project/web && rm -rf dist && bun install --frozen-lockfile && bun run build"

# Step 2: Build Rust binary (cross-compile for aarch64)
echo "[2/5] Cross-compiling launa-server for aarch64..."
docker run --rm --platform linux/amd64 \
  -v "$(pwd):/project:ro" \
  -v launa-cargo-registry:/usr/local/cargo/registry \
  -v launa-cargo-target:/project/target \
  "$IMAGE_NAME" \
  bash -c "cargo build --release --target aarch64-unknown-linux-gnu -p launa-server"

# Step 3: Extract deploy artifacts from the target volume
echo "[3/5] Preparing deploy artifacts..."
rm -rf /tmp/launa-deploy /tmp/launa-deploy-web
mkdir -p /tmp/launa-deploy /tmp/launa-deploy-web
docker run --rm --platform linux/amd64 \
  -v launa-cargo-target:/project/target \
  -v /tmp/launa-deploy:/output \
  "$IMAGE_NAME" \
  bash -c "cp /project/target/aarch64-unknown-linux-gnu/release/launa-server /output/launa-server"
cp -r web/dist/* /tmp/launa-deploy-web/

echo "  Binary: $(du -h /tmp/launa-deploy/launa-server | cut -f1)"
echo "  Web:    $(du -sh /tmp/launa-deploy-web | cut -f1)"

# Step 4: Deploy to Pi
echo "[4/5] Deploying to $PI_HOST..."
ssh -t "$PI_HOST" "sudo systemctl stop launa-server 2>/dev/null; sudo mkdir -p $PI_DIR/web/dist $PI_DIR/data && sudo chown -R \$(whoami):\$(id -gn) $PI_DIR"
scp /tmp/launa-deploy/launa-server "$PI_HOST:$PI_DIR/launa-server"
ssh "$PI_HOST" "rm -rf $PI_DIR/web/dist"
ssh "$PI_HOST" "mkdir -p $PI_DIR/web/dist"
scp -r /tmp/launa-deploy-web/* "$PI_HOST:$PI_DIR/web/dist/"
ssh "$PI_HOST" "chmod +x $PI_DIR/launa-server"

# Step 5: Install systemd service
echo "[5/5] Installing systemd service..."
ssh -t "$PI_HOST" "sudo tee /etc/systemd/system/launa-server.service > /dev/null" << 'SERVICE'
[Unit]
Description=Launa MQTT Broker + Web UI
After=network.target

[Service]
Type=simple
User=mikpe
WorkingDirectory=/opt/launa
AmbientCapabilities=CAP_NET_BIND_SERVICE
ExecStart=/opt/launa/launa-server --web-dir /opt/launa/web --db-path /opt/launa/data/launa.db --http-port 80
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
SERVICE

ssh -t "$PI_HOST" "sudo systemctl daemon-reload && sudo systemctl enable launa-server && sudo systemctl restart launa-server"

echo ""
echo "=== Deploy complete ==="
echo "  MQTT:  $PI_HOST:1883"
echo "  WS:    $PI_HOST:9001"
echo "  Web:   http://$PI_HOST"
echo ""
echo "Useful commands:"
echo "  ssh $PI_HOST 'sudo systemctl status launa-server'"
echo "  ssh $PI_HOST 'sudo journalctl -u launa-server -f'"

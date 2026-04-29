#!/bin/bash
# Deploy launa-server to a remote Linux ARM64 host (e.g. RPi Zero 2W).
#
# Prerequisites:
#   - Docker Desktop with Rosetta enabled (Settings > General)
#   - SSH key auth to the target host (ssh-copy-id user@host)
#   - sudo on the target host
#
# Usage:
#   cp deploy.example.sh deploy.sh
#   # Edit the variables below
#   ./deploy.sh

set -euo pipefail

# -- Configure these for your setup --
PI_HOST="${PI_HOST:-user@your-pi-hostname.local}"   # SSH target (user@host)
PI_USER="user"                                       # Username on the Pi (for systemd unit)
PI_DIR="${PI_DIR:-/opt/launa}"                       # Install directory on the Pi
IMAGE_NAME="${IMAGE_NAME:-launa-server:latest}"

echo "=== Launa Server Deploy ==="
echo "Target: $PI_HOST:$PI_DIR"
echo ""

# Step 1: Build Docker image (cross-compiles for aarch64)
echo "[1/4] Building Docker image..."
docker build --platform linux/amd64 -t "$IMAGE_NAME" .

# Step 2: Extract binary and web assets from the image
echo "[2/4] Extracting binary and web assets from image..."
CONTAINER=$(docker create "$IMAGE_NAME")
rm -rf /tmp/launa-deploy
mkdir -p /tmp/launa-deploy
docker cp "$CONTAINER:/usr/local/bin/launa-server" /tmp/launa-deploy/launa-server
docker cp "$CONTAINER:/var/lib/launa/web" /tmp/launa-deploy/web
docker rm "$CONTAINER" > /dev/null

echo "  Binary: $(du -h /tmp/launa-deploy/launa-server | cut -f1)"
echo "  Web:    $(du -sh /tmp/launa-deploy/web | cut -f1)"

# Step 3: Deploy to target host
echo "[3/4] Deploying to $PI_HOST..."
ssh -t "$PI_HOST" "sudo systemctl stop launa-server 2>/dev/null; sudo mkdir -p $PI_DIR/web $PI_DIR/data && sudo chown -R \$(whoami):\$(id -gn) $PI_DIR"
scp /tmp/launa-deploy/launa-server "$PI_HOST:$PI_DIR/launa-server"
scp -r /tmp/launa-deploy/web/dist "$PI_HOST:$PI_DIR/web/dist"
ssh "$PI_HOST" "chmod +x $PI_DIR/launa-server"

# Step 4: Install systemd service
echo "[4/4] Installing systemd service..."
ssh -t "$PI_HOST" "sudo tee /etc/systemd/system/launa-server.service > /dev/null" << SERVICE
[Unit]
Description=Launa MQTT Broker + Web UI
After=network.target

[Service]
Type=simple
User=$PI_USER
WorkingDirectory=$PI_DIR
AmbientCapabilities=CAP_NET_BIND_SERVICE
ExecStart=$PI_DIR/launa-server --web-dir $PI_DIR/web --state-path $PI_DIR/data/launa-state.json --http-port 80
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

#!/usr/bin/env bash
set -euo pipefail

# Heraldvis — Ubuntu only quick installer
# 1-command (preferred, keeps tty): bash <(curl -fsSL https://raw.githubusercontent.com/Alfinpratamaa/heraldvis/main/scripts/install.sh)
# fallback (also works now with /dev/tty): curl -fsSL https://raw.githubusercontent.com/Alfinpratamaa/heraldvis/main/scripts/install.sh | bash
# or: curl -fsSL https://github.com/Alfinpratamaa/heraldvis/releases/download/v0.1.0/install.sh | bash

REPO="Alfinpratamaa/heraldvis"
TAG="v0.1.0"
TARBALL="heraldvis-linux-x86_64.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${TARBALL}"

echo "=== Heraldvis Installer (Ubuntu only) ==="

# OS check
if [ -f /etc/os-release ]; then
  . /etc/os-release
  if [[ "${ID:-}" != "ubuntu" ]]; then
    echo "⚠️  Detected ${PRETTY_NAME:-unknown} — only Ubuntu is supported. Continuing anyway..."
  else
    echo "✓ Ubuntu ${VERSION_ID:-} detected"
  fi
else
  echo "⚠️  Cannot detect OS — only Ubuntu is supported. Continuing..."
fi

# deps check
for cmd in curl tar; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "✗ Required: $cmd not found. Install with: sudo apt update && sudo apt install -y $cmd"
    exit 1
  fi
done

echo "→ Downloading $URL ..."
if command -v curl >/dev/null 2>&1; then
  curl -fL -o "$TARBALL" "$URL"
else
  wget -O "$TARBALL" "$URL"
fi

echo "→ Extracting $TARBALL ..."
tar -xzf "$TARBALL"
BIN="./heraldvis-linux-x86_64/heraldvis"
if [ ! -f "$BIN" ]; then
  # fallback if tar extracts flat
  BIN="./heraldvis"
fi
chmod +x "$BIN" 2>/dev/null || true

echo ""
echo "✓ Binary ready: $BIN"
"$BIN" --help | head -20 || true

echo ""
echo "--- Configuration (FR-5a precedence: CLI > env > config.toml) ---"
if [ -t 0 ]; then
  read -r -p "Endpoint URL [http://127.0.0.1:8000]: " INPUT_EP || INPUT_EP=""
else
  read -r -p "Endpoint URL [http://127.0.0.1:8000]: " INPUT_EP < /dev/tty || INPUT_EP=""
fi
ENDPOINT="${INPUT_EP:-http://127.0.0.1:8000}"
if [ -t 0 ]; then
  read -r -p "API Key (optional, press Enter to skip): " INPUT_KEY || INPUT_KEY=""
else
  read -r -p "API Key (optional, press Enter to skip): " INPUT_KEY < /dev/tty || INPUT_KEY=""
fi
API_KEY="${INPUT_KEY:-}"

echo ""
echo "=== Quick Start ==="
echo "Config: endpoint=$ENDPOINT api_key=${API_KEY:+***set*** (hidden)}"
echo ""
if [ -n "$API_KEY" ]; then
  echo "Run:"
  echo "  $BIN --endpoint \"$ENDPOINT\" --api-key \"$API_KEY\""
  echo ""
  echo "Or via env:"
  echo "  HERALDVIS_ENDPOINT=\"$ENDPOINT\" HERALDVIS_API_KEY=\"$API_KEY\" $BIN"
else
  echo "Run:"
  echo "  $BIN --endpoint \"$ENDPOINT\""
  echo ""
  echo "Or via env:"
  echo "  HERALDVIS_ENDPOINT=\"$ENDPOINT\" $BIN"
fi
echo ""
echo "Verify dispatcher:"
echo "  $BIN --check"
echo ""
echo "Notes: Linux Ubuntu only — binary built on ubuntu-latest (libasound2). For other distros use Docker/source build."

# optional auto-run check
if [ -t 0 ]; then
  read -r -p "Run --check now? [Y/n]: " RUN_CHECK || RUN_CHECK="Y"
else
  read -r -p "Run --check now? [Y/n]: " RUN_CHECK < /dev/tty || RUN_CHECK="Y"
fi
RUN_CHECK="${RUN_CHECK:-Y}"
if [[ "$RUN_CHECK" =~ ^[Yy]$ ]]; then
  if [ -n "$API_KEY" ]; then
    "$BIN" --endpoint "$ENDPOINT" --api-key "$API_KEY" --check || true
  else
    "$BIN" --endpoint "$ENDPOINT" --check || true
  fi
fi

echo ""
echo "Done. See $BIN --help for more."

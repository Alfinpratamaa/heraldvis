#!/usr/bin/env bash
set -euo pipefail

# === Safe defaults for set -u (init before any use) ===
ENDPOINT="http://127.0.0.1:8000"
API_KEY=""
INPUT_EP=""
INPUT_KEY=""
RUN_CHECK="Y"
SAVE_BASHRC="Y"

# Heraldvis — Ubuntu only quick installer
# Preferred (keeps tty): bash <(curl -fsSL https://raw.githubusercontent.com/Alfinpratamaa/heraldvis/main/scripts/install.sh)
# Fallback (pipe, now also works via /dev/tty): curl -fsSL https://raw.githubusercontent.com/Alfinpratamaa/heraldvis/main/scripts/install.sh | bash
# Release asset: curl -fsSL https://github.com/Alfinpratamaa/heraldvis/releases/download/v0.1.0/install.sh | bash

REPO="Alfinpratamaa/heraldvis"
TAG="v0.1.0"
TARBALL="heraldvis-linux-x86_64.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${TARBALL}"

echo "=== Heraldvis Installer (Ubuntu only) ==="

# OS check
if [ -f /etc/os-release ]; then
  . /etc/os-release
  if [[ "${ID:-}" != "ubuntu" ]]; then
    echo "⚠️  Detected ${PRETTY_NAME:-unknown} — only Ubuntu is supported. Continuing anyway..." >&2
  else
    echo "✓ Ubuntu ${VERSION_ID:-} detected"
  fi
else
  echo "⚠️  Cannot detect OS — only Ubuntu is supported. Continuing..." >&2
fi

# deps check
for cmd in curl tar; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "✗ Required: $cmd not found. Install with: sudo apt update && sudo apt install -y $cmd" >&2
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
  BIN="./heraldvis"
fi
chmod +x "$BIN" 2>/dev/null || true

echo ""
echo "✓ Binary ready: $BIN"
"$BIN" --help | head -20 || true

# === 1. Pasang Binary ke $PATH ===
echo ""
echo "→ Memasang binary ke PATH..."
TARGET_DIR="/usr/local/bin"
INSTALL_SUCCESS=false

if [ -w "$TARGET_DIR" ]; then
  install -m 755 "$BIN" "$TARGET_DIR/heraldvis" && INSTALL_SUCCESS=true
elif command -v sudo >/dev/null 2>&1; then
  echo "→ Memasang binary ke $TARGET_DIR (memerlukan hak akses sudo)..."
  sudo install -m 755 "$BIN" "$TARGET_DIR/heraldvis" && INSTALL_SUCCESS=true || INSTALL_SUCCESS=false
fi

# Fallback jika /usr/local/bin tidak berhasil
if [ "$INSTALL_SUCCESS" = false ]; then
  USER_BIN="$HOME/.local/bin"
  mkdir -p "$USER_BIN"
  install -m 755 "$BIN" "$USER_BIN/heraldvis"
  TARGET_DIR="$USER_BIN"
  echo "✓ Binary dipasang ke $USER_BIN/heraldvis"
  if [[ ":$PATH:" != *":$USER_BIN:"* ]]; then
    echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"
    echo "→ PATH ditambahkan ke ~/.bashrc"
  fi
else
  echo "✓ Binary berhasil dipasang ke $TARGET_DIR/heraldvis"
fi

# Update BIN to installed location for subsequent checks
INSTALLED_BIN="$TARGET_DIR/heraldvis"
if [ -x "$INSTALLED_BIN" ]; then
  BIN="$INSTALLED_BIN"
fi

echo ""
echo "--- Configuration (FR-5a precedence: CLI > env > config.toml) ---"

# Helper: prompt that works with curl | bash (stdin=pipe) by reading from /dev/tty
prompt_input() {
  local prompt_text="$1"
  local var_name="$2"
  local input=""
  printf "%s" "$prompt_text" >&2
  if [ -t 0 ]; then
    IFS= read -r input || true
  elif [ -e /dev/tty ]; then
    IFS= read -r input < /dev/tty || true
  else
    input=""
  fi
  printf -v "$var_name" "%s" "$input"
}

prompt_input "Endpoint URL [http://127.0.0.1:8000]: " INPUT_EP
ENDPOINT="${INPUT_EP:-http://127.0.0.1:8000}"

prompt_input "API Key (optional, press Enter to skip): " INPUT_KEY
API_KEY="${INPUT_KEY:-}"

# === 2. Simpan Konfigurasi Permanen ke ~/.bashrc ===
if [ -t 0 ]; then
  read -r -p "Save configuration to ~/.bashrc? [Y/n]: " SAVE_BASHRC || SAVE_BASHRC="Y"
elif [ -e /dev/tty ]; then
  printf "Save configuration to ~/.bashrc? [Y/n]: " > /dev/tty || true
  IFS= read -r SAVE_BASHRC < /dev/tty || SAVE_BASHRC="Y"
else
  SAVE_BASHRC="Y"
fi

SAVE_BASHRC="${SAVE_BASHRC:-Y}"
if [[ "$SAVE_BASHRC" =~ ^[Yy]$ ]]; then
  BASHRC="$HOME/.bashrc"
  if [ -f "$BASHRC" ]; then
    sed -i '/# Heraldvis configuration/d' "$BASHRC"
    sed -i '/export HERALDVIS_ENDPOINT=/d' "$BASHRC"
    sed -i '/export HERALDVIS_API_KEY=/d' "$BASHRC"
  fi

  echo "" >> "$BASHRC"
  echo "# Heraldvis configuration" >> "$BASHRC"
  echo "export HERALDVIS_ENDPOINT=\"$ENDPOINT\"" >> "$BASHRC"
  if [ -n "$API_KEY" ]; then
    echo "export HERALDVIS_API_KEY=\"$API_KEY\"" >> "$BASHRC"
  fi
  echo "✓ Konfigurasi tersimpan di ~/.bashrc"
fi

echo ""
echo "=== Quick Start ==="
echo "Config: endpoint=$ENDPOINT api_key=${API_KEY:+***set*** (hidden)}"
echo ""
if [ -n "$API_KEY" ]; then
  echo "Run:"
  echo "  $BIN --endpoint \"$ENDPOINT\" --api-key \"$API_KEY\""
  echo ""
  echo "Or via env (now persisted in ~/.bashrc):"
  echo "  heraldvis"
else
  echo "Run:"
  echo "  $BIN --endpoint \"$ENDPOINT\""
  echo ""
  echo "Or via env (now persisted):"
  echo "  heraldvis"
fi
echo ""
echo "Verify dispatcher:"
echo "  $BIN --check"
echo ""
echo "Notes: Linux Ubuntu only — binary built on ubuntu-latest (libasound2). For other distros use Docker/source build."

# optional auto-run check
prompt_input "Run --check now? [Y/n]: " RUN_CHECK
RUN_CHECK="${RUN_CHECK:-Y}"
if [[ "$RUN_CHECK" =~ ^[Yy]$ ]]; then
  if [ -n "$API_KEY" ]; then
    "$BIN" --endpoint "$ENDPOINT" --api-key "$API_KEY" --check || true
  else
    "$BIN" --endpoint "$ENDPOINT" --check || true
  fi
fi

# === 3. Pembersihan & Pesan Selesai ===
rm -f "$TARBALL"

echo ""
echo "=== Instalasi Berhasil ==="
echo "Untuk langsung menggunakan di terminal ini:"
echo "  source ~/.bashrc"
echo ""
echo "Lalu jalankan dari direktori mana pun:"
echo "  heraldvis"
echo ""
echo "Done. See heraldvis --help for more."

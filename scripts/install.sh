#!/usr/bin/env bash
# Miru universal installer — Linux & macOS
# Usage: curl -sSL https://raw.githubusercontent.com/shizukutanaka/miru/main/scripts/install.sh | bash

set -euo pipefail

REPO="shizukutanaka/miru"
INSTALL_DIR="/usr/local/bin"
VERSION="${MIRU_VERSION:-latest}"

# ─── OS / Arch detection ──────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)       echo "Unsupported arch: $ARCH" && exit 1 ;;
    esac
    EXT="tar.gz"
    ;;
  Darwin)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-apple-darwin" ;;
      arm64)   TARGET="aarch64-apple-darwin" ;;
      *)       echo "Unsupported arch: $ARCH" && exit 1 ;;
    esac
    EXT="tar.gz"
    ;;
  *)
    echo "Unsupported OS: $OS. Use install.ps1 on Windows."
    exit 1
    ;;
esac

# ─── Resolve version ──────────────────────────────────────────────────────────
if [ "$VERSION" = "latest" ]; then
  VERSION="$(curl -sSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')"
fi

echo "Installing Miru $VERSION for $TARGET"

# ─── Download ─────────────────────────────────────────────────────────────────
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

URL="https://github.com/$REPO/releases/download/$VERSION/miru-$TARGET.$EXT"
echo "Downloading: $URL"
curl -sSL "$URL" -o "$TMP/miru.$EXT"

tar -xzf "$TMP/miru.$EXT" -C "$TMP"

# ─── Install ──────────────────────────────────────────────────────────────────
SUDO=""
if [ ! -w "$INSTALL_DIR" ]; then
  SUDO="sudo"
fi

$SUDO install -m 755 "$TMP/miru-host" "$INSTALL_DIR/miru-host"
$SUDO install -m 755 "$TMP/miru-signal" "$INSTALL_DIR/miru-signal"

# ─── macOS: remove quarantine ─────────────────────────────────────────────────
if [ "$OS" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$INSTALL_DIR/miru-host" 2>/dev/null || true
  xattr -d com.apple.quarantine "$INSTALL_DIR/miru-signal" 2>/dev/null || true
fi

# ─── Linux: systemd service ───────────────────────────────────────────────────
if [ "$OS" = "Linux" ] && command -v systemctl &>/dev/null; then
  $SUDO tee /etc/systemd/system/miru-host.service >/dev/null <<'EOF'
[Unit]
Description=Miru Remote Desktop Host
After=network.target graphical.target

[Service]
Type=simple
ExecStart=/usr/local/bin/miru-host
Restart=on-failure
RestartSec=5
Environment=MIRU_SIGNAL=ws://signal.miru.app:21115/ws

[Install]
WantedBy=graphical.target
EOF
  $SUDO systemctl daemon-reload
  echo ""
  echo "Systemd service installed. Enable with:"
  echo "  sudo systemctl enable --now miru-host"
fi

echo ""
echo "Miru $VERSION installed successfully!"
echo "  Host:   $INSTALL_DIR/miru-host"
echo "  Signal: $INSTALL_DIR/miru-signal"
echo ""
echo "Start the host:"
echo "  miru-host"

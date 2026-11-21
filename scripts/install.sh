#!/usr/bin/env bash
# Installs LiteChat for the current user (no root required).
# Usage: ./scripts/install.sh [--autostart] [--source /path/to/litechat]
set -euo pipefail

APP_NAME="LiteChat"
BIN_NAME="litechat"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"

SOURCE=""
AUTOSTART=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --autostart) AUTOSTART=1; shift ;;
        --source) SOURCE="${2:?--source requires a path}"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 [--autostart] [--source /path/to/litechat]"
            exit 0
            ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

if [[ -z "$SOURCE" ]]; then
    for candidate in \
        "$SCRIPT_DIR/$BIN_NAME" \
        "$ROOT_DIR/$BIN_NAME" \
        "$ROOT_DIR/target/release/$BIN_NAME"
    do
        if [[ -x "$candidate" ]]; then
            SOURCE="$candidate"
            break
        fi
    done
fi

if [[ -z "$SOURCE" || ! -x "$SOURCE" ]]; then
    echo "Cannot find executable '$BIN_NAME'. Build it first with 'cargo build --release' or pass --source <path>." >&2
    exit 1
fi

BIN_DIR="${HOME}/.local/bin"
DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-${HOME}/.config}"
APP_DIR="${DATA_HOME}/applications"
ICON_DIR="${DATA_HOME}/icons/hicolor/256x256/apps"
AUTOSTART_DIR="${CONFIG_HOME}/autostart"

echo "Installing $APP_NAME to $BIN_DIR/$BIN_NAME"
pkill -x "$BIN_NAME" 2>/dev/null || true
sleep 0.4

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"
install -m 755 "$SOURCE" "$BIN_DIR/$BIN_NAME"

if [[ -f "$ROOT_DIR/assets/litechat.png" ]]; then
    install -m 644 "$ROOT_DIR/assets/litechat.png" "$ICON_DIR/litechat.png"
fi

cat > "$APP_DIR/litechat.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=$APP_NAME
Comment=Lightweight desktop client for WhatsApp Web
Exec="$BIN_DIR/$BIN_NAME"
Icon=litechat
Terminal=false
Categories=Network;InstantMessaging;
StartupWMClass=litechat
StartupNotify=true
EOF

if [[ "$AUTOSTART" -eq 1 ]]; then
    mkdir -p "$AUTOSTART_DIR"
    cat > "$AUTOSTART_DIR/litechat.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=$APP_NAME
Comment=Lightweight desktop client for WhatsApp Web
Exec="$BIN_DIR/$BIN_NAME" --hidden
Icon=litechat
Terminal=false
Categories=Network;InstantMessaging;
StartupWMClass=litechat
X-GNOME-Autostart-enabled=true
EOF
    echo "Autostart on login: enabled"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APP_DIR" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${DATA_HOME}/icons/hicolor" 2>/dev/null || true
fi

echo "Installation complete!"
echo "Make sure $BIN_DIR is in your PATH, or launch '$APP_NAME' from your application menu."

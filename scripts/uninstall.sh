#!/usr/bin/env bash
# Uninstalls LiteChat for the current user.
# Usage: ./scripts/uninstall.sh [--remove-data]
set -euo pipefail

BIN_NAME="litechat"
REMOVE_DATA=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --remove-data) REMOVE_DATA=1; shift ;;
        -h|--help)
            echo "Usage: $0 [--remove-data]"
            exit 0
            ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-${HOME}/.config}"

pkill -x "$BIN_NAME" 2>/dev/null || true
sleep 0.4

rm -f "${HOME}/.local/bin/${BIN_NAME}"
rm -f "${DATA_HOME}/applications/litechat.desktop"
rm -f "${DATA_HOME}/icons/hicolor/256x256/apps/litechat.png"
rm -f "${CONFIG_HOME}/autostart/litechat.desktop"

if [[ "$REMOVE_DATA" -eq 1 ]]; then
    rm -rf "${DATA_HOME}/LiteChat"
    echo "Removed session data in ${DATA_HOME}/LiteChat"
else
    echo "Session data kept in ${DATA_HOME}/LiteChat (pass --remove-data to remove it)."
fi

echo "LiteChat uninstalled."

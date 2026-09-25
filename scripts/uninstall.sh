#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later

set -euo pipefail

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"

systemctl --user disable --now membrie-capture.service membried.service >/dev/null 2>&1 || true

rm -f "$HOME/.local/bin/membrie"
rm -f "$HOME/.local/libexec/membrie/membried"
rm -f "$HOME/.local/libexec/membrie/membrie-capture"
rmdir "$HOME/.local/libexec/membrie" 2>/dev/null || true
rm -f "$data_home/applications/com.chuk.Membrie.desktop"
rm -f "$data_home/icons/hicolor/scalable/apps/com.chuk.Membrie.svg"
rm -f "$config_home/systemd/user/membried.service"
rm -f "$config_home/systemd/user/membrie-capture.service"

if command -v gnome-extensions >/dev/null 2>&1; then
    gnome-extensions disable membrie@chuk.local >/dev/null 2>&1 || true
    gnome-extensions uninstall membrie@chuk.local >/dev/null 2>&1 || true
fi

systemctl --user daemon-reload
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data_home/applications"
fi

echo "Membrie has been uninstalled."
echo "Your database and backups were kept in $data_home/membrie."

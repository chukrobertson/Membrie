#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later

set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
local_bin="$HOME/.local/bin"
libexec_dir="$HOME/.local/libexec/membrie"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
applications_dir="$data_home/applications"
icons_dir="$data_home/icons/hicolor/scalable/apps"
units_dir="$config_home/systemd/user"
desktop_template="$project_dir/assets/com.chuk.Membrie.desktop"
desktop_file="$applications_dir/com.chuk.Membrie.desktop"
temporary_desktop="$(mktemp /tmp/membrie-desktop.XXXXXX)"

cleanup() {
    rm -f "$temporary_desktop"
}
trap cleanup EXIT INT TERM

for command in cargo install gnome-extensions systemctl; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "Membrie needs '$command' before it can be installed."
        exit 1
    fi
done

echo "Building Membrie…"
cargo build --manifest-path "$project_dir/Cargo.toml" --workspace --release --locked

if systemctl --user is-active --quiet membrie-capture.service; then
    systemctl --user stop membrie-capture.service
fi
if systemctl --user is-active --quiet membried.service; then
    systemctl --user stop membried.service
fi

install -Dm755 "$project_dir/target/release/membrie" "$local_bin/membrie"
install -Dm755 "$project_dir/target/release/membried" "$libexec_dir/membried"
install -Dm755 "$project_dir/target/release/membrie-capture" "$libexec_dir/membrie-capture"
install -Dm755 "$project_dir/scripts/membrie-calendar" "$libexec_dir/membrie-calendar"
install -Dm644 "$project_dir/assets/com.chuk.Membrie.svg" "$icons_dir/com.chuk.Membrie.svg"
install -Dm644 "$project_dir/systemd/membried.service" "$units_dir/membried.service"
install -Dm644 "$project_dir/systemd/membrie-capture.service" "$units_dir/membrie-capture.service"

escaped_exec=${local_bin// /\\ }
sed "s|@MEMBRIE_EXEC@|$escaped_exec/membrie|g" "$desktop_template" > "$temporary_desktop"
install -Dm644 "$temporary_desktop" "$desktop_file"

"$project_dir/scripts/install-gnome-extension.sh"

systemctl --user daemon-reload
systemctl --user reenable membried.service membrie-capture.service >/dev/null
systemctl --user start membried.service || true
systemctl --user start membrie-capture.service || true

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications_dir"
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache --force --ignore-theme-index "$data_home/icons/hicolor" >/dev/null
fi

echo
echo "Membrie is installed. Open it from the Ubuntu app grid."
if ! systemctl --user is-active --quiet membried.service; then
    echo "The installed service is waiting because another development copy may be running."
    echo "Close that copy, then log out and back in once."
elif ! systemctl --user is-active --quiet membrie-capture.service; then
    echo "Clipboard capture will start after the GNOME bridge loads. Log out and back in once."
fi
if ! /usr/bin/python3 -c "import gi; gi.require_version('ECal', '2.0')" >/dev/null 2>&1; then
    echo "Optional calendar support needs: sudo apt install gir1.2-ecal-2.0"
fi
echo "Your Remembries remain in $data_home/membrie."

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later

set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
extension_dir="$project_dir/gnome-shell-extension/membrie@chuk.local"
build_dir="$(mktemp -d /tmp/membrie-extension.XXXXXX)"
extension_uuid="membrie@chuk.local"

cleanup() {
    rm -rf "$build_dir"
}
trap cleanup EXIT INT TERM

gnome-extensions pack --force --out-dir "$build_dir" "$extension_dir"
gnome-extensions install --force "$build_dir/$extension_uuid.shell-extension.zip"

if gnome-extensions enable "$extension_uuid" >/dev/null 2>&1 \
    && gnome-extensions info "$extension_uuid" 2>/dev/null | grep -q "State: ACTIVE"; then
    echo "Membrie Clipboard Bridge installed and enabled."
    echo "If this was an update, log out and back in once to load the new bridge code."
else
    enabled_extensions="$(gsettings get org.gnome.shell enabled-extensions)"
    if [[ "$enabled_extensions" != *"'$extension_uuid'"* ]]; then
        if [[ "$enabled_extensions" == "@as []" || "$enabled_extensions" == "[]" ]]; then
            enabled_extensions="['$extension_uuid']"
        else
            enabled_extensions="${enabled_extensions%]}, '$extension_uuid']"
        fi
        gsettings set org.gnome.shell enabled-extensions "$enabled_extensions"
    fi

    echo "Membrie Clipboard Bridge installed and queued to enable."
    echo "GNOME needs one log out and back in before it can load a newly installed local extension."
fi

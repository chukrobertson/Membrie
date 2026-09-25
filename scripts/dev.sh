#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later

set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"

cargo build --workspace

"$project_dir/target/debug/membried" &
daemon_pid=$!
capture_pid=""

cleanup() {
    if [[ -n "$capture_pid" ]]; then
        kill "$capture_pid" 2>/dev/null || true
        wait "$capture_pid" 2>/dev/null || true
    fi
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if [[ -n "${MEMBRIE_SOCKET:-}" ]]; then
    socket_path="$MEMBRIE_SOCKET"
elif [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
    socket_path="$XDG_RUNTIME_DIR/membrie.sock"
elif [[ -n "${MEMBRIE_DATA_DIR:-}" ]]; then
    socket_path="$MEMBRIE_DATA_DIR/membrie.sock"
elif [[ -n "${XDG_DATA_HOME:-}" ]]; then
    socket_path="$XDG_DATA_HOME/membrie/membrie.sock"
else
    socket_path="$HOME/.local/share/membrie/membrie.sock"
fi

for _ in $(seq 1 50); do
    if [[ -S "$socket_path" ]]; then
        break
    fi
    sleep 0.05
done

"$project_dir/target/debug/membrie-capture" &
capture_pid=$!

"$project_dir/target/debug/membrie"

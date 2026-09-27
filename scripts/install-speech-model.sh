#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later

set -euo pipefail

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
model_dir="$data_home/membrie/models"
model_path="$model_dir/ggml-base.en-q5_1.bin"
model_url="https://huggingface.co/ggerganov/whisper.cpp/resolve/f281eb45af861ab5e5297d23694b7d46e090c02c/ggml-base.en-q5_1.bin?download=true"
model_sha256="4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f"

for command in curl ffmpeg ffprobe sha256sum whisper-cli; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "Local voice-note understanding first needs Ubuntu's speech tools:"
        echo "  sudo apt install ffmpeg whisper.cpp"
        exit 1
    fi
done

install -d -m 700 "$model_dir"
if [[ -f "$model_path" ]] && printf '%s  %s\n' "$model_sha256" "$model_path" | sha256sum --check --status; then
    echo "Membrie's local speech model is already installed."
else
    temporary_model="$(mktemp "$model_dir/.speech-model.XXXXXX")"
    cleanup() {
        rm -f "$temporary_model"
    }
    trap cleanup EXIT INT TERM
    echo "Downloading the 57 MiB English voice-note model…"
    curl --fail --location --retry 3 --output "$temporary_model" "$model_url"
    printf '%s  %s\n' "$model_sha256" "$temporary_model" | sha256sum --check --status
    chmod 600 "$temporary_model"
    mv -f "$temporary_model" "$model_path"
    trap - EXIT INT TERM
    echo "The verified local speech model is installed."
fi

if systemctl --user is-active --quiet membried.service; then
    systemctl --user restart membried.service
fi

echo "Voice notes will now be transcribed on this PC and added to Brie's evidence."

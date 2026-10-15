#!/usr/bin/env bash
set -euo pipefail

case "$#:${1:-}" in
    0:|1:--purge) ;;
    *) echo "Usage: $0 [--purge]" >&2; exit 2 ;;
esac
command -v jq >/dev/null || { echo "Install jq to check recovery state before uninstalling." >&2; exit 1; }

bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}"
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/insomnia-awake"

if [ "$(systemctl --user show -p LoadState --value insomnia-awake.service)" != "not-found" ]; then
    systemctl --user stop insomnia-awake.service
    systemctl --user disable insomnia-awake.service
fi
if busctl --user status io.github.rafaelt0.InsomniaAwake >/dev/null 2>&1; then
    "$bin_dir/insomnia-awake" quit
fi

journal="$state_dir/journal.json"
if [ -e "$state_dir/session.json" ] || { [ -e "$journal" ] && ! jq -e 'type == "object" and .mutedSinks == [] and .backlight == null' "$journal" >/dev/null 2>&1; }; then
    echo "Recovery is incomplete. Kept the installation and $state_dir." >&2
    [ ! -e "$journal" ] || cat "$journal" >&2
    echo "Unmute the outputs it lists, and raise the screen brightness if it is dark." >&2
    exit 1
fi

rm -f "$bin_dir/insomnia-awake" \
    "$config_dir/systemd/user/insomnia-awake.service" \
    "$data_dir/applications/insomnia-awake.desktop" \
    "$data_dir/icons/hicolor/scalable/apps/insomnia-awake.svg"
systemctl --user daemon-reload

if [ "${1:-}" = "--purge" ]; then
    rm -rf "$config_dir/insomnia-awake" "$state_dir"
    echo "Removed Insomnia Awake, its settings and its state."
else
    echo "Removed Insomnia Awake. Settings stay in $config_dir/insomnia-awake (run with --purge to delete them)."
fi

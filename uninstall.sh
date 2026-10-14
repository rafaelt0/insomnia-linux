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
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/insomnia"

if [ "$(systemctl --user show -p LoadState --value insomnia.service)" != "not-found" ]; then
    systemctl --user stop insomnia.service
    systemctl --user disable insomnia.service
fi
if busctl --user status app.getinsomnia.Insomnia >/dev/null 2>&1; then
    "$bin_dir/insomnia" quit
fi

journal="$state_dir/journal.json"
if [ -e "$state_dir/session.json" ] || { [ -e "$journal" ] && ! jq -e 'type == "object" and .mutedSinks == [] and .backlight == null' "$journal" >/dev/null 2>&1; }; then
    echo "Recovery is incomplete. Kept the installation and $state_dir." >&2
    [ ! -e "$journal" ] || cat "$journal" >&2
    echo "Unmute the outputs it lists, and raise the screen brightness if it is dark." >&2
    exit 1
fi

rm -f "$bin_dir/insomnia" \
    "$config_dir/systemd/user/insomnia.service" \
    "$data_dir/applications/insomnia.desktop" \
    "$data_dir/icons/hicolor/scalable/apps/insomnia.svg"
systemctl --user daemon-reload

if [ "${1:-}" = "--purge" ]; then
    rm -rf "$config_dir/insomnia" "$state_dir"
    echo "Removed Insomnia, its settings and its state."
else
    echo "Removed Insomnia. Settings stay in $config_dir/insomnia (run with --purge to delete them)."
fi

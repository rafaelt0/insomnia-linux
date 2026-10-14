#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 0 ]; then
    echo "Usage: $0" >&2
    exit 2
fi

here=$(cd "$(dirname "$0")" && pwd)
bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

missing=()
for cmd in cargo pkg-config pactl systemctl busctl jq; do
    command -v "$cmd" >/dev/null || missing+=("$cmd")
done
if command -v pkg-config >/dev/null; then
pkg-config --atleast-version=4.12 gtk4 || missing+=("GTK 4.12 or newer development files")
pkg-config --exists gtk4-layer-shell-0 || missing+=("gtk4-layer-shell development files")
fi
if [ ${#missing[@]} -gt 0 ]; then
    printf 'Missing:\n' >&2
    printf '  %s\n' "${missing[@]}" >&2
    exit 1
fi
for service in org.freedesktop.login1 org.freedesktop.UPower; do
    busctl --system status "$service" >/dev/null 2>&1 || echo "Warning: $service is not on the system bus. Sessions may fail to start or end early." >&2
done
busctl --system status org.freedesktop.UPower.PowerProfiles >/dev/null 2>&1 ||
    echo "Note: power-profiles-daemon is not running, so the power-saver lid action will report an error." >&2

cargo build --release --locked --manifest-path "$here/Cargo.toml"

mkdir -p "$bin_dir" "$unit_dir" "$data_dir/applications" "$data_dir/icons/hicolor/scalable/apps"
# Copy, then rename over the old binary, which may be running.
cp "$here/target/release/insomnia" "$bin_dir/.insomnia.new"
chmod 755 "$bin_dir/.insomnia.new"
mv -f "$bin_dir/.insomnia.new" "$bin_dir/insomnia"
cp "$here/packaging/insomnia.service" "$unit_dir/insomnia.service"
desktop_exec=${bin_dir//\\/\\\\}
desktop_exec=${desktop_exec//\"/\\\"}
desktop_exec=${desktop_exec//\$/\\\$}
desktop_exec=${desktop_exec//\`/\\\`}
desktop_exec=${desktop_exec//%/%%}
while IFS= read -r line; do
    case "$line" in
        Exec=*) printf 'Exec="%s/insomnia" prompt\n' "$desktop_exec" ;;
        *) printf '%s\n' "$line" ;;
    esac
done <"$here/packaging/insomnia.desktop" >"$data_dir/applications/insomnia.desktop"
cp "$here/../docs/assets/eye-open.svg" "$data_dir/icons/hicolor/scalable/apps/insomnia.svg"
if command -v update-desktop-database >/dev/null; then
    update-desktop-database -q "$data_dir/applications" || true
fi

systemctl --user daemon-reload
systemctl --user enable insomnia.service >/dev/null
systemctl --user restart insomnia.service

for _ in $(seq 50); do
    busctl --user status app.getinsomnia.Insomnia >/dev/null 2>&1 && break
    sleep 0.1
done
if ! busctl --user status app.getinsomnia.Insomnia >/dev/null 2>&1; then
    echo "Installed, but the daemon did not start. See: journalctl --user -u insomnia" >&2
    exit 1
fi
echo "Installed $bin_dir/insomnia and started it."
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to PATH to use the insomnia command." ;;
esac
echo "Try: insomnia start 2h"

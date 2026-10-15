#!/usr/bin/env bash
# Runs uninstall.sh against a throwaway home with stand-ins for systemctl and
# busctl, so it never touches a real installation.
# shellcheck disable=SC2016,SC2317,SC2329 # stand-in scripts are literal text; helpers run through check
set -uo pipefail

script="$(cd "$(dirname "$0")/.." && pwd)/uninstall.sh"
failures=0
check() {
    local what=$1; shift
    if "$@" >/dev/null 2>&1; then echo "PASS  $what"; else echo "FAIL  $what"; failures=$((failures + 1)); fi
}

home=
setup() {
    [ -z "$home" ] || rm -rf "$home"
    home=$(mktemp -d)
    mkdir -p "$home/fake" "$home/.local/bin" "$home/.config/systemd/user" "$home/.config/insomnia" \
        "$home/.local/state/insomnia" "$home/.local/share/applications" "$home/.local/share/icons/hicolor/scalable/apps"
    printf '#!/bin/sh\necho "systemctl $*" >> "%s/calls"\n[ "$2" = show ] && echo loaded\nexit 0\n' "$home" >"$home/fake/systemctl"
    printf '#!/bin/sh\nexit 1\n' >"$home/fake/busctl"
    chmod +x "$home/fake/"*
    touch "$home/.local/bin/insomnia" "$home/.config/systemd/user/insomnia.service" "$home/.config/insomnia/config.json" \
        "$home/.local/share/applications/insomnia.desktop" "$home/.local/share/icons/hicolor/scalable/apps/insomnia.svg"
}
journal() { printf '%s' "$1" >"$home/.local/state/insomnia/journal.json"; }
run() {
    env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME HOME="$home" PATH="$home/fake:$PATH" \
        bash "$script" "$@" >"$home/out" 2>&1
    code=$?
}
installed() { [ -e "$home/.local/bin/insomnia" ] && [ -e "$home/.config/systemd/user/insomnia.service" ]; }
removed() { [ ! -e "$home/.local/bin/insomnia" ] && [ ! -e "$home/.config/systemd/user/insomnia.service" ] && [ ! -e "$home/.local/share/applications/insomnia.desktop" ]; }
said() { grep -q "$1" "$home/out"; }
trap 'rm -rf "$home"' EXIT

setup
journal '{"mutedSinks": ["alsa_output.speaker"], "backlight": null}'
run
check "a muted output left in the journal makes it refuse" test "$code" -eq 1
check "and says recovery is incomplete" said "Recovery is incomplete"
check "and shows what is still muted" said "alsa_output.speaker"
check "and keeps the installation" installed
check "and stops the daemon first, which is what restores things" grep -q "systemctl --user stop insomnia.service" "$home/calls"

setup
journal '{"mutedSinks": [], "backlight": {"device": "intel_backlight", "level": 900}}'
run
check "a dark screen left in the journal makes it refuse" test "$code" -eq 1
check "and keeps the installation" installed

setup
journal '{not json'
run
check "an unreadable journal makes it refuse" test "$code" -eq 1
check "and keeps the installation" installed

setup
journal '{"mutedSinks": [], "backlight": null}'
echo '{"startedAt": 1, "endsAt": 2}' >"$home/.local/state/insomnia/session.json"
run
check "a session file left behind makes it refuse" test "$code" -eq 1
check "and keeps the installation" installed

setup
journal '{"mutedSinks": [], "backlight": null}'
run
check "a clean journal lets it finish" test "$code" -eq 0
check "and removes the binary, unit and launcher" removed
check "and keeps the settings" test -e "$home/.config/insomnia/config.json"

setup
journal '{"mutedSinks": [], "backlight": null}'
run --purge
check "--purge finishes" test "$code" -eq 0
check "and deletes the settings" test ! -e "$home/.config/insomnia"
check "and deletes the state" test ! -e "$home/.local/state/insomnia"

setup
run --force
check "an unknown option is refused" test "$code" -eq 2
check "before anything is stopped" test ! -e "$home/calls"

if [ "$failures" -eq 0 ]; then echo "ALL PASSED"; else echo "$failures FAILED"; fi
exit $((failures > 0))

#!/usr/bin/env bash
# Drives the installed daemon's prompt the way a person does: opened from the
# tray, read through the accessibility bus, keys pressed through Hyprland.
# Needs Hyprland 0.56 or newer, python3 with Atspi, and no running session.
set -uo pipefail

bin="$HOME/.local/bin/insomnia"
failures=0
check() {
    local what=$1; shift
    if "$@" >/dev/null 2>&1; then echo "PASS  $what"; else echo "FAIL  $what"; failures=$((failures + 1)); fi
}

if ! "$bin" status | grep -q '^No session.'; then
    echo "End the running session first (insomnia end)." >&2
    exit 1
fi

prompt_open() { pgrep -f "^$bin prompt" >/dev/null; }
# shellcheck disable=SC2317 # only called through check
prompt_closed() { ! prompt_open; }
key() { hyprctl dispatch "hl.dsp.send_shortcut({ mods = \"\", key = \"$1\" })" >/dev/null; sleep 1; }
open_from_tray() {
    local item
    item=org.kde.StatusNotifierItem-$(systemctl --user show -p MainPID --value insomnia)-1
    busctl --user call "$item" /StatusNotifierItem org.kde.StatusNotifierItem Activate ii 0 0
    for _ in $(seq 30); do prompt_open && break; sleep 0.1; done
    sleep 0.5
}

# Prints "label|label|..." for the prompt's text fields, then its plain
# labels. With arguments, first types them into Days, Hours, Minutes.
atspi() {
    python3 -I - "$@" <<'EOF'
import sys, time, gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

def walk(node):
    yield node
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child:
            yield from walk(child)

for _ in range(30):
    desktop = Atspi.get_desktop(0)
    apps = [desktop.get_child_at_index(i) for i in range(desktop.get_child_count())]
    app = next((a for a in apps if a and (a.get_name() or "") == "insomnia"), None)
    if app:
        break
    time.sleep(0.1)
else:
    sys.exit("the prompt is not on the accessibility bus")
nodes = list(walk(app))
fields = [n for n in nodes if n.get_role_name() == "text"]
for field, value in zip(fields, sys.argv[1:]):
    Atspi.EditableText.set_text_contents(field, value)
print("|".join(f.get_name() for f in fields))
print("|".join(Atspi.Text.get_text(n, 0, -1) for n in nodes if n.get_role_name() == "label"))
EOF
}

open_from_tray
check "the tray's left click opens the prompt" prompt_open
read -r names labels < <(atspi | paste -sd' ')
check "the fields are named Days, Hours and Minutes" test "$names" = "Days|Hours|Minutes"
check "an idle prompt asks how long to keep awake" grep -q "Keep awake for" <<<"$labels"

atspi "x" "" "" >/dev/null
key Return
check "a word in Days keeps the prompt open" prompt_open
check "and says it is not a number" grep -q "is not a whole number" <<<"$(atspi | tail -1)"
check "and starts nothing" grep -q "^No session." <<<"$("$bin" status)"

atspi "" "" "3" >/dev/null
key Return
check "Return with 3 minutes closes the prompt" prompt_closed
check "and starts a session" grep -q "^Awake until" <<<"$("$bin" status)"
before=$("$bin" status | head -1)

open_from_tray
check "during a session the prompt adds time" grep -q "^Add" <<<"$(atspi | tail -1)"
key Escape
check "Escape closes it" prompt_closed
check "and changes nothing" test "$("$bin" status | head -1)" = "$before"

open_from_tray
atspi "" "1" "" >/dev/null
key Return
check "adding 1 hour closes the prompt" prompt_closed
check "and the session has more than an hour left" grep -q "1h" <<<"$("$bin" status | head -1)"

"$bin" end >/dev/null
if [ "$failures" -eq 0 ]; then echo "ALL PASSED"; else echo "$failures FAILED"; fi
exit $((failures > 0))

#!/usr/bin/env bash
# Drives a real daemon against the real logind, power-profiles-daemon and
# PipeWire on this machine. It mutes every output for a few seconds and holds
# power-saver briefly, then puts back what it found.
set -uo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
cargo build --quiet --manifest-path "$here/Cargo.toml" || exit 1
bin="$here/target/debug/insomnia"

if busctl --user status app.getinsomnia.Insomnia >/dev/null 2>&1; then
    echo "An Insomnia daemon is already running. Stop it first (systemctl --user stop insomnia)." >&2
    exit 1
fi

home=$(mktemp -d)
log="$home/daemon.log"
export INSOMNIA_HOME="$home" INSOMNIA_ALLOW_LID_SIMULATION=1
failures=0
daemon=

baseline_profile=$(powerprofilesctl get)
declare -A baseline_mute
while read -r sink; do baseline_mute[$sink]=$(pactl get-sink-mute "$sink"); done < <(pactl list short sinks | cut -f2)
module=$(pactl load-module module-null-sink sink_name=insomnia_e2e)
mkdir "$home/bin"
printf '#!/bin/sh\necho "$*" >> %s/pactl.calls\nexec /usr/bin/pactl "$@"\n' "$home" > "$home/bin/pactl"
chmod +x "$home/bin/pactl"

cleanup() {
    [ -n "$daemon" ] && kill "$daemon" 2>/dev/null && wait "$daemon" 2>/dev/null
    pactl unload-module "$module"
    for sink in "${!baseline_mute[@]}"; do
        [ "$(pactl get-sink-mute "$sink" 2>/dev/null)" = "${baseline_mute[$sink]}" ] && continue
        echo "FAIL  $sink did not end as it started (${baseline_mute[$sink]}); putting it back"
        pactl set-sink-mute "$sink" "$([ "${baseline_mute[$sink]}" = "Mute: yes" ] && echo 1 || echo 0)"
        failures=$((failures + 1))
    done
    if [ "$(powerprofilesctl get)" != "$baseline_profile" ]; then
        echo "FAIL  the power profile ended as $(powerprofilesctl get), not $baseline_profile"
        failures=$((failures + 1))
    fi
    echo "daemon log: $log"
    if [ "$failures" -eq 0 ]; then echo "ALL PASSED"; else echo "$failures FAILED"; fi
    rm -f "$home"/*.json
    exit $((failures > 0))
}
trap cleanup EXIT

check() {
    local what=$1; shift
    if "$@" >/dev/null 2>&1; then echo "PASS  $what"; else echo "FAIL  $what"; failures=$((failures + 1)); fi
}

# Polls for up to five seconds: the daemon converges after each event, not
# synchronously with the call that caused it.
eventually() {
    local what=$1; shift
    for _ in $(seq 50); do "$@" >/dev/null 2>&1 && { echo "PASS  $what"; return; }; sleep 0.1; done
    echo "FAIL  $what"; failures=$((failures + 1))
}

start_daemon() {
    PATH="$home/bin:$PATH" "$bin" >>"$log" 2>&1 &
    daemon=$!
    eventually "the daemon claims its bus name" busctl --user status app.getinsomnia.Insomnia
}

inhibiting() { systemd-inhibit --list --no-pager | grep -E '^Insomnia .*sleep:handle-lid-switch.*block'; }
not() { ! "$@"; }
muted() { [ "$(pactl get-sink-mute insomnia_e2e)" = "Mute: yes" ]; }
profile_is() { [ "$(powerprofilesctl get)" = "$1" ]; }
status_has() { "$bin" status | grep -q "$1"; }
says() { local want=$1 out; shift; out=$("$@" 2>&1); grep -q "$want" <<<"$out"; }

start_daemon
check "a fresh daemon reports no session" status_has "No session."
check "config.json is created with owner-only access" test "$(stat -c %a "$home/config.json")" = 600
check "nothing is inhibited before a session" not inhibiting

check "start 2m succeeds" says "Keeping awake until" "$bin" start 2m
check "sleep and the lid switch are blocked" inhibiting
check "session.json is written" test -s "$home/session.json"
check "a second start is refused" says "already running" "$bin" start 1m
check "the lid stays open, so nothing is muted" not muted

check "simulating a closed lid is accepted" "$bin" simulate-lid closed
eventually "a closed lid mutes the outputs" muted
eventually "a closed lid holds power-saver" profile_is power-saver
check "the mute journal names the test sink" grep -q insomnia_e2e "$home/journal.json"
check "status shows the closed lid" status_has "Lid closed (simulated)"

check "simulating an open lid is accepted" "$bin" simulate-lid open
eventually "an open lid unmutes" not muted
eventually "an open lid gives the profile back" profile_is "$baseline_profile"

check "closing again mutes again" "$bin" simulate-lid closed
eventually "the second close mutes" muted
kill -9 "$daemon"; wait "$daemon" 2>/dev/null
eventually "logind drops the block when the daemon dies" not inhibiting
eventually "power-profiles-daemon drops the hold when the daemon dies" profile_is "$baseline_profile"
check "the mute outlives the crash, journaled" muted

start_daemon
check "the restarted daemon resumes the session" status_has "Awake until"
eventually "the resumed session blocks sleep again" inhibiting
eventually "the restart unmutes what the crashed run muted" not muted
check "the journal is empty after the restore" not grep -q insomnia_e2e "$home/journal.json"

before=$(sed -n 's/.*"endsAt": \([0-9]*\).*/\1/p' "$home/session.json")
check "extend adds time" says "Keeping awake until" "$bin" extend 30m
after=$(sed -n 's/.*"endsAt": \([0-9]*\).*/\1/p' "$home/session.json")
check "the deadline moved by 30 minutes" test $((after - before)) -eq 1800

check "end succeeds" says "Session ended." "$bin" end
eventually "ending releases the block" not inhibiting
check "session.json is gone" test ! -e "$home/session.json"
check "end without a session is refused" says "No session is running." "$bin" end
check "extend without a session is refused" says "No session is running." "$bin" extend 5m

check "start with no duration uses the default preset" says "Keeping awake until" "$bin" start
check "the default session is 60 minutes" status_has "left)"
remaining=$(( $(sed -n 's/.*"endsAt": \([0-9]*\).*/\1/p' "$home/session.json") - $(date +%s) ))
check "the default session ends about an hour from now" test "$remaining" -gt 3500 -a "$remaining" -le 3600
"$bin" end >/dev/null

check "a 3 second session starts" "$bin" start 3s
check "the lid closes during it" "$bin" simulate-lid closed
eventually "it mutes" muted
sleep 4
eventually "the deadline ends the session" status_has "No session."
eventually "the deadline releases the block" not inhibiting
eventually "the deadline unmutes even with the lid still closed" not muted
eventually "the deadline gives the profile back" profile_is "$baseline_profile"
"$bin" simulate-lid real >/dev/null

check "a 2 minute session starts with every output muted by the user" "$bin" start 2m
for sink in $(pactl list short sinks | cut -f2); do pactl set-sink-mute "$sink" 1; done
"$bin" simulate-lid closed >/dev/null
sleep 1
: > "$home/pactl.calls"
sleep 3
check "a closed lid does not keep calling pactl" test ! -s "$home/pactl.calls"
"$bin" simulate-lid open >/dev/null
sleep 1
check "outputs the user muted stay muted after the lid opens" muted
"$bin" end >/dev/null
"$bin" simulate-lid real >/dev/null
pactl set-sink-mute insomnia_e2e 0
for sink in "${!baseline_mute[@]}"; do
    pactl set-sink-mute "$sink" "$([ "${baseline_mute[$sink]}" = "Mute: yes" ] && echo 1 || echo 0)"
done

check "bad durations are refused before reaching the daemon" not "$bin" start 31d
check "start 10m for the quit test" "$bin" start 10m
check "quit answers" says "Insomnia quit." "$bin" quit
wait "$daemon"; code=$?
daemon=
check "the daemon exits 0 on quit" test "$code" -eq 0
check "quitting ends the session" test ! -e "$home/session.json"
eventually "quitting releases the block" not inhibiting

start_daemon
"$bin" start 10m >/dev/null
"$bin" simulate-lid closed >/dev/null
eventually "a closed lid mutes before SIGTERM" muted
kill -TERM "$daemon"; wait "$daemon"; code=$?
daemon=
check "SIGTERM exits 0" test "$code" -eq 0
check "SIGTERM unmutes" not muted
check "SIGTERM ends the session" test ! -e "$home/session.json"

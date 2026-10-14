# Insomnia for Linux

Keep your computer awake for a set time, including with the lid closed.
The eye in the tray opens a days, hours, and minutes prompt. Right-click it
for presets, session controls, and lid settings.

This is a native Rust port with a GTK 4 prompt and a StatusNotifierItem tray.
It is experimental. The desktop tested here is CachyOS with Hyprland and
Noctalia. Other desktops can use the CLI even without a tray host.

## Install from this checkout

Install Rust with Cargo, a C compiler, pkg-config, GTK 4.12 or newer development
files, and gtk4-layer-shell development files. The installer also checks for
`pactl`, `jq`, `systemctl`, and `busctl`.

The desktop needs systemd-logind and a user D-Bus session. Laptops need UPower
for battery protection. Audio muting uses PulseAudio or PipeWire's PulseAudio
server. The power-saver action needs power-profiles-daemon with its hold API.

Run from the repository root:

```bash
./linux/install.sh
insomnia status
insomnia start 20m
```

The installer builds a release binary, installs it at `~/.local/bin/insomnia`,
and starts a systemd user service. It installs a desktop launcher and eye icon
under `$XDG_DATA_HOME`, or `~/.local/share`. The service starts with
`graphical-session.target`. It needs no sudoers rule or root access.

Run the installer again to upgrade. Restarting the service ends its current
session. To open the prompt from your application launcher, choose **Insomnia**.
If your desktop does not start `graphical-session.target`, run
`systemctl --user start insomnia` after login.

## Use the CLI

```bash
insomnia start 2h30m
insomnia extend 30m
insomnia status
insomnia end
insomnia prompt
insomnia quit
```

`start` without a duration uses the default preset of one hour. A bare number
means minutes. Units include `s`, `m`, `h`, and `d`. A session can last up to
30 days. Opening the lid leaves the timer running. The tray's left click opens
the prompt in add-time mode when a session is already active.

## Lid and battery behavior

During a session, Insomnia holds logind's `sleep:handle-lid-switch` inhibitor.
The tray and `insomnia status` report whether the lock was acquired. Desktop
components that bypass logind's inhibitors are outside this port's control.

With the lid closed, Insomnia saves and turns off the built-in panel's
backlight, mutes outputs that were unmuted, and holds power-saver. Opening the
lid or ending the session restores those changes. A screen brightness you set
by hand during the close is kept. Outputs you had already muted stay muted.
An unplugged output stays in the recovery journal until it reconnects, or you
choose to stop waiting for it in the tray menu.

When logind reports the laptop as docked, closing the lid turns off the built-in
panel but leaves audio and the lid's power-saver action alone. Battery rules
still apply. Docking is checked while a session runs because logind does not
announce changes to its `Docked` property.

On battery power, the defaults hold power-saver below 40% and end the session
below 10%. An unreadable battery ends the session after two failed reads when
the end floor is enabled. A desktop without a battery has no battery floors.

## Change settings

The tray has switches for panel darkening, muting, and power-saver on lid close.
Other settings are in `$XDG_CONFIG_HOME/insomnia/config.json`, or
`~/.config/insomnia/config.json`:

| Key | Default | Meaning |
| --- | --- | --- |
| `presetsMinutes` | `[30, 60, 120, 240, 480]` | Tray presets in minutes. |
| `defaultMinutes` | `60` | Duration for an empty start or prompt. |
| `darkenOnLidClose` | `true` | Turn off the built-in panel backlight. |
| `muteOnLidClose` | `true` | Mute currently connected, unmuted outputs. |
| `powerSaverOnLidClose` | `true` | Hold power-saver while closed and undocked. |
| `lowPowerFloor` | `40` | Hold power-saver below this battery percentage. |
| `endFloor` | `10` | End below this percentage. `0` disables this rule. |

After editing the file, run `systemctl --user reload insomnia`. Reloading keeps
the current session. Invalid values are repaired and logged. Invalid JSON is
moved aside and replaced with defaults.

## Recovery and logs

State lives in `$XDG_STATE_HOME/insomnia`, or `~/.local/state/insomnia`.
The daemon writes the session deadline to `session.json` and saves audio and
panel changes in `journal.json` before applying them. State files are written
with owner-only permissions.

Logind releases the sleep inhibitor when the process dies. Power-profiles-daemon
releases its hold when the D-Bus connection closes. The user service restarts a
crashed daemon after two seconds. An unexpired session resumes after restart.
An expired session ends, and the daemon attempts journaled restoration.

Audio muting and panel darkening can outlive a crash. Restoration needs the
daemon to run again. There is no independent recovery agent. Corrupt state is
kept beside the original file for inspection. Read status warnings and the log
if restoration fails:

```bash
insomnia status
journalctl --user -u insomnia
systemd-inhibit --list
```

## Uninstall

```bash
./linux/uninstall.sh
./linux/uninstall.sh --purge
```

The uninstaller stops the service and ends its session. It refuses to remove
the installation while session state or a nonempty or unreadable recovery
journal remains. Settings and state stay unless you pass `--purge`.

## Development and validation

```bash
cargo test --locked --manifest-path linux/Cargo.toml
cargo clippy --locked --manifest-path linux/Cargo.toml --all-targets -- -D warnings
```

`tests/services.rs` runs the daemon on a private D-Bus with service fixtures.
It does not change the host's audio, brightness, or power profile.

`tests/e2e.sh` runs the daemon against the host's logind, audio server, and
power-profiles-daemon. It temporarily mutes outputs and turns the panel off.
It restores their initial state on exit. Run it on a laptop with a working
backlight and power-profiles-daemon, while you can observe the screen:

```bash
systemctl --user stop insomnia
./linux/tests/e2e.sh
systemctl --user start insomnia
```

`tests/prompt.sh` drives the installed prompt on Hyprland. It opens it from
the tray, types through the accessibility bus, and presses keys through
Hyprland. It starts and ends a short session.

The live harness covers simulated lid states, actual sleep inhibitors,
deadline expiry, extending, brightness and audio restoration, crashes, config
reloads, and shutdown. It does not establish physical lid-switch delivery on
every laptop. Test a short, observed session on your hardware first.

This port does not include macOS app freezing, keyboard-backlight control,
hotspot failover, tmux nudges, browser relaunch controls, or app-specific thermal
rules. Kernel thermal protection and NetworkManager reconnection do not replace
those features. GNOME and other desktops without layer-shell use an ordinary
GTK window for the prompt. A tray host is needed to display the eye.

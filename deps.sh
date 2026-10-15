#!/usr/bin/env bash
# Installs what install.sh needs to build and run Insomnia, using this
# distribution's package manager. Asks for your password through sudo.
set -euo pipefail

min_rust=1.89
sudo=$([ "$(id -u)" -eq 0 ] || echo sudo)

# shellcheck source=/dev/null
. /etc/os-release
# A Rust from rustup conflicts with the distribution's package, so only add
# the package when there is no cargo yet.
have_cargo=
if command -v cargo >/dev/null; then have_cargo=yes; fi
family=" ${ID:-} ${ID_LIKE:-} "
case "$family" in
    *" arch "*)
        packages=(gtk4 gtk4-layer-shell pkgconf jq libpulse)
        [ -n "$have_cargo" ] || packages+=(rust)
        # Arch supports only full upgrades, so this asks before upgrading the
        # system too, unless nobody is at a terminal to answer.
        confirm=()
        [ -t 0 ] || confirm=(--noconfirm)
        $sudo pacman -Syu --needed "${confirm[@]}" "${packages[@]}" ;;
    *" fedora "*)
        packages=(gcc gtk4-devel gtk4-layer-shell-devel pkgconf-pkg-config jq pulseaudio-utils)
        [ -n "$have_cargo" ] || packages+=(cargo)
        $sudo dnf install -y "${packages[@]}" ;;
    *" suse "* | *" opensuse "*)
        packages=(gcc gtk4-devel gtk4-layer-shell-devel pkgconf jq pulseaudio-utils)
        [ -n "$have_cargo" ] || packages+=(cargo)
        $sudo zypper --non-interactive install "${packages[@]}" ;;
    *" debian "* | *" ubuntu "*)
        $sudo apt-get update
        packages=(build-essential libgtk-4-dev pkg-config jq pulseaudio-utils)
        apt-cache show libgtk4-layer-shell-dev >/dev/null 2>&1 && packages+=(libgtk4-layer-shell-dev)
        [ -n "$have_cargo" ] || packages+=(cargo)
        $sudo apt-get install -y "${packages[@]}" ;;
    *)
        echo "Unknown distribution ($ID). Install Rust $min_rust or newer, GTK 4.12+ development files," >&2
        echo "pkg-config, jq and pactl yourself. gtk4-layer-shell is optional." >&2
        exit 1 ;;
esac

rust=$(rustc --version 2>/dev/null | cut -d' ' -f2)
if [ -z "$rust" ] || [ "$(printf '%s\n%s\n' "$min_rust" "$rust" | sort -V | head -1)" != "$min_rust" ]; then
    echo "This needs Rust $min_rust or newer, and this system has ${rust:-none}. Install it with rustup:" >&2
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    exit 1
fi
echo "Dependencies installed. Next: ./install.sh"

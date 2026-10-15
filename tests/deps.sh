#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir "$fixture/bin"
sed "s|^\\. /etc/os-release$|. \"\$DEPS_OS_RELEASE\"|" "$root/deps.sh" >"$fixture/deps.sh"
printf 'ID=fedora\n' >"$fixture/os-release"
printf '#!/bin/sh\necho 0\n' >"$fixture/bin/id"
printf '#!/bin/sh\nexit 0\n' >"$fixture/bin/dnf"
printf '#!/bin/sh\nexit 0\n' >"$fixture/bin/cargo"
printf '#!/bin/sh\nexit 127\n' >"$fixture/bin/rustc"
chmod +x "$fixture/bin/"*

code=0
DEPS_OS_RELEASE="$fixture/os-release" PATH="$fixture/bin:$PATH" bash "$fixture/deps.sh" >"$fixture/output" 2>&1 || code=$?
test "$code" -eq 1
grep -q 'this system has none' "$fixture/output"
grep -q 'Install it with rustup' "$fixture/output"
echo "PASS  an unavailable rustc prints the rustup guidance"

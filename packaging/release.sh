#!/usr/bin/env bash
# Builds the release download: both binaries, the scripts that install them,
# and a checksum. Usage: packaging/release.sh VERSION OUTDIR
set -euo pipefail

version=${1:?usage: packaging/release.sh VERSION OUTDIR}
out=${2:?usage: packaging/release.sh VERSION OUTDIR}
root=$(cd "$(dirname "$0")/.." && pwd)
name="insomnia-$version-$(uname -m)-linux"
stage="$out/$name"

pkg-config --exists gtk4-layer-shell-0 || { echo "gtk4-layer-shell is needed to build the release" >&2; exit 1; }
cargo build --release --locked --manifest-path "$root/Cargo.toml" --target-dir "$out/target/layer-shell"
cargo build --release --locked --no-default-features --manifest-path "$root/Cargo.toml" --target-dir "$out/target/plain"

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/packaging"
install -m 755 "$out/target/layer-shell/release/insomnia" "$stage/bin/insomnia"
install -m 755 "$out/target/plain/release/insomnia" "$stage/bin/insomnia-plain"
install -m 755 "$root/install.sh" "$root/uninstall.sh" "$stage/"
install -m 644 "$root/README.md" "$root/LICENSE" "$stage/"
install -m 644 "$root/packaging/insomnia.service" "$root/packaging/insomnia.desktop" "$root/packaging/insomnia.svg" "$stage/packaging/"

tar -C "$out" -cJf "$out/$name.tar.xz" "$name"
(cd "$out" && sha256sum "$name.tar.xz" >"$name.tar.xz.sha256")
echo "$out/$name.tar.xz"

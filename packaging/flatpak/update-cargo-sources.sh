#!/usr/bin/env bash
# Regenerates packaging/flatpak/cargo-sources.json from Cargo.lock, so the
# Flatpak build can vendor every crate offline. Run it after any change to
# Cargo.lock and commit the result.
#
# Needs `uv` (https://docs.astral.sh/uv/), or python3 with aiohttp, PyYAML
# and tomlkit installed. The generator comes from flatpak-builder-tools;
# set FLATPAK_CARGO_GENERATOR to a local copy to skip the download.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
url="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

generator="${FLATPAK_CARGO_GENERATOR:-}"
if [[ -z "$generator" ]]; then
    generator="$work/flatpak-cargo-generator.py"
    curl -fsSL -o "$generator" "$url"
fi

out="$here/cargo-sources.json"
if command -v uv >/dev/null; then
    # The script declares its dependencies inline (PEP 723).
    uv run --quiet --script "$generator" "$root/Cargo.lock" -o "$out"
else
    python3 -I "$generator" "$root/Cargo.lock" -o "$out"
fi
echo "Wrote $out"

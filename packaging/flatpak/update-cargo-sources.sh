#!/usr/bin/env bash
# Regenerates packaging/flatpak/cargo-sources.json from Cargo.lock, so the
# Flatpak build can vendor every crate offline. Run it after any change to
# Cargo.lock (Dependabot's included) and commit the result.
#
#   packaging/flatpak/update-cargo-sources.sh           regenerate
#   packaging/flatpak/update-cargo-sources.sh --check   fail if it is out of
#                                                       step with Cargo.lock (CI)
#
# Regenerating needs `uv` (https://docs.astral.sh/uv/), or python3 with
# aiohttp, PyYAML and tomlkit installed. The generator comes from
# flatpak-builder-tools at a fixed commit (generator_rev below); set
# FLATPAK_CARGO_GENERATOR to a local copy to skip the download. --check only
# needs python3 (3.11 or newer) and no network.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="$here/cargo-sources.json"
# flatpak-builder-tools master on 2026-09-30; update it deliberately.
generator_rev=74697c75b630d7330e77250fc13cb5ea688d9479
url="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$generator_rev/cargo/flatpak-cargo-generator.py"

if [[ ${1:-} == --check ]]; then
    # The crates.io packages in Cargo.lock (name, version, checksum) must be
    # exactly the archives cargo-sources.json vendors.
    python3 -I - "$root/Cargo.lock" "$out" <<'EOF'
import json
import sys
import tomllib

lock_path, sources_path = sys.argv[1], sys.argv[2]
with open(lock_path, "rb") as f:
    lock = tomllib.load(f)
locked, other = set(), []
for pkg in lock.get("package", []):
    source = pkg.get("source")
    if source is None:
        continue  # the workspace's own crates
    if source.startswith("registry+") and "checksum" in pkg:
        locked.add((pkg["name"], pkg["version"], pkg["checksum"]))
    else:
        other.append(f"{pkg['name']} {pkg['version']} ({source})")

with open(sources_path) as f:
    sources = json.load(f)
vendored = set()
prefix = "https://static.crates.io/crates/"
for entry in sources:
    if entry.get("type") != "archive" or not entry.get("url", "").startswith(prefix):
        continue
    name, file = entry["url"][len(prefix):].split("/", 1)
    version = file[len(name) + 1 : -len(".crate")]
    vendored.add((name, version, entry["sha256"]))

missing = sorted(locked - vendored)
extra = sorted(vendored - locked)
for name, version, checksum in missing:
    print(f"missing from cargo-sources.json: {name} {version} ({checksum})")
for name, version, checksum in extra:
    print(f"not in Cargo.lock: {name} {version} ({checksum})")
for line in other:
    print(f"not a crates.io package, check it by hand: {line}")
if missing or extra or other:
    print("cargo-sources.json is out of step with Cargo.lock: "
          "run packaging/flatpak/update-cargo-sources.sh and commit the result")
    sys.exit(1)
print(f"cargo-sources.json matches Cargo.lock ({len(locked)} crates)")
EOF
    exit
fi
[[ $# -eq 0 ]] || { sed -n '2,15p' "${BASH_SOURCE[0]}" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

generator="${FLATPAK_CARGO_GENERATOR:-}"
if [[ -z "$generator" ]]; then
    generator="$work/flatpak-cargo-generator.py"
    curl -fsSL -o "$generator" "$url"
fi

if command -v uv >/dev/null; then
    # The script declares its dependencies inline (PEP 723).
    uv run --quiet --script "$generator" "$root/Cargo.lock" -o "$out"
else
    python3 -I "$generator" "$root/Cargo.lock" -o "$out"
fi
echo "Wrote $out"

#!/usr/bin/env bash
# Prints the Flathub version of the manifest: the same file, with the local
# `dir` source replaced by the release tag (and its commit, which Flathub
# requires so the build is reproducible), built without Swell's own update
# checks (SWELL_DISABLE_UPDATE_CHECK), since Flathub delivers the updates.
#
#   packaging/flatpak/flathub-manifest.sh            # version from Cargo.toml
#   packaging/flatpak/flathub-manifest.sh 1.0.1 > ../flathub/io.github.oxakromax.SwellPlayer.yml
#
# The tag v<version> must exist locally (git fetch --tags).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)}"
commit="$(git -C "$root" rev-list -n1 "v$version")"

awk -v tag="v$version" -v commit="$commit" '
  /^    CARGO_NET_OFFLINE: / {
    print
    print "    SWELL_DISABLE_UPDATE_CHECK: \"1\""
    next
  }
  /^      - type: dir$/ {
    print "      - type: git"
    print "        url: https://github.com/oxakromax/swell-player.git"
    print "        tag: " tag
    print "        commit: " commit
    print "        x-checker-data:"
    print "          type: git"
    print "          tag-pattern: ^v([\\d.]+)$"
    skipping = 1
    next
  }
  # Drop the dir source'"'"'s own keys (path, skip and its list).
  skipping && /^        / { next }
  { skipping = 0; print }
' "$here/io.github.oxakromax.SwellPlayer.yml"

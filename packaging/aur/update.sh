#!/usr/bin/env bash
# Updates packaging/aur/swell-player for a published release: sets pkgver,
# resets pkgrel, writes the real sha256 of the GitHub tag tarball and
# regenerates .SRCINFO (and the -git package's .SRCINFO).
#
#   packaging/aur/update.sh            # version from Cargo.toml
#   packaging/aur/update.sh 1.0.1      # explicit version
#
# Run it after the tag v<version> is pushed to GitHub. Needs curl,
# sha256sum and makepkg (Arch Linux).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)}"
url="https://github.com/oxakromax/swell-player/archive/refs/tags/v$version.tar.gz"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "Downloading $url"
curl -fsSL -o "$work/source.tar.gz" "$url"
sum="$(sha256sum "$work/source.tar.gz" | cut -d' ' -f1)"
echo "sha256: $sum"

pkgbuild="$here/swell-player/PKGBUILD"
current="$(sed -n 's/^pkgver=//p' "$pkgbuild")"
sed -i \
    -e "s/^pkgver=.*/pkgver=$version/" \
    -e "s/^sha256sums=.*/sha256sums=('$sum')/" \
    "$pkgbuild"
if [[ "$current" != "$version" ]]; then
    sed -i 's/^pkgrel=.*/pkgrel=1/' "$pkgbuild"
fi
# The placeholder note above sha256sums no longer applies.
sed -i '/^# PLACEHOLDER/,/^# regenerates \.SRCINFO\.$/d' "$pkgbuild"

for pkg in swell-player swell-player-git; do
    (cd "$here/$pkg" && makepkg --printsrcinfo > .SRCINFO)
done
echo "Updated $pkgbuild and .SRCINFO files; review with git diff, then publish (packaging/README.md)."

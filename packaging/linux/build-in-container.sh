#!/usr/bin/env bash
# Builds a .deb or .rpm of Swell in a clean container, from the files
# committed at HEAD (uncommitted changes are not included).
#
#   packaging/linux/build-in-container.sh deb [image]   # default ubuntu:25.10
#   packaging/linux/build-in-container.sh rpm [image]   # default fedora:43
#
# Packages land in target/packages. Cargo's registry, rustup and the target dir
# are cached in target/packages/cache between runs (delete it to start over). Needs
# docker or podman (set CONTAINER=podman).
set -euo pipefail

kind="${1:?usage: $0 deb|rpm [image]}"
case "$kind" in
  deb) image="${2:-ubuntu:25.10}" ;;
  rpm) image="${2:-fedora:43}" ;;
  *) echo "usage: $0 deb|rpm [image]" >&2; exit 2 ;;
esac
engine="${CONTAINER:-docker}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dist="$root/target/packages"
mkdir -p "$dist/cache"

src="$(mktemp -d)"
trap 'rm -rf "$src"' EXIT
git -C "$root" archive --format=tar HEAD | tar -x -C "$src"

# The script that runs inside the container.
cat > "$src/.container-build.sh" <<'EOF'
set -euxo pipefail
if command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends \
    build-essential pkg-config curl ca-certificates dpkg-dev \
    libgtk-4-dev libadwaita-1-dev libgstreamer1.0-dev libsqlite3-dev \
    libglib2.0-dev-bin
else
  dnf install -y --setopt=install_weak_deps=False \
    gcc make pkgconf-pkg-config curl ca-certificates rpm-build \
    gtk4-devel libadwaita-devel gstreamer1-devel sqlite-devel glib2-devel
fi
export CARGO_HOME="/cache/cargo-$KIND" RUSTUP_HOME="/cache/rustup-$KIND"
export CARGO_TARGET_DIR="/cache/target-$KIND"
export PATH="$CARGO_HOME/bin:$PATH"
command -v cargo >/dev/null ||
  curl -fsSL https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
cd /src
cargo build --release --locked
if [ "$KIND" = deb ]; then
  command -v cargo-deb >/dev/null || cargo install cargo-deb --locked
  cargo deb --no-build --output /dist/
else
  command -v cargo-generate-rpm >/dev/null || cargo install cargo-generate-rpm --locked
  cargo generate-rpm --target-dir "$CARGO_TARGET_DIR" --output /dist/
fi
chown -R "$OWNER" /cache /dist
EOF

"$engine" run --rm \
  -e KIND="$kind" -e OWNER="$(id -u):$(id -g)" \
  -v "$src:/src:ro" -v "$dist:/dist" -v "$dist/cache:/cache" \
  "$image" bash /src/.container-build.sh
ls -l "$dist"/*."$kind"

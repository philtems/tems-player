#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v docker >/dev/null 2>&1; then
    printf 'Docker est requis pour compiler la cible musl.\n' >&2
    exit 1
fi

docker run --rm -v "$ROOT_DIR:/work" -w /work rust:alpine sh -ec '
    apk add --no-cache curl build-base cmake pkgconf linux-headers alsa-lib-dev opus-dev wayland-dev libxkbcommon-dev libxcb-dev libx11-dev libxrandr-dev libxi-dev libxcursor-dev libxinerama-dev mesa-dev dbus-dev
    cd /tmp
    curl -fsSL https://www.alsa-project.org/files/pub/lib/alsa-lib-1.2.11.tar.bz2 | tar xj
    cd /tmp/alsa-lib-1.2.11
    ./configure --prefix=/usr --disable-shared --enable-static
    make -j"$(nproc)"
    make install
    cd /tmp
    curl -fsSL https://downloads.xiph.org/releases/opus/opus-1.4.tar.gz | tar xz
    cd /tmp/opus-1.4
    ./configure --prefix=/usr --disable-shared --enable-static
    make -j"$(nproc)"
    make install
    cd /work
    cargo build --release --target x86_64-unknown-linux-musl
'

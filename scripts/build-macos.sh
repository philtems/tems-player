#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SDKROOT="${SDKROOT:-/home/tems/Sources/MacOSX26.1.sdk}"

if [[ ! -d "$SDKROOT" ]]; then
    printf 'SDK macOS introuvable : %s\n' "$SDKROOT" >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    printf 'cargo est introuvable dans PATH.\n' >&2
    exit 1
fi

if ! cargo zigbuild --help >/dev/null 2>&1; then
    printf 'cargo-zigbuild est requis (cargo install cargo-zigbuild).\n' >&2
    exit 1
fi

cd "$ROOT_DIR"
export SDKROOT
export COREAUDIO_SDK_PATH="$SDKROOT"
export BINDGEN_EXTRA_CLANG_ARGS="--target=aarch64-apple-macosx26.1 -D__has_attribute(x)=0"
export LIBOPUS_NO_PKG=1
export LIBOPUS_STATIC=1

cargo zigbuild --target aarch64-apple-darwin --release
export BINDGEN_EXTRA_CLANG_ARGS="--target=x86_64-apple-macosx26.1 -D__has_attribute(x)=0"
cargo zigbuild --target x86_64-apple-darwin --release

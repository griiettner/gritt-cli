#!/usr/bin/env sh
# Deterministic release build of the `gritt` binary plus a SHA-256 checksum.
#
# Usage: scripts/release/build.sh [target-triple] [output-dir]
#
# The build pins the lockfile, fixes SOURCE_DATE_EPOCH to the last commit,
# and remaps the source and cargo paths so two builds of the same commit on
# the same toolchain produce byte-identical binaries. Run it twice into two
# output directories and compare the checksums to verify reproducibility.
#
# The compiler is pinned by rust-toolchain.toml at the repository root, so
# `cargo` and `rustc` below resolve to that exact version. BUILD-INFO
# records it beside the checksum for anyone rebuilding later.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TARGET=${1:-$(rustc -vV | sed -n 's/^host: //p')}
OUT=${2:-"$ROOT/dist/$TARGET"}
CARGO_HOME_DIR=${CARGO_HOME:-"$HOME/.cargo"}

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
  SOURCE_DATE_EPOCH=$(git -C "$ROOT" log -1 --pretty=%ct 2>/dev/null || echo 0)
fi
export SOURCE_DATE_EPOCH
export CARGO_INCREMENTAL=0
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$ROOT=/build --remap-path-prefix=$CARGO_HOME_DIR=/cargo"

cd "$ROOT"
PINNED=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
RUSTC_VERSION=$(rustc -vV | sed -n 's/^release: //p')
if [ "$RUSTC_VERSION" != "$PINNED" ]; then
  echo "rust-toolchain.toml pins $PINNED but rustc is $RUSTC_VERSION; run: rustup toolchain install $PINNED" >&2
  exit 1
fi
COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)
cargo build --release --locked --bin gritt --target "$TARGET"

mkdir -p "$OUT"
BIN=gritt
case "$TARGET" in *windows*) BIN=gritt.exe ;; esac
cp "target/$TARGET/release/$BIN" "$OUT/$BIN"

cd "$OUT"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$BIN" > SHA256SUMS
else
  shasum -a 256 "$BIN" > SHA256SUMS
fi
{
  echo "rustc $RUSTC_VERSION"
  echo "toolchain $PINNED"
  echo "target $TARGET"
  echo "commit $COMMIT"
  echo "source_date_epoch $SOURCE_DATE_EPOCH"
} > BUILD-INFO
cat SHA256SUMS BUILD-INFO

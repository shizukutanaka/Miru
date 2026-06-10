#!/usr/bin/env bash
# Build Miru reproducibly: same commit → bit-identical binaries on any host.
#
# Pin every variable that could leak into output bytes:
#   - Rust toolchain version   (rust-toolchain.toml)
#   - Build timestamp          (SOURCE_DATE_EPOCH from git commit)
#   - Locale                   (LC_ALL=C)
#   - Path                     (we use a stable build dir)
#   - Resource ordering        (codegen-units=1, lto=thin)
#
# Usage:
#   ./scripts/reproducible-build.sh            # build, print SHA256
#   ./scripts/reproducible-build.sh --verify   # also compare with SHA256SUMS

set -euo pipefail

cd "$(dirname "$0")/.."

# ─── Pinned environment ───────────────────────────────────────────────────────

export LC_ALL=C
export TZ=UTC
export LANG=C.UTF-8

# Reproducible timestamps: derived from the commit, not wall-clock
SOURCE_DATE_EPOCH="$(git log -1 --pretty=%ct)"
export SOURCE_DATE_EPOCH

# Disable parallelism artifacts and incremental compilation
export CARGO_INCREMENTAL=0
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$(pwd)=/build/miru -C codegen-units=1"

echo "── Reproducible build ─────────────────────────────────────"
echo "commit:            $(git rev-parse HEAD)"
echo "source date epoch: $SOURCE_DATE_EPOCH ($(date -u -d "@$SOURCE_DATE_EPOCH" +'%Y-%m-%dT%H:%M:%SZ'))"
echo "rust toolchain:    $(cat rust-toolchain.toml 2>/dev/null | head -1 || rustc --version)"
echo "host triple:       $(rustc --version --verbose | grep host | cut -d' ' -f2)"
echo

# ─── Build ────────────────────────────────────────────────────────────────────

CARGO_FLAGS=(--release --workspace --locked --frozen)
BINS=(miru-host miru-mcp miru-signal)
for bin in "${BINS[@]}"; do
  CARGO_FLAGS+=(--bin "$bin")
done

cargo build "${CARGO_FLAGS[@]}"

# ─── SHA256 inventory ────────────────────────────────────────────────────────

echo
echo "── SHA256 inventory ──────────────────────────────────────"
SUMS_FILE="target/release/SHA256SUMS"
> "$SUMS_FILE"
for bin in "${BINS[@]}"; do
  path="target/release/$bin"
  if [ -e "$path" ]; then
    sum="$(sha256sum "$path" | cut -d' ' -f1)"
    echo "$sum  $bin" | tee -a "$SUMS_FILE"
  fi
done

# ─── Optional: verify against published SHA256SUMS ───────────────────────────

if [ "${1:-}" = "--verify" ]; then
  echo
  echo "── Verifying against published SHA256SUMS ────────────────"
  REF="dist/SHA256SUMS"
  if [ ! -f "$REF" ]; then
    echo "ERROR: $REF not found. Download it from the release page first." >&2
    exit 2
  fi

  for bin in "${BINS[@]}"; do
    actual="$(sha256sum "target/release/$bin" | cut -d' ' -f1)"
    expected="$(grep "  $bin" "$REF" | cut -d' ' -f1)"
    if [ "$actual" = "$expected" ]; then
      echo "OK   $bin"
    else
      echo "FAIL $bin"
      echo "  expected: $expected"
      echo "  actual:   $actual"
      exit 1
    fi
  done
  echo
  echo "All binaries match the published SHA256SUMS."
fi

echo
echo "Done. Artifacts at target/release/, sums at $SUMS_FILE"

#!/usr/bin/env bash
# scripts/dev-bootstrap.sh — restore the Miru dev environment after a session reset.
#
# This is a DEV-ENVIRONMENT helper, not part of the product. It assumes the
# sandbox layout where the project lives at /home/claude/miru and the canonical
# copy is in /mnt/user-data/outputs/miru.
#
# Usage:  source scripts/dev-bootstrap.sh    (source it, so env vars persist)
#
# What it does, idempotently:
#   1. Restore project from outputs if /home/claude/miru is missing.
#   2. Install the Rust toolchain + system libs if absent.
#   3. Export build env that keeps target/ small and rebuilds fast.
#   4. Free disk if target/ has ballooned.
#   5. Start Xvfb on :99 if not already running (for capture/E2E tests).

set -uo pipefail

PROJECT=/home/claude/miru
CANON=/mnt/user-data/outputs/miru

# ── 1. Project ───────────────────────────────────────────────────────────────
if [ ! -d "$PROJECT" ]; then
    echo "→ restoring project from $CANON"
    cp -r "$CANON" "$PROJECT"
fi
cd "$PROJECT" || exit 1

# ── 2. Toolchain + system deps ─────────────────────────────────────────────────
if ! command -v cargo >/dev/null 2>&1; then
    echo "→ installing rust toolchain"
    apt-get install -y rustc-1.91 cargo-1.91 >/dev/null 2>&1
    update-alternatives --install /usr/local/bin/cargo cargo /usr/bin/cargo-1.91 100 2>/dev/null
    update-alternatives --install /usr/local/bin/rustc rustc /usr/bin/rustc-1.91 100 2>/dev/null
fi

# Probe one representative header; if missing, install the whole set.
if ! pkg-config --exists x11 2>/dev/null; then
    echo "→ installing system libs"
    apt-get install -y --fix-missing \
        libx11-dev libxcb-shm0-dev libxcb-randr0-dev libopus-dev libvpx-dev \
        libasound2-dev libclang-dev libgtk-3-dev pkg-config xvfb >/dev/null 2>&1
fi

# ── 3. Fast/small build profile (env-only, doesn't touch product Cargo.toml) ───
# opt-level=0 + line-tables-only debuginfo ≈ 3-4× smaller target/, faster rebuild.
export CARGO_PROFILE_DEV_OPT_LEVEL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_INCREMENTAL=1

# ── 4. Disk guard ──────────────────────────────────────────────────────────────
avail_kb=$(df --output=avail / | tail -1)
if [ "${avail_kb:-0}" -lt 2000000 ]; then   # < ~2 GB free
    echo "→ low disk; pruning target/ incremental + test deps"
    rm -rf target/debug/incremental 2>/dev/null
    # Keep the three product binaries; drop bulky test/dep artifacts.
    find target/debug/deps -name '*.rlib' -delete 2>/dev/null
fi

# ── 5. Xvfb for capture / E2E ──────────────────────────────────────────────────
if ! pgrep -f "Xvfb :99" >/dev/null 2>&1; then
    echo "→ starting Xvfb :99"
    rm -f /tmp/.X99-lock 2>/dev/null
    Xvfb :99 -screen 0 1280x720x24 -ac >/dev/null 2>&1 &
    sleep 2
fi
export DISPLAY=:99

echo "✓ Miru dev environment ready"
echo "  project:  $PROJECT"
echo "  cargo:    $(cargo --version 2>/dev/null)"
echo "  display:  $DISPLAY"
echo "  disk:     $(df -h / | awk 'NR==2{print $4" free"}')"

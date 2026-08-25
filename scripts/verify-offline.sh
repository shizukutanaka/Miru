#!/usr/bin/env bash
#
# Offline verification — the checks that still work without crates.io.
#
# WHY THIS EXISTS
#
# Sandboxed and air-gapped environments commonly cannot reach static.crates.io,
# so `cargo test --workspace` fails at dependency download and Rust changes end
# up merged on eyeball review alone. That is how a missing struct field once
# reached this branch: a compiler would have caught it in a second.
#
# Before assuming you are in that situation, CHECK — the failure is narrower
# than it looks, and each part has a different fix:
#
#   rustc missing?          Probably just the rust-toolchain.toml pin failing to
#                           download. `ls ~/.rustup/toolchains` and set
#                           RUSTUP_TOOLCHAIN to one that is there. This script
#                           already does that automatically.
#   ffmpeg/libvpx/pipewire  Try `sudo apt-get update` FIRST. A stale package
#   headers missing?        index 404s on every .deb and looks exactly like a
#                           blocked network. After updating, libavcodec-dev,
#                           libvpx-dev and libpipewire-0.3-dev install normally.
#   cargo cannot fetch?     Check which host is refused. index.crates.io is
#                           frequently allowed while static.crates.io — where
#                           the .crate tarballs live — is not, and only the
#                           second one stops the build. `cargo fetch` prints
#                           "CONNECT tunnel failed, response 403"; the host is
#                           named in $HTTPS_PROXY/__agentproxy/status under
#                           recentRelayFailures. If static.crates.io is denied,
#                           no amount of local setup will help and the checks
#                           below are the ceiling.
#
# These checks do not need any dependency to be downloaded:
#
#   1. Parse every tracked .rs file (rustfmt parses; it resolves nothing).
#   2. Validate every Cargo.toml and the workspace graph (cargo metadata
#      --no-deps resolves nothing, so it needs no registry), and check that no
#      .rs file is orphaned — unreachable from any `mod` declaration, and so
#      never compiled by cargo either.
#   3. Compile and RUN the tests of modules that only use std, with plain
#      rustc — no cargo, no registry. Modules with a small external surface
#      also run, against protocol types EXTRACTED from the real source so the
#      stand-ins cannot drift (scripts/offline_harness.py).
#   4. Frontend typecheck + unit tests (npm deps are usually reachable).
#
# THIS IS NOT A SUBSTITUTE for `cargo test --workspace`. It cannot catch a
# type error across crate boundaries, a wrong trait bound, or a bad API call
# into a dependency. Run the real thing before merging. This exists so that
# "I could not build it" stops meaning "I checked nothing".
#
# Usage:  scripts/verify-offline.sh
#
# If the toolchain pinned by rust-toolchain.toml cannot be downloaded, set
# RUSTUP_TOOLCHAIN to one that is already installed, e.g.
#   RUSTUP_TOOLCHAIN=stable scripts/verify-offline.sh
# (`ls ~/.rustup/toolchains` shows what is available.)

set -uo pipefail
cd "$(dirname "$0")/.."

FAILED=0
step() { printf '\n\033[1m== %s\033[0m\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAILED=1; }
ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; }

# ── Toolchain ────────────────────────────────────────────────────────────────
if ! rustc --version >/dev/null 2>&1; then
  installed=$(ls ~/.rustup/toolchains 2>/dev/null | head -1)
  if [ -n "$installed" ]; then
    export RUSTUP_TOOLCHAIN="$installed"
    echo "note: pinned toolchain unavailable; using installed '$installed'"
  else
    echo "error: no usable Rust toolchain found"
    exit 1
  fi
fi
rustc --version

# ── 1. Parse every Rust file ─────────────────────────────────────────────────
step "Rust syntax (all tracked .rs)"
count=0
for f in $(git ls-files '*.rs'); do
  count=$((count + 1))
  err=$(rustfmt --edition 2021 --emit stdout "$f" 2>&1 >/dev/null | grep -E '^error' | head -3)
  [ -n "$err" ] && { fail "$f"; echo "$err" | sed 's/^/       /'; }
done
[ "$FAILED" -eq 0 ] && ok "$count files parsed"

# ── 2. Workspace manifests ───────────────────────────────────────────────────
# --no-deps means cargo parses and validates every Cargo.toml and the workspace
# graph without resolving or downloading anything. Catches a malformed manifest
# or a dependency added to the wrong section.
step "Workspace manifests (cargo metadata --offline)"
if err=$(cargo metadata --offline --no-deps --format-version 1 2>&1 >/dev/null); then
  ok "$(git ls-files '*/Cargo.toml' Cargo.toml | wc -l | tr -d ' ') manifests valid"
else
  fail "cargo metadata"; echo "$err" | head -10 | sed 's/^/       /'
fi

# ── 2b. Module reachability ──────────────────────────────────────────────────
# A .rs file that no `mod` declaration reaches is never compiled by anything,
# so `cargo build` is silent about it. This check found three such files on this
# branch, two of which were the stated implementations of unfinished features.
step "Module reachability"
if orphans=$(python3 scripts/check-module-reachability.py); then
  ok "every .rs under src/ is reachable from a mod declaration"
else
  fail "unreachable module files (never compiled, tests never run):"
  echo "$orphans" | sed 's/^/       /'
fi

# ── 3. Standalone module tests ───────────────────────────────────────────────
# Modules whose code and tests only touch std can be compiled and run directly.
# Add a module here when it qualifies — it is the cheapest real verification
# available in an offline environment.
step "Standalone module tests (plain rustc)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

run_standalone() { # <label> <source> [shim-file]
  local label=$1 src=$2 shim=${3:-}
  local out="$TMP/$label.rs"
  if [ -n "$shim" ]; then
    # Inner doc comments (//!) must stay at the top of the file, so the shim is
    # spliced in after them rather than prepended.
    awk -v shimfile="$shim" '
      BEGIN { done = 0 }
      done == 0 && !/^\/\/!/ && !/^[[:space:]]*$/ {
        while ((getline line < shimfile) > 0) print line
        done = 1
      }
      { print }
    ' "$src" > "$out"
  else
    cp "$src" "$out"
  fi
  if ! rustc --edition 2021 --test "$out" -o "$TMP/$label" 2>"$TMP/$label.err"; then
    fail "$label (compile)"; sed 's/^/       /' "$TMP/$label.err" | head -20; return
  fi
  if "$TMP/$label" >"$TMP/$label.out" 2>&1; then
    ok "$label — $(grep -Eo '[0-9]+ passed' "$TMP/$label.out" | head -1)"
  else
    fail "$label (tests)"; tail -20 "$TMP/$label.out" | sed 's/^/       /'
  fi
}

# backoff only calls rand::random::<f64>() in next_delay; the schedule itself is
# pure, so a fixed draw is enough to exercise it.
cat > "$TMP/rand_shim.rs" <<'SHIM'
mod rand {
    pub trait Rnd { fn gen() -> Self; }
    impl Rnd for f64 { fn gen() -> f64 { 0.375 } }
    pub fn random<T: Rnd>() -> T { T::gen() }
}
SHIM

# backpressure only uses std; the tracing macros are the sole outside call.
cat > "$TMP/tracing_shim.rs" <<'SHIM'
macro_rules! log_noop { ($($t:tt)*) => {} }
mod tracing {
    pub(crate) use log_noop as debug;
    pub(crate) use log_noop as info;
    pub(crate) use log_noop as warn;
}
SHIM

run_standalone keymap       crates/miru-input/src/keymap.rs
run_standalone backoff      crates/miru-common/src/backoff.rs "$TMP/rand_shim.rs"
run_standalone backpressure crates/miru-host/src/backpressure.rs "$TMP/tracing_shim.rs"

# Modules with a small external surface can also run, via a generated harness
# that extracts the real protocol types (so they cannot drift) and stubs only
# the OS-touching calls. See scripts/offline_harness.py.
run_harness() { # <label> <module> [<type-source>:<Types...> | raw --types/--alias flags]
  local label=$1 module=$2; shift 2
  # A bare "<file>:<Types>" third argument is shorthand for a single --types.
  local -a extra=()
  if [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; then extra=(--types "$1"); shift; fi
  extra+=("$@")
  if ! python3 scripts/offline_harness.py --module "$module" --out "$TMP/$label.rs" \
        "${extra[@]}" 2>"$TMP/$label.gen"; then
    fail "$label (harness)"; sed 's/^/       /' "$TMP/$label.gen" | head -5; return
  fi
  if ! rustc --edition 2021 --test "$TMP/$label.rs" -o "$TMP/$label" 2>"$TMP/$label.err"; then
    fail "$label (compile)"; grep -E '^error' -A 4 "$TMP/$label.err" | head -20 | sed 's/^/       /'; return
  fi
  if "$TMP/$label" >"$TMP/$label.out" 2>&1; then
    ok "$label — $(grep -Eo '[0-9]+ passed' "$TMP/$label.out" | head -1)"
  else
    fail "$label (tests)"; tail -20 "$TMP/$label.out" | sed 's/^/       /'
  fi
}

run_harness input_handler crates/miru-host/src/input_handler.rs \
  crates/miru-common/src/message.rs:MouseButton,InputKind,InputEvent,ClipboardFormat,ClipboardSync

run_harness qos_bbr crates/miru-host/src/qos_bbr.rs \
  crates/miru-common/src/message.rs:QosUpdate,QosHint

run_harness codec_negotiation crates/miru-common/src/codec.rs \
  crates/miru-common/src/message.rs:VideoCodec,AudioCodec

# --types omitted: these need only the anyhow/tracing stubs.
# miru-sandbox was tried and does not qualify — it declares platform submodules
# (`mod linux;`) that a single-file harness cannot supply.
run_harness wol          crates/miru-common/src/wol.rs
run_harness parent_check crates/miru-mcp/src/parent_check.rs

# ffmpeg_enc is behind a feature that is off by default, so cargo would not
# build it even once the registry is reachable. Its codec table is pure, and the
# tests cross-check it against the real HwEncoder::codecs().
# The FFI is exercised for real further down; here it is stubbed so the codec
# table's cross-check against HwEncoder::codecs() can run without libavcodec.
cat > "$TMP/ffmpeg_ffi_stub.rs" <<'SHIM'
#[allow(unused)]
mod platform {
    pub mod ffmpeg_ffi {
        pub struct FfmpegEncoder;
        impl FfmpegEncoder {
            pub fn open(_n: &str, _w: u32, _h: u32, _f: u8, _b: u32) -> Option<Self> { None }
            pub fn send(&mut self, _i: &[u8], _k: bool) -> Result<(), i32> { Ok(()) }
            pub fn receive(&mut self) -> Result<Option<Packet>, i32> { Ok(None) }
            pub fn set_bitrate(&mut self, _k: u32) {}
        }
        pub struct Packet { pub data: Vec<u8>, pub keyframe: bool }
        pub fn encoder_compiled_in(_n: &str) -> bool { false }
    }
}
SHIM

run_harness ffmpeg_enc crates/miru-codec/src/ffmpeg_enc.rs \
  --prelude "$TMP/ffmpeg_ffi_stub.rs" \
  --types crates/miru-common/src/message.rs:VideoCodec \
  --types crates/miru-codec/src/hw.rs:HwEncoder \
  --types crates/miru-codec/src/lib.rs:EncodedPacket \
  --types crates/miru-codec/src/encoder.rs:EncoderBackend \
  --alias encoder --alias hw

run_harness hw_probe crates/miru-codec/src/hw.rs \
  crates/miru-common/src/message.rs:VideoCodec

# Native FFI: the libavcodec shim is C, so it needs a compile+link step rather
# than a generated harness. Real encode, real library — the hardware encoders
# take the identical path, only the codec name differs.
step "libavcodec FFI (real encode)"
if ! pkg-config --exists libavcodec libavutil 2>/dev/null; then
  echo "  skip (no libavcodec-dev — apt-get install libavcodec-dev libavutil-dev)"
elif ! command -v cc >/dev/null 2>&1; then
  echo "  skip (no C compiler)"
else
  if cc -c -O2 -fPIC crates/miru-codec/csrc/miru_ffmpeg.c -o "$TMP/miru_ffmpeg.o" \
       $(pkg-config --cflags libavcodec libavutil) 2>"$TMP/shim.err" \
     && ar rcs "$TMP/libmiru_ffmpeg.a" "$TMP/miru_ffmpeg.o"; then
    if rustc --edition 2021 --test crates/miru-codec/src/platform/ffmpeg_ffi.rs \
         -L "$TMP" -l static=miru_ffmpeg -o "$TMP/ffmpeg_ffi" 2>"$TMP/ffi.err"; then
      if "$TMP/ffmpeg_ffi" >"$TMP/ffi.out" 2>&1; then
        ok "ffmpeg_ffi — $(grep -Eo '[0-9]+ passed' "$TMP/ffi.out" | head -1)"
      else
        fail "ffmpeg_ffi (tests)"; tail -20 "$TMP/ffi.out" | sed 's/^/       /'
      fi
    else
      fail "ffmpeg_ffi (compile)"; grep -E '^error' -A4 "$TMP/ffi.err" | head -20 | sed 's/^/       /'
    fi
  else
    fail "miru_ffmpeg.c (compile)"; head -10 "$TMP/shim.err" | sed 's/^/       /'
  fi
fi

# The portal client is C over GDBus + libpipewire. Compiling and running it
# checks the negotiation logic and the typed error path; whether a real capture
# can start depends on a live compositor, which the test detects and adapts to.
step "xdg-desktop-portal ScreenCast client"
if ! pkg-config --exists gio-2.0 libpipewire-0.3 2>/dev/null; then
  echo "  skip (no glib/pipewire dev — apt-get install libglib2.0-dev libpipewire-0.3-dev)"
elif ! command -v cc >/dev/null 2>&1; then
  echo "  skip (no C compiler)"
else
  PKGS="gio-2.0 glib-2.0 gobject-2.0 libpipewire-0.3"
  # miru_pw_testsrc.c is test scaffolding: the gate links it, build.rs never does.
  if cc -c -O2 -fPIC crates/miru-capture/csrc/miru_portal.c -o "$TMP/miru_portal.o" \
       $(pkg-config --cflags $PKGS) 2>"$TMP/portal.err" \
     && cc -c -O2 -fPIC crates/miru-capture/csrc/miru_pw_testsrc.c -o "$TMP/miru_testsrc.o" \
       $(pkg-config --cflags $PKGS) 2>>"$TMP/portal.err" \
     && ar rcs "$TMP/libmiru_portal.a" "$TMP/miru_portal.o" "$TMP/miru_testsrc.o"; then
    if rustc --edition 2021 --test crates/miru-capture/src/platform/portal_ffi.rs \
         -L "$TMP" -l static=miru_portal $(pkg-config --libs-only-L $PKGS) \
         -o "$TMP/portal_ffi" 2>"$TMP/pffi.err"; then
      if "$TMP/portal_ffi" >"$TMP/pffi.out" 2>&1; then
        ok "portal_ffi — $(grep -Eo '[0-9]+ passed' "$TMP/pffi.out" | head -1)"
      else
        fail "portal_ffi (tests)"; tail -20 "$TMP/pffi.out" | sed 's/^/       /'
      fi
    else
      fail "portal_ffi (compile)"; grep -E '^error' -A4 "$TMP/pffi.err" | head -20 | sed 's/^/       /'
    fi
  else
    fail "miru_portal.c (compile)"; head -10 "$TMP/portal.err" | sed 's/^/       /'
  fi
fi

# ── 4. Frontend ──────────────────────────────────────────────────────────────
step "Frontend (tsc + vitest)"
UI=crates/miru-client/ui
if [ -d "$UI/node_modules" ]; then
  (cd "$UI" && npm run --silent typecheck) && ok "tsc" || fail "tsc"
  (cd "$UI" && npm test --silent >"$TMP/vitest.out" 2>&1) \
    && ok "vitest — $(grep -Eo 'Tests +[0-9]+ passed' "$TMP/vitest.out" | head -1)" \
    || { fail "vitest"; tail -20 "$TMP/vitest.out" | sed 's/^/       /'; }
else
  echo "  skip (no node_modules — run: cd $UI && npm install)"
fi

# ── Result ───────────────────────────────────────────────────────────────────
echo
if [ "$FAILED" -eq 0 ]; then
  printf '\033[32mAll offline checks passed.\033[0m Still run `cargo test --workspace` before merging.\n'
else
  printf '\033[31mOffline checks failed.\033[0m\n'
fi
exit "$FAILED"

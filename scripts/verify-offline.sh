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
# These checks do not need any dependency to be downloaded:
#
#   1. Parse every tracked .rs file (rustfmt parses; it resolves nothing).
#   2. Validate every Cargo.toml and the workspace graph (cargo metadata
#      --no-deps resolves nothing, so it needs no registry).
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

run_standalone keymap  crates/miru-input/src/keymap.rs
run_standalone backoff crates/miru-common/src/backoff.rs "$TMP/rand_shim.rs"

# Modules with a small external surface can also run, via a generated harness
# that extracts the real protocol types (so they cannot drift) and stubs only
# the OS-touching calls. See scripts/offline_harness.py.
run_harness() { # <label> <module> <type-source>:<Types...>
  local label=$1 module=$2 types=$3
  if ! python3 scripts/offline_harness.py --module "$module" --out "$TMP/$label.rs" \
        --types "$types" 2>"$TMP/$label.gen"; then
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

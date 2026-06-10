# Miru — Development Workflow Notes

This file captures the dev-loop conventions for working on Miru across sessions
in an ephemeral sandbox. It is **dev tooling documentation**, not product docs.

## Session start (always)

```bash
source scripts/dev-bootstrap.sh
```

This single command (idempotent):
- restores the project from `/mnt/user-data/outputs/miru` if missing
- installs the Rust toolchain + system libs if absent
- exports a fast/small build profile (`opt-level=0`, line-tables debuginfo)
- frees disk if `target/` ballooned
- starts Xvfb `:99` and exports `DISPLAY`

## Session end (always)

**Sync changed files to `/mnt/user-data/outputs/miru` before finishing.**
Files not synced are LOST on the next session reset. This has bitten us
repeatedly — e.g. `local_bridge.rs`, `security_integration.rs`, and the
revocation wiring all had to be re-implemented because they were never synced.

```bash
# Sync everything that changed (safest):
rsync -a --exclude target --exclude .git /home/claude/miru/ /mnt/user-data/outputs/miru/
# Then present key files so they're visible to the user.
```

## Testing efficiently

`cargo test` from cold often exceeds the command timeout because it rebuilds
from scratch. Pattern that works:

```bash
# 1. Build the test binary (long, but one-time):
cargo test -p <crate> --test <name> --no-run
# 2. Run the produced binary directly (fast):
./target/debug/deps/<name>-<hash>
```

For unit tests, build per-crate rather than `--workspace` to avoid timeouts.

## Recurring gotchas

| Symptom | Cause | Fix |
|---------|-------|-----|
| `No space left on device` during link | `target/` > ~4 GB (debug=true rlibs) | `rm -rf target/debug/incremental target/debug/deps/*.rlib` |
| Test hangs forever | stale `miru-signal` holding ports from a timeout-killed test | `pkill -9 -f miru-signal` |
| `$DISPLAY not set` crash on host | no Xvfb | `source scripts/dev-bootstrap.sh` (starts it) |
| Xvfb "server already running" | `:99` lock left behind | handled by bootstrap (`rm -f /tmp/.X99-lock`) |
| Port conflict in relay test | fixed 21115/21117 | tests use `MIRU_RDV_PORT`/`MIRU_RELAY_PORT` (31115/31117) |

## Test inventory (run all before declaring "green")

```bash
cargo test -p miru-common -p miru-auth -p miru-agent \
           -p miru-constellation -p miru-mcp -p miru-sandbox \
           -p miru-transparency --lib          # unit (7 crates)
cargo test -p miru-transport                    # handshake + e2e
cargo test --bin miru-host                      # host incl. agent gate
cargo test -p miru-agent --test security_integration
cargo test -p miru-mcp --test local_bridge_e2e  # needs DISPLAY
cargo test -p miru-signal --test relay_e2e      # spawns signal server
```

## Warning policy

All three binaries must be **0 warnings**:
```bash
for b in miru-host miru-mcp miru-signal; do cargo build --bin $b 2>&1 | grep -c '^warning:'; done
```
Scaffold modules not yet wired use `#[allow(dead_code)]` at the `mod` declaration
in `crates/miru-host/src/main.rs`, with a comment pointing to the roadmap.

## Profile note

`Cargo.toml` `[profile.dev]` keeps `debug = true` for contributors/CI (they
expect full debuginfo). The sandbox override lives only in env vars exported by
`dev-bootstrap.sh`, so the committed product config is never polluted.

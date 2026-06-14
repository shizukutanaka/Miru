# Unwrap / expect Triage

Status: **2 production unwrap/expect calls** — both verified Category A.
Category B (mutex poison) and Category C (genuine runtime failures) eliminated.

## Category definitions

- **A** — Provably-safe at compile time (const values, post-guard, fixed-size types, unrecoverable startup).
- **B** — Mutex poison (cooperative; data is append-only or atomic; recovery = same as panic). — **0 remaining**.
- **C** — Genuine failure path. — **0 remaining**.

## Inventory

### A: Provably safe (2 sites)

| File | Line | Reason |
|------|------|--------|
| `miru-client/src-tauri/src/main.rs:47` | `.expect("Tauri app failed")` | Tauri app startup failure is unrecoverable; no meaningful error to propagate |
| `miru-codec/src/ffmpeg_enc.rs:86` | commented-out `unwrap()` in dead code | Inside a `//` comment — counted by the awk budget check but unreachable |

### B: Mutex poison — none remaining ✅

All former Category B sites were converted:
- `miru-agent/src/audit.rs` → `unwrap_or_else(p.into_inner())` + warn! log
- `miru-input/src/platform/linux.rs` → match with warn! log

### C: Genuine — none remaining ✅

All former Category C sites were converted:
- Windows DXGI (6) → `.ok_or_else(|| anyhow::anyhow!(...))?`
- `capture_loop.rs` encoder init → `if encoder.is_none() { match … { Err => continue } }`
- `state.rs` identity load → `unwrap_or_else(|_| DeviceIdentity::generate())`
- `client/session.rs` decoder → `if decoder.is_none() { match … { Err => continue } }`
- `miru-transparency/src/lib.rs:55` → `commitment()` now returns `Result<[u8; 32]>`, uses `?`

## Rule

Production code **must not panic on adversarial input or recoverable system errors**.
Category A panics that encode invariants are acceptable and document why a value
cannot fail at that point. Category C is permanently banned.

## CI enforcement

`.github/workflows/ci.yml` `crypto-gate` job checks:
- Budget ≤ 5 unwrap/expect in production (excludes tests, benches, fuzz)
- No non-strict `.verify()` in production
- No `danger_accept_invalid_*` APIs
- `SessionCipher` must not derive `Clone`

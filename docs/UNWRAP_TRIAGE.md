# Unwrap / expect Triage

Status: **24 production unwrap/expect calls** — all verified Category A or B.
Category C (genuine runtime failures) has been eliminated.

## Category definitions

- **A** — Provably-safe at compile time (const values, post-guard, fixed-size types).
- **B** — Mutex poison (cooperative; data is append-only or atomic; recovery = same as panic).
- **C** — Genuine failure path. ZERO remaining.

## Inventory

### A: Provably safe (14 sites)

| File | Line | Reason |
|------|------|--------|
| `miru-auth/src/lib.rs:109` | `NonZeroU32::new(100_000)` | const 100_000 ≠ 0 |
| `miru-auth/src/argon.rs:31` | `Argon2::new(params)` | hardcoded valid params |
| `miru-transparency/src/lib.rs:54` | `to_vec(Serialize)` | derived Serialize, no map keys |
| `miru-transparency/src/lib.rs:88` | `getrandom` | OsRng — OS failure = system dead |
| `miru-transport/src/handshake.rs:229,231` | HKDF 32-byte expand | max 8160B; 32B trivially fits |
| `miru-transport/src/quic.rs:208` | `Duration::from_secs(60).try_into()` | 60s fits VarInt |
| `miru-transport/src/quic.rs:210` | `QuicClientConfig::try_from(rustls)` | config we just built |
| `miru-mcp/src/lib.rs:70,79,94` | `to_value(struct)` | internal types, derived Serialize |
| `miru-mcp/src/server.rs:316` | `.first.unwrap()` | guarded by `.is_none()` above |
| `miru-agent/src/audit.rs:56` | `to_vec(AuditEntry)` | internal struct |
| `miru-agent/src/token.rs:230` | `to_vec(AgentTokenPayload)` | internal struct |
| `miru-common/src/crypto.rs:86,88,119` | HKDF fill, slice try_into | size-checked by type |
| `miru-host/src/qos_bbr.rs:152,154,166` | `.back()/.nth(2)` | `len()>=3` guard above |
| `miru-host/src/capture_loop.rs:86` | `.as_mut().expect("just inserted above")` | `if encoder.is_none()` guard |
| `miru-client/src-tauri/src/session.rs:143` | `.as_mut().expect("just inserted above")` | same pattern |

### B: Mutex poison — cooperative (3 sites)

| File | Rationale |
|------|-----------|
| `miru-agent/src/audit.rs:125,126,142` | Append-only log; poison = panic elsewhere; `unwrap_or_else(into_inner)` |

### C: Genuine — none remaining ✅

All former Category C sites were converted:
- Windows DXGI (6) → `.ok_or_else(|| anyhow::anyhow!(...))?`
- `capture_loop.rs` encoder init → `if encoder.is_none() { match … { Err => continue } }`
- `state.rs` identity load → `unwrap_or_else(|_| DeviceIdentity::generate())`
- `client/session.rs` decoder → `if decoder.is_none() { match … { Err => continue } }`

## Rule

Production code **must not panic on adversarial input or recoverable system errors**.
Category A panics that encode invariants are acceptable and document why a value
cannot fail at that point. Category C is permanently banned.

## CI enforcement

`.github/workflows/ci.yml` `crypto-gate` job checks:
- Budget ≤ 24 unwrap/expect in production (excludes tests)
- No non-strict `.verify()` in production
- No `danger_accept_invalid_*` APIs
- `SessionCipher` must not derive `Clone`

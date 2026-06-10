# Miru Threat Model

This document specifies Miru's security posture using STRIDE per component.
It is the source of truth for what Miru defends against and — equally
important — what is explicitly out of scope.

Companion documents:
- [`SECURITY.md`](../SECURITY.md) — how to report issues
- [`docs/CRYPTO.md`](./CRYPTO.md) — cryptographic invariants
- [`docs/postmortems/`](./postmortems/) — historical incidents

## 1. System overview

Miru is a P2P remote desktop with three distinguishing surfaces beyond a
conventional remote-control product:

```
              ┌──────────────────────────────────────────┐
   AI Agent   │  Claude Desktop / Cursor / Codex / etc.  │
   (LLM)      └──────────────┬───────────────────────────┘
                             │ stdio JSON-RPC (MCP)
                             ▼
              ┌──────────────────────────────────────────┐
              │              miru-mcp                    │  ← agent surface
              │  parent-process check, rate limit, ACL   │
              └──────────────┬───────────────────────────┘
                             │ in-process (token-gated)
                             ▼
              ┌──────────────────────────────────────────┐
              │        miru-host (capture)               │
              │  capture, encode, mTLS, audit log        │
              └──────────────┬───────────────────────────┘
                             │ X25519 + AEAD over QUIC / WS-relay
                             ▼
              ┌──────────────────────────────────────────┐
              │            Network (peer/relay)          │
              └──────────────┬───────────────────────────┘
                             ▼
              ┌──────────────────────────────────────────┐
              │              miru-client                 │
              │  Tauri WebView, React UI                 │
              └──────────────────────────────────────────┘
```

**Trust boundaries** (each → indicates "do not trust without verification"):

- LLM provider → MCP server → host (token + capability check)
- Network → peer (mTLS + Ed25519 fingerprint pin)
- WebView → Rust core (Tauri capabilities + IPC isolation)
- Local process → Ed25519 device key (OS keystore boundary)

## 2. Assets

| Asset                              | Sensitivity | Where stored                              |
|------------------------------------|-------------|-------------------------------------------|
| Ed25519 device private key         | Critical    | OS keystore / passphrase-encrypted file   |
| Argon2id PIN verifier              | High        | Local disk (`~/.miru/peers.json`)         |
| Per-session AEAD keys              | Critical    | RAM only, `Zeroize`-on-drop               |
| Capability tokens                  | High        | RAM (issued); UI clipboard during transfer |
| Screen frame buffers               | Critical    | RAM, `mlock`-pinned, never swapped        |
| Audit log                          | High        | Local disk, append-only, hash-chained     |
| TOFU fingerprint pin log           | High        | Local disk, append-only, hash-chained     |
| Revocation list                    | Medium      | Local disk, append-only                   |

## 3. Actors

| Actor                          | Trust          | Threat                                    |
|--------------------------------|----------------|-------------------------------------------|
| Legitimate user                | Trusted        | (the principal we serve)                  |
| Authorised peer                | Conditional    | Verified by Ed25519 fingerprint pin       |
| AI agent (LLM)                 | Conditional    | May be prompt-injected; may emit tool calls outside intent |
| MCP host (Claude Desktop, etc.)| Conditional    | Trusted to deliver our binary's stdio    |
| Model provider (Anthropic, …)  | Conditional    | Trusted not to backdoor responses; may be subpoenaed |
| Network attacker (passive)     | Adversary      | Cannot read; may read mDNS / SNI         |
| Network attacker (active MITM) | Adversary      | Cannot impersonate (mTLS + pin); may DoS |
| Local malware (same UID)       | Adversary      | Can read user-readable files; sandboxed by OS |
| Local malware (root)           | Adversary      | OUT OF SCOPE — system is fully compromised |
| Build-pipeline attacker        | Adversary      | Mitigated by Sigstore + Trusted Publishing + SHA-pinned actions |
| Dependency author (compromised)| Adversary      | Mitigated by `cargo-vet` + 7-day cooldown |
| Insider at Miru org            | Adversary      | Mitigated by 2-of-N release signing (v1.0) |

## 4. STRIDE per component

### 4.1 Capture host (`miru-capture`, `miru-host`)

| | Threat | Mitigation | Status |
|-|--------|------------|--------|
| **S** | Malicious peer impersonates a known device | Ed25519 identity sigs in handshake; `verify_strict` (ZIP-215) | ✅ v0.1 |
| **T** | Frame buffer tampering in transit | ChaCha20-Poly1305 AEAD with per-direction keys | ✅ v0.1 |
| **R** | Peer denies issuing input events | Audit log of incoming InputEvents (planned v0.3) | ⚠️ v0.3 |
| **I** | Frame buffer leakage via swap / core dump | `mlock`-pinned arena; `prctl(PR_SET_DUMPABLE,0)` | ⚠️ v1.0 |
| **D** | Encoder DoS via crafted frames | Decoder bounds checks, frame size cap (4K max in v0.1) | ⚠️ v1.0 |
| **E** | Host privilege escalation via capture API | Drop privileges after init; Linux `landlock` + `seccomp` | ⚠️ v1.0 |

### 4.2 Transport (`miru-transport`)

| | Threat | Mitigation | Status |
|-|--------|------------|--------|
| **S** | MITM with bogus cert | mTLS-only with TOFU pin; refuse `danger_accept_invalid_certs` (CI grep) | ✅ v0.1 |
| **T** | Replay of recorded ciphertext | 128-bit sliding-window replay protection per direction | ✅ v0.1 |
| **R** | Relay denies routing a session | Sigstore Rekor anchor of relay routing tickets | ⚠️ v1.0 |
| **I** | Relay sees plaintext | mTLS terminates at peers; relay sees only opaque ciphertext | ✅ v0.1 |
| **D** | Relay flooded with false connections | Rate limit + token-bound binding; PoW for unauthenticated probes (v1.0) | ⚠️ v1.0 |
| **E** | Cross-direction key reuse → AEAD break | `derive_subkey("miru-h2v-v1")` / `("miru-v2h-v1")` HKDF split | ✅ v0.1 |

### 4.3 MCP server (`miru-mcp`)

| | Threat | Mitigation | Status |
|-|--------|------------|--------|
| **S** | Local malware spawns miru-mcp directly | Parent-process allowlist (Claude Desktop, Cursor, Codex, …) | ✅ v0.1 |
| **T** | LLM mutates tool schemas to widen capability | Tool manifest hash pinned at install; refuse drift | ⚠️ v1.0 |
| **R** | LLM denies executing a destructive action | Hash-chained audit log with redacted args | ✅ v0.1 |
| **I** | Audit log leaks plaintext keystrokes / clipboard | Redaction matrix (`miru_agent::redact`): SHA-256 + length only | ✅ v0.1 |
| **D** | Runaway agent loop (rapid clicks, file enum) | Per-capability token-bucket + 24h daily cap | ✅ v0.1 |
| **E** | Token compromise → unbounded use | TTL ≤15 min, per-task scope, revocation list (panic rotation) | ✅ v0.1 |

### 4.4 Tauri WebView (`miru-client`)

| | Threat | Mitigation | Status |
|-|--------|------------|--------|
| **S** | Remote-origin iframe accesses IPC | Strict CSP `frame-src 'none'`; isolation pattern | ✅ v0.1 |
| **T** | XSS via injected peer name / clipboard | Framework text-binding only; eslint-plugin-security | ✅ v0.1 |
| **R** | UI redress (transparent overlay) | Window-opacity check on input injection target | ⚠️ v1.0 |
| **I** | WebView memory dump leaks tokens | Tokens displayed once, stored only in Rust-side state | ✅ v0.1 |
| **D** | Malformed protocol message crashes UI | Schema-validated IPC payloads, panic-free deserialisation | ⚠️ v1.0 |
| **E** | `shell:execute` escape | Plugin disabled in capabilities; Rust ≥1.77.2 (BatBadBut) | ✅ v0.1 |

### 4.5 Constellation / discovery (`miru-constellation`, `miru-discovery`)

| | Threat | Mitigation | Status |
|-|--------|------------|--------|
| **S** | mDNS responder spoofs `_miru._tcp.local.` | Discovery is informational only; Ed25519 pin required before any session bytes | ✅ v0.1 |
| **T** | Forged constellation membership | Ed25519 sig over join blob; `verify_strict` | ✅ v0.1 |
| **R** | Member denies role-grant action | Constellation token chain (signed by constellation key) | ✅ v0.1 |
| **I** | mDNS leaks device topology to LAN | Document trade-off in README; allow opt-out | ✅ v0.1 |
| **D** | Flood of join requests | Rate limit + per-source quotas | ⚠️ v1.0 |
| **E** | Member elevates to constellation-owner | Owner role gated by Argon2id-derived key, never delegated | ✅ v0.1 |

### 4.6 Transparency log (`miru-transparency`)

| | Threat | Mitigation | Status |
|-|--------|------------|--------|
| **S** | Forged co-signed commitment | Verify both host + viewer Ed25519 sigs over canonical metadata | ✅ v0.1 |
| **T** | Tamper with local Merkle log | SHA-256 chain; periodic Rekor anchor (v1.0) | ⚠️ v1.0 |
| **R** | Peer denies session metadata | CoSignedCommitment is non-repudiable | ✅ v0.1 |
| **I** | Commitment leaks PII | Commitment includes only fingerprints + timestamps | ✅ v0.1 |
| **D** | Log file size unbounded | Rotate at 100 MB; archived files retain hash chain | ⚠️ v1.0 |
| **E** | Commitment used as auth token | Commitments are evidence, not capability; never accepted in lieu of token | ✅ v0.1 |

## 5. Cryptographic invariants

These properties MUST hold; violation is a critical bug:

1. **No nonce reuse**. Each AEAD nonce under a given key is used exactly once.
   Enforced by per-direction monotonic counter; verified by property test.
2. **No key reuse across directions**. Host→viewer and viewer→host use
   distinct keys derived via HKDF info `miru-h2v-v1` / `miru-v2h-v1`.
3. **Forward secrecy**. Long-term keys never participate in session encryption.
   X25519 ephemeral keypairs derive the session secret.
4. **Signature verification is strict (ZIP-215)**. All `verify` calls in
   production use `VerifyingKey::verify_strict`. CI greps for `verify(`
   without `_strict` and fails the build.
5. **Constant-time comparison for secrets**. All MAC/tag/fingerprint/token
   comparisons use `subtle::ConstantTimeEq`.
6. **Argon2id parameters versioned**. PIN verifiers carry parameter version;
   future cost increases are automatic on next reset.
7. **Replay protection on decrypt**. 128-bit sliding window per cipher;
   refused before AEAD CPU is spent.

## 6. Out of scope

Documenting these honestly is part of the threat model:

- **Root-level malware**. If `root`/SYSTEM is compromised, no application
  defence holds. Use OS-level controls (SELinux, full-disk encryption,
  Secure Boot).
- **Hardware attacks**. Cold-boot, JTAG, BadUSB, etc. Use hardware security
  keys for the device identity at v2.0.
- **Supply-chain attack on the Rust toolchain**. Track upstream RustSec.
- **Social engineering of the legitimate user**. Documentation can mitigate
  but cannot prevent (e.g. user copies a token into a phishing site).
- **Quantum computer that breaks X25519/Ed25519 today**. Hybrid PQC
  (`X25519MLKEM768`) targeted for v1.0. Until then, *harvest-now-decrypt-later*
  is documented in PRIVACY.md.
- **Indirect prompt injection via screen content** that bypasses the
  destructive-action confirmation gate. Anthropic's published numbers
  (88% blocked baseline → 1.4% post-mitigation) suggest residual risk.
  Confirmation-gate is the deterministic backstop.

## 7. Open questions

These are tracked but unresolved; PRs welcome.

- How to detect a compromised LLM provider in real time without per-call
  attestation? (Behavioural anomaly detection on action streams — v2.0.)
- Practical reproducible-build target across macOS notarisation?
- Threshold signing for releases without operational complexity for
  a single-maintainer project? (2-of-2 with a hardware backup at v1.0.)

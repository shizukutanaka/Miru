# Miru Cryptographic Specification

This document fixes every cryptographic invariant for Miru. It is the
contract between protocol implementations: any change here is a wire-protocol
change requiring a version bump.

Companion documents:
- [`THREAT_MODEL.md`](./THREAT_MODEL.md) — what we defend against
- [`SECURITY.md`](../SECURITY.md) — how to report flaws

## 1. Algorithm choices

| Purpose                       | Algorithm                          | Crate (Rust)             |
|-------------------------------|------------------------------------|--------------------------|
| ECDH key exchange             | X25519                             | `x25519-dalek` 2.x       |
| Identity signature            | Ed25519 (ZIP-215)                  | `ed25519-dalek` 2.x      |
| Symmetric AEAD                | ChaCha20-Poly1305                  | `chacha20poly1305` 0.10  |
| Key derivation                | HKDF-SHA256                        | `ring::hkdf` 0.17        |
| Hash                          | SHA-256                            | `ring::digest`           |
| Password hash (PIN)           | Argon2id                           | `argon2` 0.5             |
| Constant-time compare         | (subtle compare)                   | `subtle` 2.5             |
| Memory zeroization            | `zeroize` + `secrecy`              | `zeroize` 1.7            |
| TLS                           | TLS 1.3 only                       | `rustls` 0.23 (`aws-lc-rs`) |
| Hybrid PQC (v1.0)             | X25519MLKEM768                     | `rustls-post-quantum`    |

**Forbidden** (never accept):
- TLS 1.2 or below
- RC4, MD5, SHA-1 in any production path
- AES-CBC without HMAC (no MAC-then-encrypt)
- Ed25519 batch verification on attacker-controlled inputs
- `Verifier::verify` (non-strict) — use `verify_strict` only
- Custom crypto primitives

## 2. Handshake

### 2.1 Wire transcript

```
Viewer                                                     Host
──────                                                     ────
  │                                                          │
  │ ─── Hello {                                               │
  │       version: u32,                                       │
  │       role: Viewer,                                       │
  │       eph_pub_v: [u8;32],          (X25519)               │
  │       id_pub_v: [u8;32],           (Ed25519)              │
  │       sig_v: [u8;64],   = sign(id_priv_v, challenge_v)    │
  │       features: FeatureSet,                               │
  │     }                                                     │
  │ ─────────────────────────────────────────────────────────►│
  │                                                          │
  │                                          ┌───── verify_strict(id_pub_v, challenge_v, sig_v)
  │                                          ├───── codec = negotiate(host_features, viewer_features)
  │                                          ├───── eph_v_priv = X25519::random()
  │                                          ├───── session_id = UUID::v4()
  │                                          ├───── shared = X25519(eph_v_priv, eph_pub_v)
  │                                          └───── session_key = HKDF(shared, salt=session_id, info="miru-session-v1")
  │                                                          │
  │ ◄──── HelloAck {                                          │
  │         session_id: UUID,                                 │
  │         eph_pub_h: [u8;32],                               │
  │         id_pub_h: [u8;32],                                │
  │         sig_h: [u8;64], = sign(id_priv_h, challenge_h)    │
  │         selected_codec: VideoCodec,                       │
  │         selected_audio: AudioCodec,                       │
  │       }                                                   │
  │                                                          │
  ├─── verify_strict(id_pub_h, challenge_h, sig_h)            │
  ├─── shared = X25519(eph_priv_v, eph_pub_h)                 │
  └─── session_key = HKDF(shared, salt=session_id, info="miru-session-v1")
                                                             │
              From here both sides do directional split:
              tx_v = HKDF(session_key, info="miru-v2h-v1")
              rx_v = HKDF(session_key, info="miru-h2v-v1")
              tx_h = HKDF(session_key, info="miru-h2v-v1")
              rx_h = HKDF(session_key, info="miru-v2h-v1")
              (host's tx == viewer's rx, by construction)
```

### 2.2 Challenges

Each side signs a challenge that BINDS its identity public key to its
ephemeral public key, preventing key compromise impersonation:

```
challenge_v = b"miru-handshake-v1" || eph_pub_v || id_pub_v
challenge_h = b"miru-handshake-v1" || eph_pub_h || id_pub_h
```

**Why no per-session nonce?** The ephemeral pubkey is itself a per-session
random value. Replaying a captured Hello reveals nothing because the
attacker doesn't hold the corresponding ephemeral private key, so no
shared secret can be derived. Forward secrecy is preserved.

### 2.3 verify_strict (ZIP-215)

`ed25519_dalek::VerifyingKey::verify_strict` is mandatory. It additionally:

- Rejects small-order public keys (cofactor 8 attacks)
- Rejects non-canonical R encodings (malleability)
- Eliminates the divergence between batched and unbatched verification

The non-strict `verify` is forbidden in production code; CI greps for it.

## 3. Per-session encryption

### 3.1 SessionCipher invariants

- **No `Clone`**. Cloning a cipher would duplicate the send counter under
  the same key, producing a textbook nonce-reuse break of ChaCha20-Poly1305.
- **One direction per cipher**. Multiple streams within a direction MUST
  use `derive_subkey(info)` to obtain independent keys.
- **Counter never wraps**. `u64::MAX = 18.4 quintillion`; even at 1M
  encryptions/second this exceeds 500,000 years. Re-keying triggers well
  before that bound (see §4).
- **Replay window**. Receiver tracks `(highest_seq, bitmap)` per cipher.
  Default window: 128 sequence numbers. Anything outside the window is
  rejected without spending AEAD CPU.

### 3.2 Frame format

```
+----------------+--------------------------------+
| seq u64 LE     | ciphertext + 16-byte tag       |
+----------------+--------------------------------+
       8 bytes              variable
```

The 12-byte ChaCha20-Poly1305 nonce is constructed as
`seq.to_le_bytes() || [0;4]`. This embeds the direction implicitly (each
direction has a distinct key, so even identical seq values produce
distinct AEAD outputs).

### 3.3 HKDF info strings

Versioned and structured. Bumping the version forces a clean break.

| Use                          | Info string                           |
|------------------------------|---------------------------------------|
| Session key from shared      | `b"miru-session-v1"`                  |
| Subkey root salt             | `b"miru-subkey-v1"`                   |
| Host→viewer direction        | `b"miru-h2v-v1"`                      |
| Viewer→host direction        | `b"miru-v2h-v1"`                      |
| Audit-chain key (v1.0)       | `b"miru/v1 audit-chain"`              |

### 3.4 Key rotation

| Trigger                       | Action                            | Status      |
|-------------------------------|-----------------------------------|-------------|
| Counter near `u64::MAX`       | Soft re-keying via fresh ECDH     | ⚠️ v1.0     |
| 24-hour session age           | Soft re-keying                    | ⚠️ v1.0     |
| User clicks "panic"           | Hard rotation (invalidate session)| ✅ v0.1     |
| Suspected token compromise    | Hard rotation + revocation list   | ✅ v0.1     |

## 4. Token format (capability JWT-equivalent)

```
miru-agent.<base64url(payload)>.<base64url(ed25519_sig)>
```

Payload (JSON, canonicalised):

```json
{
  "jti": "<uuid>",
  "iss": "<base64url(issuer_ed25519_pubkey)>",
  "sub": "<short label>",
  "iat": 1234567890,
  "exp": 1234568790,
  "caps": ["screen_read", "pointer_move", ...],
  "scope": null
}
```

### 4.1 Invariants

- **TTL ≤ 15 min** (`MAX_TTL_SECS` in `miru_agent::token`). Issue clamps
  any larger value. Documented rationale in security playbook §3.
- **Verified with `verify_strict`** before payload deserialisation.
  Never deserialise untrusted bytes.
- **Issuer match**: payload `iss` must equal base64 of the verifying key
  used for `verify_strict`. Mismatch = reject.
- **Revocation check**: JTI must not appear in the revocation list.
- **Per-task scope** (v1.0): tokens encode user-task ID; reused token
  in a different task = reject.

## 5. PIN derivation (peer pairing)

Argon2id parameters, all explicit (no defaults):

```
m_cost = 65 536  (64 MiB)
t_cost = 3
p      = 1
salt   = 16 bytes from getrandom
output = 32 bytes
```

Stored as PHC string with a versioned tag so future cost increases are
detectable on first verify.

## 6. Audit log

### 6.1 Format

JSONL, append-only. Each line:

```json
{
  "seq": N,
  "prev_hash": "<hex sha-256 of previous entry>",
  "timestamp_ms": ...,
  "token_jti": "<uuid>",
  "capability": "screen_read",
  "action": { ...redacted... },
  "confirmed": true | false | null,
  "outcome": "ok" | "denied" | "error"
}
```

### 6.2 Redaction matrix

Implemented in `miru_agent::redact::redact_action`. Forced by
`AuditLog::append`; cannot be bypassed at call sites.

| Capability      | Field          | Replaced with                    |
|-----------------|----------------|----------------------------------|
| KeyType         | `text` (>64 B) | `text_len`, `text_sha256`        |
| ClipboardWrite  | `text`         | `text_len`, `text_sha256`, tag   |
| ClipboardRead   | response only  | redacted at outbound boundary    |
| OpenUrl         | `url`          | `url` (origin-only)              |
| FileRead/Write  | `bytes`        | `bytes_len`, `bytes_sha256`      |
| ScreenRead      | `image`        | `image_len`, `image_sha256`      |
| ShellExec       | `argv`         | preserved verbatim               |
| PointerMove/Click | coords       | preserved verbatim               |

**Never logged**: stdout/stderr of `ShellExec`, raw frame buffers, full
URL paths/queries, plaintext keystrokes longer than 64 bytes.

### 6.3 Tamper-evidence

`prev_hash` chains every entry. Verification recomputes the chain from
seq 0; mismatch at any line = log compromised. Local-only verification
in v0.1; periodic Rekor anchor in v1.0 bounds attacker forgery window.

## 7. Verification matrix (test obligations)

Every invariant above MUST have a corresponding test:

| Invariant                                  | Test                                              |
|--------------------------------------------|---------------------------------------------------|
| §3.1 No nonce reuse                        | `crypto::tests::replay_rejected`                  |
| §3.1 Out-of-order tolerance                | `crypto::tests::out_of_order_within_window_ok`    |
| §3.1 Old-seq rejection                     | `crypto::tests::very_old_seq_rejected`            |
| §3.1 Subkey isolation                      | `crypto::tests::derive_subkey_produces_independent_ciphers` |
| §2.3 verify_strict                         | CI grep: `verify(` without `_strict`              |
| §4.1 TTL ≤ 15 min                          | `token::tests::issue_clamps_oversized_ttl` (TODO) |
| §4.1 Revocation check                      | `revocation::tests::*`                            |
| §6.2 Redaction at append                   | `redact::tests::*` (11 cases)                     |
| §6.3 Chain integrity                       | `audit::tests::tamper_*`                          |

CI runs `cargo test --workspace --lib` on every PR; failure blocks merge.

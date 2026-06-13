# ADR 0016 — Host-Only Transparency Attestation (v0.1)

**Date:** 2026-06-13  
**Status:** Accepted  
**Deciders:** Claude Socratic Review

---

## Context

`session.rs` posted session commitments to a Rekor transparency log using
`CoSignedCommitment::new(metadata, host_key, host_key)` — passing the host's
signing key for **both** the `host_key` and `viewer_key` parameters.

`CoSignedCommitment::verify()` checks `viewer_signature_b64` against
`metadata.viewer_pubkey_b64`. Since the signature was made with the host key
but verified against the viewer pubkey (a different key), verification
**always fails**. Every Rekor entry Miru had ever posted was
cryptographically unverifiable.

Root cause: `CoSignedCommitment` was designed for a future protocol where
both peers co-sign at session end. In practice, the host does not have the
viewer's private signing key — nor should it. Creating a co-signed commitment
requires a 2-round exchange where the viewer receives the commitment hash and
sends its own signature back. That protocol doesn't exist yet.

---

## Decision

1. Add `HostOnlyAttestation { commitment_b64, nonce_b64, host_signature_b64 }`
   with `verify_host()` that checks only the host's signature.

2. Add `rekor::build_host_statement()` that includes `"signer": "host_only"` in
   the predicate so downstream verifiers know not to expect a viewer signature.

3. Add `rekor::submit_host_to_rekor()` that accepts `HostOnlyAttestation`.

4. Fix `session.rs` to call `HostOnlyAttestation::new(&metadata, &host_key)`
   instead of the broken `CoSignedCommitment::new(..., host_key, host_key)`.

5. Keep `CoSignedCommitment` and `submit_to_rekor()` intact — they are
   correct for the future 2-round protocol.

---

## Consequences

**Immediate:** Rekor entries are now verifiable. A third party can check:
- `commitment == SHA256(canonical_metadata || nonce)`
- `host_signature` is valid Ed25519 over `commitment` using `host_pubkey`

**Limitation:** Only the host's side of the session is attested. If the host
lies about `video_frames`, `total_bytes`, or `relayed`, there is no
cryptographic check from the viewer. This is weaker than a co-signed log but
infinitely better than an always-failing signature.

**Future (v1.0):** After session teardown, the viewer can receive the host's
`HostOnlyAttestation`, verify it, sign `commitment` with its own key, and
send the viewer signature back. The host then upgrades to
`CoSignedCommitment` and posts that to Rekor. This requires one post-session
message, which is acceptable latency.

---

## Alternatives Considered

### Keep `CoSignedCommitment` but generate viewer key on host
Rejected: fabricating a fake viewer key defeats the purpose of the
transparency log entirely. The log entry would prove the host committed to
the session, but the "viewer signature" would be the host's, providing no
independent attestation.

### Skip transparency logging until co-signing is implemented
Rejected: host-only attestation has genuine value — it prevents the host
from later denying that a session occurred or that it transmitted certain
byte counts. The `"signer": "host_only"` field makes the limitation explicit
rather than hiding it behind a broken signature.

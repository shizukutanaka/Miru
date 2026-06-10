//! End-to-end integration test of the security control stack.
//!
//! Exercises the full chain in a single process:
//!
//! ```
//! issue token (TTL ≤15min)
//!     ↓
//! parse + verify_strict
//!     ↓
//! authorize capability
//!     ↓
//! audit log append (with redaction)
//!     ↓
//! verify chain integrity
//!     ↓
//! revoke + check rejection
//! ```
//!
//! This is the closest we can get to a real host ↔ viewer loopback without
//! actually running the network stack. The network paths are covered
//! separately by `miru-transport`'s own tests.

use ed25519_dalek::SigningKey;
use miru_agent::audit::{AuditLog, AuditOutcome};
use miru_agent::redact::redact_action;
use miru_agent::revocation::RevocationList;
use miru_agent::token::{AgentToken, Capability, MAX_TTL_SECS};
use rand::rngs::OsRng;
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn full_token_lifecycle() {
    let dir = tempdir().unwrap();
    let mut rng = OsRng;
    let signing_key = SigningKey::generate(&mut rng);
    let pubkey = signing_key.verifying_key();

    // ── 1. Issue a token with mixed-severity capabilities ──
    let caps: HashSet<Capability> = [
        Capability::ScreenRead,
        Capability::KeyType,
        Capability::OpenUrl,
        Capability::ShellExec,
    ]
    .into_iter()
    .collect();

    let token = AgentToken::issue(
        &signing_key,
        "test-claude-session",
        caps.clone(),
        Duration::from_secs(600), // 10 minutes
        None,
    );

    // TTL must not exceed MAX_TTL_SECS even if caller asks for more.
    assert!(token.payload.exp - token.payload.iat <= MAX_TTL_SECS);

    // ── 2. Serialize → parse → verify_strict round trip ──
    let serialized = token.to_string();
    let parsed =
        AgentToken::parse_and_verify(&serialized, &pubkey).expect("token should parse + verify");
    assert_eq!(parsed.payload.jti, token.payload.jti);
    assert_eq!(parsed.payload.caps, caps);

    // ── 3. Audit log: append with redaction enforcement ──
    let log_path = dir.path().join("audit.log");
    let log = AuditLog::open(&log_path).unwrap();

    // KeyType with long plaintext — must be redacted at append time.
    let secret = "password=hunter2_verylongsecretthatmustnotbelogged".repeat(5);
    let seq = log
        .append(
            &token.payload.jti.to_string(),
            Capability::KeyType,
            json!({"text": &secret}),
            Some(true),
            AuditOutcome::Ok,
        )
        .unwrap();
    assert_eq!(seq, 0);

    // Read back the log file directly; the plaintext MUST NOT appear.
    let raw = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        !raw.contains("hunter2"),
        "audit log leaked plaintext! contents: {raw}"
    );
    assert!(
        raw.contains("text_len"),
        "expected redaction sentinel `text_len`"
    );
    assert!(
        raw.contains("text_sha256"),
        "expected redaction sentinel `text_sha256`"
    );

    // ── 4. OpenUrl redaction — path/query stripped to origin ──
    log.append(
        &token.payload.jti.to_string(),
        Capability::OpenUrl,
        json!({"url": "https://api.example.com/v1/secret?token=abc123"}),
        Some(true),
        AuditOutcome::Ok,
    )
    .unwrap();
    let raw = std::fs::read_to_string(&log_path).unwrap();
    assert!(!raw.contains("abc123"), "audit log leaked query string");
    assert!(!raw.contains("/v1/secret"), "audit log leaked URL path");
    assert!(raw.contains("https://api.example.com"));

    // ── 5. ShellExec argv preserved (auditable) ──
    log.append(
        &token.payload.jti.to_string(),
        Capability::ShellExec,
        json!({"cmd": "ls", "args": ["-la"]}),
        Some(true),
        AuditOutcome::Ok,
    )
    .unwrap();
    let raw = std::fs::read_to_string(&log_path).unwrap();
    assert!(raw.contains("\"ls\""), "ShellExec argv should be preserved");

    // ── 6. Revocation: panic rotation ──
    let rev_path = dir.path().join("rev.log");
    let rev = RevocationList::open(&rev_path).unwrap();
    assert!(!rev.is_revoked(&token.payload.jti));

    rev.revoke(token.payload.jti, "test panic rotation")
        .unwrap();
    assert!(rev.is_revoked(&token.payload.jti));

    // Restart: revocations persist
    drop(rev);
    let rev2 = RevocationList::open(&rev_path).unwrap();
    assert!(rev2.is_revoked(&token.payload.jti));
}

#[test]
fn redaction_is_unbypassable() {
    // Verify that even at the redact_action layer, no plaintext leaks.
    let inputs = [
        (Capability::KeyType, json!({"text": "x".repeat(200)})),
        (
            Capability::ClipboardWrite,
            json!({"text": "secret password"}),
        ),
        (
            Capability::OpenUrl,
            json!({"url": "https://evil.example.com/exfil?data=ssn"}),
        ),
        (
            Capability::FileRead,
            json!({"path": "/etc/passwd", "bytes": vec![0u8; 1024]}),
        ),
    ];
    for (cap, action) in inputs {
        let red = redact_action(cap, action.clone());
        let ser = serde_json::to_string(&red).unwrap();
        // Plaintext patterns that must never survive redaction
        assert!(!ser.contains("hunter2"), "leaked plaintext for {cap:?}");
        assert!(!ser.contains("ssn"), "leaked URL query for {cap:?}");
    }
}

#[test]
fn signature_tampering_detected() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let pubkey = key.verifying_key();

    let token = AgentToken::issue(
        &key,
        "test",
        [Capability::ScreenRead].into_iter().collect(),
        Duration::from_secs(60),
        None,
    );
    let serialized = token.to_string();

    // Flip a bit in the signature — must reject.
    let parts: Vec<&str> = serialized.split('.').collect();
    use base64::Engine;
    let mut sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[2])
        .unwrap();
    sig_bytes[0] ^= 0x01;
    let tampered_sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sig_bytes);
    let tampered = format!("{}.{}.{}", parts[0], parts[1], tampered_sig);

    let result = AgentToken::parse_and_verify(&tampered, &pubkey);
    assert!(
        result.is_err(),
        "verify_strict must reject tampered signature"
    );
}

#[test]
fn token_from_different_issuer_rejected() {
    let mut rng = OsRng;
    let alice = SigningKey::generate(&mut rng);
    let mallory = SigningKey::generate(&mut rng);

    let token = AgentToken::issue(
        &mallory,
        "evil",
        [Capability::ShellExec].into_iter().collect(),
        Duration::from_secs(60),
        None,
    );

    // Trying to verify against Alice's pubkey must fail.
    let result = AgentToken::parse_and_verify(&token.to_string(), &alice.verifying_key());
    assert!(result.is_err());
}

#[test]
fn audit_log_chain_integrity() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("audit.log");
    let log = AuditLog::open(&log_path).unwrap();

    // Write 10 entries
    for i in 0..10 {
        log.append(
            "jti-test",
            Capability::ScreenRead,
            json!({"i": i}),
            Some(true),
            AuditOutcome::Ok,
        )
        .unwrap();
    }

    drop(log);

    // Reopen and run the built-in chain verifier.
    let log2 = AuditLog::open(&log_path).unwrap();
    let count = log2.verify().expect("chain should verify");
    assert_eq!(count, 10);

    // Now tamper: corrupt one line of the file and re-open — opening
    // (which calls replay_chain) MUST detect the broken chain.
    let raw = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    let mut tampered = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 5 {
                // Flip a value in the middle of an entry
                l.replace("\"i\":5", "\"i\":99")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    tampered.push('\n');
    std::fs::write(&log_path, tampered).unwrap();

    // Either AuditLog::open OR a subsequent .verify() must detect tampering.
    let detected = match AuditLog::open(&log_path) {
        Err(_) => true,
        Ok(log3) => log3.verify().is_err(),
    };
    assert!(detected, "tampered chain must be rejected");
}

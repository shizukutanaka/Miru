//! Integration tests for miru-auth.

use miru_auth::{
    generate_pin, pin_to_key, AclStore, DeviceIdentity, Permission, TrustDecision, TrustedPeer,
};
use tempfile::tempdir;

#[test]
fn identity_persists_across_loads() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("id");

    let id1 = DeviceIdentity::load_or_create(&path).unwrap();
    let pubkey1 = *id1.verifying_key.as_bytes();
    let fpr1 = id1.pubkey_fingerprint();
    drop(id1);

    let id2 = DeviceIdentity::load_or_create(&path).unwrap();
    assert_eq!(*id2.verifying_key.as_bytes(), pubkey1);
    assert_eq!(id2.pubkey_fingerprint(), fpr1);
}

#[test]
fn identity_creates_new_each_time_when_no_file() {
    let dir = tempdir().unwrap();
    let id1 = DeviceIdentity::load_or_create(&dir.path().join("a")).unwrap();
    let id2 = DeviceIdentity::load_or_create(&dir.path().join("b")).unwrap();
    assert_ne!(id1.pubkey_fingerprint(), id2.pubkey_fingerprint());
}

#[test]
fn signature_verifies_with_correct_key() {
    let id = DeviceIdentity::generate();
    let msg = b"important payload";
    let sig = id.sign(msg);
    assert!(DeviceIdentity::verify(
        id.verifying_key.as_bytes(),
        msg,
        &sig
    ));
}

#[test]
fn signature_fails_with_wrong_key() {
    let id1 = DeviceIdentity::generate();
    let id2 = DeviceIdentity::generate();
    let msg = b"payload";
    let sig = id1.sign(msg);
    assert!(!DeviceIdentity::verify(
        id2.verifying_key.as_bytes(),
        msg,
        &sig
    ));
}

#[test]
fn signature_fails_with_tampered_message() {
    let id = DeviceIdentity::generate();
    let sig = id.sign(b"original");
    assert!(!DeviceIdentity::verify(
        id.verifying_key.as_bytes(),
        b"tampered",
        &sig
    ));
}

#[test]
fn pin_format_six_digits() {
    for _ in 0..50 {
        let pin = generate_pin();
        assert_eq!(pin.len(), 6);
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
    }
}

#[test]
fn pin_to_key_is_deterministic() {
    let k1 = pin_to_key("123456", "DEVICE-1");
    let k2 = pin_to_key("123456", "DEVICE-1");
    assert_eq!(k1, k2);
}

#[test]
fn pin_to_key_differs_on_pin_change() {
    let k1 = pin_to_key("000000", "DEVICE-1");
    let k2 = pin_to_key("000001", "DEVICE-1");
    assert_ne!(k1, k2);
}

#[test]
fn pin_to_key_differs_on_device_change() {
    let k1 = pin_to_key("123456", "DEVICE-A");
    let k2 = pin_to_key("123456", "DEVICE-B");
    assert_ne!(k1, k2);
}

#[test]
fn acl_persists_across_save_load() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("acl.json");

    let mut acl = AclStore::default();
    acl.trust(TrustedPeer {
        device_id: "AAAA-BBBB".into(),
        pubkey_b64: "key1".into(),
        fingerprint: "AB:CD:EF".into(),
        permission: Permission::Control,
        first_seen: 1000,
        last_seen: 2000,
        friendly_name: Some("Alice's laptop".into()),
    });
    acl.save(&path).unwrap();

    let loaded = AclStore::load(&path).unwrap();
    assert_eq!(loaded.list().len(), 1);
    assert_eq!(
        loaded.check("AAAA-BBBB", "key1"),
        TrustDecision::Trusted(Permission::Control)
    );
}

#[test]
fn acl_detects_pubkey_mismatch() {
    let mut acl = AclStore::default();
    acl.trust(TrustedPeer {
        device_id: "AAAA-BBBB".into(),
        pubkey_b64: "legitimate_key".into(),
        fingerprint: "".into(),
        permission: Permission::Full,
        first_seen: 0,
        last_seen: 0,
        friendly_name: None,
    });

    // Same device ID but DIFFERENT pubkey = potential MITM
    assert_eq!(
        acl.check("AAAA-BBBB", "EVIL_KEY_FROM_ATTACKER"),
        TrustDecision::PubkeyMismatch
    );
}

#[test]
fn acl_unknown_device_returns_unknown() {
    let acl = AclStore::default();
    assert_eq!(acl.check("UNKNOWN-DEV", "any_key"), TrustDecision::Unknown);
}

#[test]
fn acl_revoke_removes_peer() {
    let mut acl = AclStore::default();
    acl.trust(TrustedPeer {
        device_id: "AAAA-BBBB".into(),
        pubkey_b64: "k".into(),
        fingerprint: "".into(),
        permission: Permission::Control,
        first_seen: 0,
        last_seen: 0,
        friendly_name: None,
    });
    assert!(acl.revoke("AAAA-BBBB"));
    assert_eq!(acl.check("AAAA-BBBB", "k"), TrustDecision::Unknown);
    assert!(!acl.revoke("AAAA-BBBB")); // already gone
}

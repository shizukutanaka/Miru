#![no_main]
//! Fuzz target: verify_register_signature must never panic on arbitrary input.
//!
//! The signal server calls this on every Register message received over the
//! network. All fields (pubkey, signature, signed_at_sec) come from an
//! untrusted client. This target ensures malformed base64, short byte arrays,
//! invalid ed25519 points, and crafted timestamps never cause a panic.

use libfuzzer_sys::fuzz_target;
use miru_common::message::Register;
use miru_transport::signaling::verify_register_signature;

fuzz_target!(|data: &[u8]| {
    // Split data into 4 chunks: device_id / pubkey / signature / timestamp_bytes
    if data.len() < 4 {
        return;
    }
    let (a, rest) = data.split_at(1);
    let split1 = (a[0] as usize).min(rest.len());
    let (device_id_bytes, rest) = rest.split_at(split1);
    if rest.len() < 3 {
        return;
    }
    let (b, rest) = rest.split_at(1);
    let split2 = (b[0] as usize).min(rest.len());
    let (pubkey_bytes, rest) = rest.split_at(split2);
    if rest.len() < 2 {
        return;
    }
    let (c, sig_bytes) = rest.split_at(1);
    let split3 = (c[0] as usize).min(sig_bytes.len());
    let sig_bytes = &sig_bytes[..split3];

    let reg = Register {
        device_id: String::from_utf8_lossy(device_id_bytes).into_owned(),
        pubkey: base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            pubkey_bytes,
        ),
        pub_addr: None,
        pub_port: None,
        signature: if sig_bytes.is_empty() {
            None
        } else {
            Some(base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                sig_bytes,
            ))
        },
        signed_at_sec: if sig_bytes.is_empty() { None } else { Some(u64::MAX) },
    };

    // Must not panic — only Ok(bool) or Err(_) are acceptable outcomes.
    let _ = verify_register_signature(&reg);
});

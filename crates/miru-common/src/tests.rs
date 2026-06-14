#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::{
        codec::negotiate,
        crypto::{KeyPair, SessionCipher},
        message::{Msg, Ping, VideoCodec},
        session::DeviceId,
    };

    #[test]
    fn device_id_format() {
        let id = DeviceId::new();
        let parts: Vec<&str> = id.0.split('-').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 4);
        assert_eq!(parts[1].len(), 4);
        assert!(parts[0].chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn device_id_unique() {
        let ids: std::collections::HashSet<String> = (0..100).map(|_| DeviceId::new().0).collect();
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn codec_negotiation_picks_best() {
        let host = vec![VideoCodec::H264, VideoCodec::Av1, VideoCodec::Vp9];
        let viewer = vec![VideoCodec::Av1, VideoCodec::H264];
        assert_eq!(negotiate(&host, &viewer), Some(VideoCodec::Av1));
    }

    #[test]
    fn codec_negotiation_fallback() {
        assert_eq!(
            negotiate(&[VideoCodec::H264], &[VideoCodec::Vp9, VideoCodec::H264]),
            Some(VideoCodec::H264),
        );
    }

    #[test]
    fn codec_negotiation_none() {
        assert_eq!(negotiate(&[VideoCodec::H265], &[VideoCodec::Vp8]), None);
    }

    #[test]
    fn cipher_roundtrip() {
        let c = SessionCipher::new([42u8; 32]);
        let ct = c.encrypt(b"hello miru").unwrap();
        assert_eq!(c.decrypt(&ct).unwrap(), b"hello miru");
    }

    #[test]
    fn cipher_distinct_nonces() {
        let c = SessionCipher::new([7u8; 32]);
        let ct1 = c.encrypt(b"a").unwrap();
        let ct2 = c.encrypt(b"a").unwrap();
        assert_ne!(ct1, ct2);
    }

    /// Critical: encrypted output must NEVER contain the plaintext.
    /// (Sanity check against accidentally returning plaintext from encrypt())
    #[test]
    fn cipher_does_not_leak_plaintext() {
        let c = SessionCipher::new([3u8; 32]);
        let plaintext = b"SECRET_DATA_MUST_NOT_LEAK_12345";
        let ct = c.encrypt(plaintext).unwrap();

        // ciphertext must not contain the plaintext as substring
        let pt_str = std::str::from_utf8(plaintext).unwrap();
        let ct_str = String::from_utf8_lossy(&ct);
        assert!(!ct_str.contains(pt_str), "ciphertext leaked plaintext!");

        // ciphertext must be at least 16 bytes longer (12 nonce + 16 tag minus 8 stored seq)
        assert!(ct.len() >= plaintext.len() + 16);
    }

    #[test]
    fn cipher_rejects_tampered_ciphertext() {
        let c = SessionCipher::new([5u8; 32]);
        let mut ct = c.encrypt(b"important").unwrap();
        // Flip a bit somewhere in the ciphertext (after the seq nonce)
        ct[15] ^= 0x01;
        assert!(c.decrypt(&ct).is_err(), "tampered ciphertext was accepted");
    }

    #[test]
    fn cipher_rejects_truncated() {
        let c = SessionCipher::new([5u8; 32]);
        let ct = c.encrypt(b"data").unwrap();
        for len in 0..ct.len() {
            assert!(
                c.decrypt(&ct[..len]).is_err(),
                "truncated ct was accepted at len {len}"
            );
        }
    }

    #[test]
    fn key_exchange_symmetric() {
        // Two independent keypairs with shared dh — same shared secret on both sides
        // (but x25519-dalek consumes EphemeralSecret on dh, so we test indirectly)
        let kp1 = KeyPair::generate();
        let kp2 = KeyPair::generate();
        let pub2 = *kp2.public.as_bytes();
        let pub1 = *kp1.public.as_bytes();
        let s1 = kp1.diffie_hellman(&pub2);
        let s2 = kp2.diffie_hellman(&pub1);
        assert_eq!(s1, s2, "DH shared secrets diverged");
    }

    #[test]
    fn message_roundtrip() {
        let msg = Msg::Ping(Ping { ts: 12345 });
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Msg = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, Msg::Ping(_)));
    }

    #[test]
    fn all_message_variants_serialize() {
        use crate::message::*;
        let msgs: Vec<Msg> = vec![
            Msg::Ping(Ping { ts: 0 }),
            Msg::Pong(Pong {
                ts: 0,
                server_ts: 0,
            }),
            Msg::KeyFrame,
            Msg::Register(Register {
                device_id: "A1B2-C3D4".into(),
                pubkey: "".into(),
                pub_addr: None,
                pub_port: None,
                signature: None,
                signed_at_sec: None,
            }),
            Msg::Close(CloseReason {
                code: 0,
                reason: "".into(),
            }),
            Msg::QosUpdate(QosUpdate {
                fps: 30,
                bitrate_kbps: 2000,
                quality: 75,
            }),
        ];
        for msg in &msgs {
            let json = serde_json::to_string(msg).expect("serialize");
            serde_json::from_str::<Msg>(&json).expect("deserialize");
        }
    }

    // ── Fuzz-equivalent property tests ──────────────────────────────────────
    // These run the same invariants as fuzz/fuzz_targets/* but deterministically
    // in normal CI (no nightly/libfuzzer needed). They guard against a refactor
    // silently breaking the "never panic on arbitrary bytes" property.

    /// Mirror of fuzz_message_parse: the JSON message parser must never panic
    /// on arbitrary input — only return Ok or Err.
    #[test]
    fn message_parser_never_panics_on_garbage() {
        use crate::message::Msg;
        use rand::rngs::StdRng;
        use rand::{RngCore, SeedableRng};

        // Fixed seed → reproducible. 10k random byte strings of varied length.
        let mut rng = StdRng::seed_from_u64(0x4D49_5255_u64);
        for _ in 0..10_000 {
            let len = (rng.next_u32() % 512) as usize;
            let mut buf = vec![0u8; len];
            rng.fill_bytes(&mut buf);
            // Must not panic; result is intentionally discarded.
            let _ = serde_json::from_slice::<Msg>(&buf);
        }

        // Also exercise structurally-plausible-but-invalid JSON.
        let evil = [
            br#"{"type":"#.as_slice(),
            br#"{"type":"video_frame","data":"#.as_slice(),
            br#"{"type":"unknown_variant_xyz","x":1}"#.as_slice(),
            br#"[[[[[[[[[[]]]]]]]]]]"#.as_slice(),
            br#"{"type":"qos_update","fps":99999999999999999999}"#.as_slice(),
        ];
        for bytes in evil {
            let _ = serde_json::from_slice::<Msg>(bytes);
        }
    }

    /// Mirror of fuzz_decrypt: the AEAD decrypt path must never panic on
    /// arbitrary input — garbage must yield Err, not a crash.
    #[test]
    fn decrypt_never_panics_on_garbage() {
        use crate::crypto::SessionCipher;
        use rand::rngs::StdRng;
        use rand::{RngCore, SeedableRng};

        let cipher = SessionCipher::new([0u8; 32]);
        let mut rng = StdRng::seed_from_u64(0xDEAD_BEEF);
        for _ in 0..10_000 {
            let len = (rng.next_u32() % 128) as usize;
            let mut buf = vec![0u8; len];
            rng.fill_bytes(&mut buf);
            // Garbage ciphertext must error, never panic.
            assert!(cipher.decrypt(&buf).is_err() || buf.len() >= 24);
        }

        // Boundary lengths around the nonce(8)+tag(16) framing.
        for len in 0..32usize {
            let buf = vec![0u8; len];
            let _ = cipher.decrypt(&buf); // must not panic
        }
    }
}

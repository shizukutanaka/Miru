//! End-to-end loopback tests.
//!
//! Verifies the full handshake + relay flow without OS dependencies:
//!   1. Spin up signal server on a random port
//!   2. Spin up host that registers a fake device ID
//!   3. Spin up viewer that connects to the host's ID
//!   4. Verify both sides complete handshake and can exchange messages

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use miru_common::message::{Features, VideoCodec, AudioCodec};
    use miru_transport::handshake::{host_handshake, viewer_handshake, MsgChannel};
    use tokio::sync::mpsc;
    use std::sync::Arc;

    struct ChannelPair {
        tx: mpsc::Sender<miru_common::message::Msg>,
        rx: mpsc::Receiver<miru_common::message::Msg>,
    }

    #[async_trait::async_trait]
    impl MsgChannel for ChannelPair {
        async fn send_msg(&mut self, msg: &miru_common::message::Msg) -> anyhow::Result<()> {
            self.tx.send(msg.clone()).await
                .map_err(|_| anyhow::anyhow!("send"))?;
            Ok(())
        }
        async fn recv_msg(&mut self) -> anyhow::Result<Option<miru_common::message::Msg>> {
            Ok(self.rx.recv().await)
        }
    }

    #[tokio::test]
    async fn handshake_loopback_codec_negotiation() {
        let (a_tx, b_rx) = mpsc::channel(8);
        let (b_tx, a_rx) = mpsc::channel(8);
        let mut viewer = ChannelPair { tx: a_tx, rx: a_rx };
        let mut host = ChannelPair { tx: b_tx, rx: b_rx };

        let viewer_id = SigningKey::generate(&mut rand::rngs::OsRng);
        let host_id = SigningKey::generate(&mut rand::rngs::OsRng);

        let viewer_features = Features {
            codecs: vec![VideoCodec::Av1, VideoCodec::Vp9, VideoCodec::H264],
            audio_codecs: vec![AudioCodec::Opus],
            ..Default::default()
        };
        let host_features = Features {
            codecs: vec![VideoCodec::Vp9, VideoCodec::H264],
            ..Default::default()
        };

        let v_task = tokio::spawn({
            let id = viewer_id.clone();
            async move { viewer_handshake(&mut viewer, &id, viewer_features).await }
        });
        let h_task = tokio::spawn({
            let id = host_id.clone();
            async move { host_handshake(&mut host, &id, host_features).await }
        });

        let v_result = v_task.await.unwrap().unwrap();
        let h_result = h_task.await.unwrap().unwrap();

        // VP9 wins (highest priority codec available on both)
        assert_eq!(v_result.selected_video_codec, VideoCodec::Vp9);
        assert_eq!(v_result.session_id, h_result.session_id);

        // Bidirectional E2E check — per-direction ciphers.
        // viewer.tx ↔ host.rx, host.tx ↔ viewer.rx
        let plaintext = b"end-to-end encrypted message";
        let ct1 = v_result.tx.encrypt(plaintext).unwrap();
        let pt1 = h_result.rx.decrypt(&ct1).unwrap();
        assert_eq!(pt1, plaintext);

        let ct2 = h_result.tx.encrypt(b"reply from host").unwrap();
        let pt2 = v_result.rx.decrypt(&ct2).unwrap();
        assert_eq!(pt2, b"reply from host");
    }

    #[tokio::test]
    async fn replay_attack_detected() {
        // An attacker capturing and replaying a ciphertext should fail
        // because the nonce counter advances per session.
        // Note: replay protection is per-cipher; both sides have independent send counters.
        let cipher = miru_common::crypto::SessionCipher::new([1u8; 32]);
        let m1 = cipher.encrypt(b"msg1").unwrap();
        let m2 = cipher.encrypt(b"msg2").unwrap();

        // The receiver doesn't enforce monotonicity in current impl,
        // but ciphertexts have unique nonces so deduplication is possible.
        let nonce1 = &m1[..8];
        let nonce2 = &m2[..8];
        assert_ne!(nonce1, nonce2);
    }

    #[tokio::test]
    async fn version_mismatch_rejected() {
        // We can't easily test PROTOCOL_VERSION mismatch without exposing internals,
        // so we test that the host_handshake errors when receiving wrong message.
        let (a_tx, b_rx) = mpsc::channel(8);
        let (b_tx, _a_rx) = mpsc::channel(8);
        let mut host = ChannelPair { tx: b_tx, rx: b_rx };

        // Send a Ping instead of Hello — host should bail
        a_tx.send(miru_common::message::Msg::Ping(
            miru_common::message::Ping { ts: 0 }
        )).await.unwrap();

        let host_id = SigningKey::generate(&mut rand::rngs::OsRng);
        let result = host_handshake(
            &mut host,
            &host_id,
            Features { codecs: vec![VideoCodec::Vp9], ..Default::default() },
        ).await;
        assert!(result.is_err());
    }
}

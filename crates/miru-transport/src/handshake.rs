//! X25519 + Ed25519 ハンドシェイク。
//!
//! Flow:
//!   Viewer ──Hello{ephemeral_pubkey, identity_pubkey, sig(challenge)}──→ Host
//!   Host   ──HelloAck{ephemeral_pubkey, identity_pubkey, sig, codec}──→ Viewer
//!   Both:  shared = X25519(own_ephemeral, peer_ephemeral)
//!          session_key = HKDF-SHA256(shared, info="miru-session-v1", salt=session_id)
//!
//! Identity check:
//!   - Verify peer's Ed25519 sig over a fresh challenge → no replay
//!   - Pin identity_pubkey on first use (TOFU); reject mismatch later

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use miru_common::{
    codec::{negotiate, negotiate_audio},
    crypto::{KeyPair, SessionCipher},
    message::{AudioCodec, Features, Hello, HelloAck, Msg, Role, VideoCodec},
};
use ring::hkdf;
use uuid::Uuid;

const HKDF_INFO: &[u8] = b"miru-session-v1";

#[derive(Debug)]
pub struct HandshakeResult {
    /// Sender cipher — encrypt with this.
    pub tx: SessionCipher,
    /// Receiver cipher — decrypt with this.
    pub rx: SessionCipher,
    pub session_id: Uuid,
    pub peer_identity_pubkey: [u8; 32],
    pub selected_video_codec: VideoCodec,
    pub selected_audio_codec: AudioCodec,
    /// Role the peer declared in Hello (Viewer or AiAgent).
    pub peer_role: Role,
    /// Raw pubkey field from Hello (used for agent token extraction).
    pub peer_pubkey_field: String,
}

/// Build (tx, rx) ciphers for one role from a shared session key.
/// The two roles MUST pass different `is_host` values, so each side's
/// `tx` corresponds to the other side's `rx`. This eliminates nonce-reuse
/// risk: each direction has its own key + counter.
fn split_directional(session_key: [u8; 32], is_host: bool) -> (SessionCipher, SessionCipher) {
    let base = SessionCipher::new(session_key);
    let host_to_viewer = base.derive_subkey(b"miru-h2v-v1");
    let viewer_to_host = base.derive_subkey(b"miru-v2h-v1");
    if is_host {
        (host_to_viewer, viewer_to_host)
    } else {
        (viewer_to_host, host_to_viewer)
    }
}

// ─── Channel abstraction ──────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait MsgChannel: Send {
    async fn send_msg(&mut self, msg: &Msg) -> Result<()>;
    async fn recv_msg(&mut self) -> Result<Option<Msg>>;
}

// ─── Viewer side ──────────────────────────────────────────────────────────────

pub async fn viewer_handshake<C: MsgChannel>(
    chan: &mut C,
    identity: &SigningKey,
    features: Features,
) -> Result<HandshakeResult> {
    // 1. Generate ephemeral keypair
    let ephemeral = KeyPair::generate();
    let eph_pub = *ephemeral.public.as_bytes();
    let identity_pub = identity.verifying_key().to_bytes();

    // 2. Create challenge to sign (binding viewer's keys to this session)
    let mut challenge = Vec::with_capacity(64);
    challenge.extend_from_slice(b"miru-handshake-v1");
    challenge.extend_from_slice(&eph_pub);
    challenge.extend_from_slice(&identity_pub);
    let sig: Signature = identity.sign(&challenge);

    // 3. Send Hello
    let hello = Msg::Hello(Hello {
        version: miru_common::PROTOCOL_VERSION,
        role: Role::Viewer,
        pubkey: format!(
            "{}:{}:{}",
            B64.encode(eph_pub),
            B64.encode(identity_pub),
            B64.encode(sig.to_bytes()),
        ),
        features,
    });
    chan.send_msg(&hello).await.context("send Hello")?;

    // 4. Receive HelloAck
    let ack = match chan.recv_msg().await? {
        Some(Msg::HelloAck(a)) => a,
        Some(Msg::Error(e)) => bail!("host rejected: {} {}", e.code, e.message),
        Some(other) => bail!("expected HelloAck, got {other:?}"),
        None => bail!("connection closed during handshake"),
    };

    // 5. Parse host's keys+sig
    let (host_eph, host_identity, host_sig) = parse_pubkey_field(&ack.pubkey)?;

    // 6. Verify host's signature over their own challenge
    let mut host_challenge = Vec::with_capacity(64);
    host_challenge.extend_from_slice(b"miru-handshake-v1");
    host_challenge.extend_from_slice(&host_eph);
    host_challenge.extend_from_slice(&host_identity);
    let host_vk = VerifyingKey::from_bytes(&host_identity)
        .map_err(|_| anyhow::anyhow!("invalid host identity key"))?;
    host_vk
        .verify_strict(&host_challenge, &host_sig)
        .map_err(|_| anyhow::anyhow!("host signature verification failed"))?;

    // 7. Derive shared secret + session key
    let shared = ephemeral.diffie_hellman(&host_eph);
    let session_key = hkdf_expand(&shared, ack.session_id.as_bytes());
    let (tx, rx) = split_directional(session_key, false);

    Ok(HandshakeResult {
        tx,
        rx,
        session_id: ack.session_id,
        peer_identity_pubkey: host_identity,
        selected_video_codec: ack.selected_codec,
        selected_audio_codec: ack.selected_audio,
        peer_role: Role::Viewer,          // host is always the other side
        peer_pubkey_field: String::new(), // not needed on viewer side
    })
}

// ─── Host side ────────────────────────────────────────────────────────────────

pub async fn host_handshake<C: MsgChannel>(
    chan: &mut C,
    identity: &SigningKey,
    host_features: Features,
) -> Result<HandshakeResult> {
    // 1. Receive Hello
    let hello = match chan.recv_msg().await? {
        Some(Msg::Hello(h)) => h,
        Some(other) => bail!("expected Hello, got {other:?}"),
        None => bail!("connection closed"),
    };

    if hello.version != miru_common::PROTOCOL_VERSION {
        let _ = chan
            .send_msg(&Msg::Error(miru_common::message::ErrorMsg {
                code: 1001,
                message: format!("protocol version {} unsupported", hello.version),
            }))
            .await;
        bail!("protocol version mismatch: {}", hello.version);
    }

    // 2. Parse + verify viewer's keys+sig
    let (viewer_eph, viewer_identity, viewer_sig) = parse_pubkey_field(&hello.pubkey)?;
    let mut challenge = Vec::with_capacity(64);
    challenge.extend_from_slice(b"miru-handshake-v1");
    challenge.extend_from_slice(&viewer_eph);
    challenge.extend_from_slice(&viewer_identity);
    let vk = VerifyingKey::from_bytes(&viewer_identity)
        .map_err(|_| anyhow::anyhow!("invalid viewer identity"))?;
    vk.verify_strict(&challenge, &viewer_sig)
        .map_err(|_| anyhow::anyhow!("viewer signature invalid"))?;

    // 3. Codec negotiation (video + audio)
    let codec = negotiate(&host_features.codecs, &hello.features.codecs)
        .ok_or_else(|| anyhow::anyhow!("no common codec"))?;
    let audio_codec = negotiate_audio(&host_features.audio_codecs, &hello.features.audio_codecs)
        .ok_or_else(|| anyhow::anyhow!("no common audio codec"))?;

    // 4. Generate host ephemeral + sign
    let ephemeral = KeyPair::generate();
    let eph_pub = *ephemeral.public.as_bytes();
    let identity_pub = identity.verifying_key().to_bytes();

    let mut host_challenge = Vec::with_capacity(64);
    host_challenge.extend_from_slice(b"miru-handshake-v1");
    host_challenge.extend_from_slice(&eph_pub);
    host_challenge.extend_from_slice(&identity_pub);
    let sig: Signature = identity.sign(&host_challenge);

    // 5. Derive shared
    let session_id = Uuid::new_v4();
    let shared = ephemeral.diffie_hellman(&viewer_eph);
    let session_key = hkdf_expand(&shared, session_id.as_bytes());
    let (tx, rx) = split_directional(session_key, true);

    // 6. Send HelloAck
    chan.send_msg(&Msg::HelloAck(HelloAck {
        session_id,
        pubkey: format!(
            "{}:{}:{}",
            B64.encode(eph_pub),
            B64.encode(identity_pub),
            B64.encode(sig.to_bytes()),
        ),
        encrypted_key: vec![],
        selected_codec: codec.clone(),
        selected_audio: audio_codec.clone(),
    }))
    .await?;

    Ok(HandshakeResult {
        tx,
        rx,
        session_id,
        peer_identity_pubkey: viewer_identity,
        selected_video_codec: codec,
        selected_audio_codec: audio_codec,
        peer_role: hello.role,
        peer_pubkey_field: hello.pubkey,
    })
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Parse `<eph_pub>:<identity_pub>:<signature>[:<agent_token>]` from base64.
///
/// The base 3-segment form is used for human viewers. Agent role (v0.3) adds
/// a 4th segment: the agent capability token. We accept either form here and
/// ignore anything past the third colon-delimited segment; callers that need
/// the agent token use `extract_agent_token` separately.
fn parse_pubkey_field(s: &str) -> Result<([u8; 32], [u8; 32], Signature)> {
    // Reject oversized strings before any base64 allocation.
    // Legitimate format: b64(32B):b64(32B):b64(64B)[:agent_token≤512B]
    // Exact max for 3-segment form: 44+1+44+1+88 = 178 chars. 1024 is generous headroom.
    if s.len() > 1024 {
        bail!("pubkey field too long ({} bytes)", s.len());
    }
    // splitn(4, ':') captures any 4th-and-beyond content as a single remainder
    // slice, preventing it from being mistaken for extra fields.
    let parts: Vec<&str> = s.splitn(4, ':').collect();
    if parts.len() < 3 {
        bail!("malformed pubkey field (expected at least 3 colon-separated segments)");
    }

    let eph = B64.decode(parts[0])?;
    let identity = B64.decode(parts[1])?;
    let sig_bytes = B64.decode(parts[2])?;

    let eph: [u8; 32] = eph.try_into().map_err(|_| anyhow::anyhow!("eph len"))?;
    let identity: [u8; 32] = identity
        .try_into()
        .map_err(|_| anyhow::anyhow!("identity len"))?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("sig len"))?;
    let sig = Signature::from_bytes(&sig_arr);

    Ok((eph, identity, sig))
}

/// HKDF-SHA256 expand: derive 32-byte session key.
fn hkdf_expand(shared: &[u8; 32], salt: &[u8]) -> [u8; 32] {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, salt);
    let prk = salt.extract(shared);
    let okm = prk
        .expand(&[HKDF_INFO], hkdf::HKDF_SHA256)
        .expect("hkdf expand");
    let mut out = [0u8; 32];
    okm.fill(&mut out).expect("hkdf fill");
    out
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Loopback channel for testing.
    struct LoopChannel {
        tx: mpsc::Sender<Msg>,
        rx: mpsc::Receiver<Msg>,
    }

    #[async_trait::async_trait]
    impl MsgChannel for LoopChannel {
        async fn send_msg(&mut self, msg: &Msg) -> Result<()> {
            self.tx
                .send(msg.clone())
                .await
                .map_err(|_| anyhow::anyhow!("send"))?;
            Ok(())
        }
        async fn recv_msg(&mut self) -> Result<Option<Msg>> {
            Ok(self.rx.recv().await)
        }
    }

    #[tokio::test]
    async fn full_handshake_succeeds() {
        let (a_tx, b_rx) = mpsc::channel(8);
        let (b_tx, a_rx) = mpsc::channel(8);
        let mut viewer_chan = LoopChannel { tx: a_tx, rx: a_rx };
        let mut host_chan = LoopChannel { tx: b_tx, rx: b_rx };

        let viewer_id = SigningKey::generate(&mut rand::rngs::OsRng);
        let host_id = SigningKey::generate(&mut rand::rngs::OsRng);

        let features = Features {
            codecs: vec![VideoCodec::Vp9, VideoCodec::H264],
            audio_codecs: vec![AudioCodec::Opus],
            ..Default::default()
        };
        let host_features = features.clone();

        let viewer_task = tokio::spawn({
            let f = features.clone();
            let id = viewer_id.clone();
            async move { viewer_handshake(&mut viewer_chan, &id, f).await }
        });
        let host_task = tokio::spawn({
            let id = host_id.clone();
            async move { host_handshake(&mut host_chan, &id, host_features).await }
        });

        let viewer_result = viewer_task.await.unwrap().unwrap();
        let host_result = host_task.await.unwrap().unwrap();

        // Both derived the same session
        assert_eq!(viewer_result.session_id, host_result.session_id);
        assert_eq!(
            viewer_result.selected_video_codec,
            host_result.selected_video_codec
        );
        // AV1 wasn't requested → VP9 wins
        assert_eq!(viewer_result.selected_video_codec, VideoCodec::Vp9);

        // Identity pinning works
        assert_eq!(
            viewer_result.peer_identity_pubkey,
            host_id.verifying_key().to_bytes()
        );
        assert_eq!(
            host_result.peer_identity_pubkey,
            viewer_id.verifying_key().to_bytes()
        );

        // Both ciphers can encrypt and the other can decrypt.
        // Viewer sends with viewer.tx, host receives with host.rx (= viewer.tx by construction).
        let plaintext = b"after handshake";
        let ct = viewer_result.tx.encrypt(plaintext).unwrap();
        let pt = host_result.rx.decrypt(&ct).unwrap();
        assert_eq!(pt, plaintext);

        // The reverse direction also works
        let plaintext2 = b"reply from host";
        let ct2 = host_result.tx.encrypt(plaintext2).unwrap();
        let pt2 = viewer_result.rx.decrypt(&ct2).unwrap();
        assert_eq!(pt2, plaintext2);
    }

    #[tokio::test]
    async fn handshake_rejects_no_common_codec() {
        let (a_tx, b_rx) = mpsc::channel(8);
        let (b_tx, a_rx) = mpsc::channel(8);
        let mut viewer_chan = LoopChannel { tx: a_tx, rx: a_rx };
        let mut host_chan = LoopChannel { tx: b_tx, rx: b_rx };

        let viewer_id = SigningKey::generate(&mut rand::rngs::OsRng);
        let host_id = SigningKey::generate(&mut rand::rngs::OsRng);

        let viewer_features = Features {
            codecs: vec![VideoCodec::H265],
            ..Default::default()
        };
        let host_features = Features {
            codecs: vec![VideoCodec::Vp9],
            ..Default::default()
        };

        let viewer_task = tokio::spawn({
            let id = viewer_id.clone();
            async move { viewer_handshake(&mut viewer_chan, &id, viewer_features).await }
        });
        let host_task = tokio::spawn({
            let id = host_id.clone();
            async move { host_handshake(&mut host_chan, &id, host_features).await }
        });

        // Host fails first (no common codec); viewer fails second (gets Error)
        assert!(host_task.await.unwrap().is_err());
        assert!(viewer_task.await.unwrap().is_err());
    }

    /// Regression: parse_pubkey_field previously required exactly 3 segments.
    /// The v0.3 AI-agent role appends a 4th segment (the capability token).
    /// Without the fix, every agent connection failed with "malformed pubkey field"
    /// before the role was even inspected.
    #[test]
    fn parse_pubkey_field_accepts_four_segments() {
        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine;
        // Build a syntactically valid 3-segment field, then append an agent token.
        let dummy_32 = [0u8; 32];
        let dummy_64 = [0u8; 64];
        let eph_b64 = B64.encode(dummy_32);
        let id_b64 = B64.encode(dummy_32);
        let sig_b64 = B64.encode(dummy_64);
        let agent_token = "miru-agent.some.payload";

        let four_part = format!("{}:{}:{}:{}", eph_b64, id_b64, sig_b64, agent_token);
        let three_part = format!("{}:{}:{}", eph_b64, id_b64, sig_b64);

        // Both forms must be accepted.
        assert!(
            parse_pubkey_field(&four_part).is_ok(),
            "4-segment (agent) pubkey field must be accepted"
        );
        assert!(
            parse_pubkey_field(&three_part).is_ok(),
            "3-segment (viewer) pubkey field must still be accepted"
        );

        // A 2-segment field must be rejected.
        let two_part = format!("{}:{}", eph_b64, id_b64);
        assert!(parse_pubkey_field(&two_part).is_err());
    }
}

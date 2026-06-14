//! Signal server client — registers device, requests connections, handles relay offers.

use anyhow::{Context, Result};
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use miru_common::{
    message::{ConnectRequest, Msg, Register},
    session::DeviceId,
};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::info;

pub struct SignalClient {
    tx: mpsc::Sender<Msg>,
    events: mpsc::Receiver<SignalEvent>,
    /// Base64url-encoded Ed25519 verifying key sent in Register.
    identity_pubkey: String,
    /// Raw pubkey bytes used for signature construction.
    identity_pubkey_bytes: Option<[u8; 32]>,
    /// Private signing key for signing registration messages.
    signing_key: Option<SigningKey>,
}

#[derive(Debug)]
pub enum SignalEvent {
    Registered {
        device_id: String,
        relay_addr: Option<String>,
    },
    IncomingConnection {
        token: String,
        relay_addr: String,
        relay_port: u16,
    },
    ConnectAck {
        target_id: String,
        /// True → must use relay; False → may attempt QUIC P2P first.
        relay: bool,
        /// Host's STUN-discovered public IP (None if behind symmetric NAT).
        host_addr: Option<String>,
        /// Host's STUN-discovered public port.
        host_port: Option<u16>,
    },
    Error {
        code: u16,
        message: String,
    },
    Disconnected,
}

impl SignalClient {
    /// Connect to the signal server and register with an optional STUN-discovered
    /// public address. Pass `pub_addr = None` to skip public address advertisement.
    pub async fn connect_with_pub_addr(
        signal_url: &str,
        device_id: &DeviceId,
        identity_pubkey: Option<&[u8; 32]>,
        pub_addr: Option<(String, u16)>,
    ) -> Result<Self> {
        Self::connect_with_pub_addr_signed(signal_url, device_id, identity_pubkey, None, pub_addr).await
    }

    /// Like `connect_with_pub_addr` but also signs the Register message with
    /// the provided Ed25519 signing key. The signal server verifies the signature
    /// and rejects registrations that claim a device_id without proving key ownership.
    pub async fn connect_with_pub_addr_signed(
        signal_url: &str,
        device_id: &DeviceId,
        identity_pubkey: Option<&[u8; 32]>,
        signing_key: Option<&SigningKey>,
        pub_addr: Option<(String, u16)>,
    ) -> Result<Self> {
        let client = Self::connect_signed(signal_url, device_id, identity_pubkey, signing_key).await?;
        if let Some((addr, port)) = pub_addr {
            client
                .register_with_pub_addr(device_id, Some(addr), Some(port))
                .await?;
        }
        Ok(client)
    }

    pub async fn connect(
        signal_url: &str,
        device_id: &DeviceId,
        identity_pubkey: Option<&[u8; 32]>,
    ) -> Result<Self> {
        Self::connect_signed(signal_url, device_id, identity_pubkey, None).await
    }

    /// Connect and sign all registration messages with the provided Ed25519 key.
    /// Callers with a `DeviceIdentity` should prefer this over `connect()` so the
    /// signal server can verify that the registering device owns the claimed pubkey.
    pub async fn connect_signed(
        signal_url: &str,
        device_id: &DeviceId,
        identity_pubkey: Option<&[u8; 32]>,
        signing_key: Option<&SigningKey>,
    ) -> Result<Self> {
        // Encode pubkey for registration (base64url, no padding).
        let pubkey_b64 = identity_pubkey
            .map(|k| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(k))
            .unwrap_or_default();

        // Warn when connecting without TLS — device IDs and connection metadata
        // (peer addresses, timing) are visible to any network observer.
        // wss:// URLs are always safe. ws:// is only safe for loopback (localhost,
        // 127.0.0.1, ::1) where no other host can intercept the traffic.
        if signal_url.starts_with("ws://") {
            let is_loopback = signal_url.contains("localhost")
                || signal_url.contains("127.0.0.1")
                || signal_url.contains("[::1]");
            if !is_loopback {
                tracing::warn!(
                    "Signal server URL uses plain WebSocket (ws://): device IDs and connection \
                    metadata are visible to network observers. Use wss:// with a TLS-terminating \
                    reverse proxy (nginx, Caddy) in production. URL: {signal_url}"
                );
            }
        }

        let (ws_stream, _) = connect_async(signal_url)
            .await
            .context("signal server connect failed")?;
        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Msg>(32);
        let (evt_tx, evt_rx) = mpsc::channel::<SignalEvent>(32);

        // Writer task
        tokio::spawn(async move {
            while let Some(msg) = cmd_rx.recv().await {
                let Ok(json) = serde_json::to_string(&msg) else {
                    continue;
                };
                if ws_tx.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        });

        // Reader task
        let evt_tx2 = evt_tx.clone();
        tokio::spawn(async move {
            while let Some(raw) = ws_rx.next().await {
                match raw {
                    Ok(Message::Text(text)) => {
                        let Ok(msg) = serde_json::from_str::<Msg>(&text) else {
                            continue;
                        };
                        let event = match msg {
                            Msg::RegisterAck(ack) => SignalEvent::Registered {
                                device_id: ack.device_id,
                                relay_addr: ack.relay_addr,
                            },
                            Msg::Relay(offer) => SignalEvent::IncomingConnection {
                                token: offer.token,
                                relay_addr: offer.relay_addr,
                                relay_port: offer.relay_port,
                            },
                            Msg::ConnectAck(ack) => SignalEvent::ConnectAck {
                                target_id: ack.target_id,
                                relay: ack.relay,
                                host_addr: ack.host_addr,
                                host_port: ack.host_port,
                            },
                            Msg::Error(e) => SignalEvent::Error {
                                code: e.code,
                                message: e.message,
                            },
                            _ => continue,
                        };
                        if evt_tx2.send(event).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => {
                        let _ = evt_tx2.send(SignalEvent::Disconnected).await;
                        break;
                    }
                    _ => {}
                }
            }
        });

        let client = Self {
            tx: cmd_tx,
            events: evt_rx,
            identity_pubkey: pubkey_b64,
            identity_pubkey_bytes: identity_pubkey.copied(),
            signing_key: signing_key.cloned(),
        };

        // Register immediately
        client.register(device_id).await?;
        info!("Signal: registered as {}", device_id);
        Ok(client)
    }

    async fn register(&self, device_id: &DeviceId) -> Result<()> {
        self.register_with_pub_addr(device_id, None, None).await
    }

    pub async fn register_with_pub_addr(
        &self,
        device_id: &DeviceId,
        pub_addr: Option<String>,
        pub_port: Option<u16>,
    ) -> Result<()> {
        let (signature, signed_at_sec) = self.sign_registration(&device_id.0);
        self.send(Msg::Register(Register {
            device_id: device_id.0.clone(),
            pubkey: self.identity_pubkey.clone(),
            pub_addr,
            pub_port,
            signature,
            signed_at_sec,
        }))
        .await
    }

    /// Build the Ed25519 signature for a Register message, if we have a signing key.
    /// Returns (signature_b64url, unix_seconds) or (None, None) if unsigned.
    fn sign_registration(&self, device_id: &str) -> (Option<String>, Option<u64>) {
        use ed25519_dalek::Signer;

        let (Some(sk), Some(pk_bytes)) = (&self.signing_key, &self.identity_pubkey_bytes) else {
            return (None, None);
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // blob = device_id_bytes || pubkey_bytes (32) || timestamp_le (8)
        let mut blob = Vec::with_capacity(device_id.len() + 40);
        blob.extend_from_slice(device_id.as_bytes());
        blob.extend_from_slice(pk_bytes.as_ref());
        blob.extend_from_slice(&now.to_le_bytes());

        let sig = sk.sign(&blob);
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
        (Some(sig_b64), Some(now))
    }

    pub async fn request_connect(&self, target_id: &str, viewer_addr: &str) -> Result<()> {
        self.send(Msg::Connect(ConnectRequest {
            target_id: target_id.to_string(),
            viewer_addr: viewer_addr.to_string(),
            viewer_port: 0,
        }))
        .await
    }

    async fn send(&self, msg: Msg) -> Result<()> {
        self.tx
            .send(msg)
            .await
            .map_err(|_| anyhow::anyhow!("signal tx closed"))
    }

    pub async fn next_event(&mut self) -> Option<SignalEvent> {
        self.events.recv().await
    }
}

/// Verify a signed Register message. Returns Err if signature is present but invalid.
/// Returns Ok(false) if no signature (backward-compat; caller may warn).
/// Returns Ok(true) if signature verified.
pub fn verify_register_signature(reg: &miru_common::message::Register) -> Result<bool> {
    use ed25519_dalek::VerifyingKey;

    let (Some(sig_b64), Some(ts)) = (&reg.signature, reg.signed_at_sec) else {
        return Ok(false);
    };

    // Reject timestamps outside ±5 minutes to prevent replay.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let delta = now.abs_diff(ts);
    if delta > 300 {
        anyhow::bail!("Register signature timestamp too skewed: {}s drift", delta);
    }

    // Decode the pubkey (base64url).
    let pk_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&reg.pubkey)
        .context("pubkey is not base64url")?;
    let pk_arr: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("pubkey must be 32 bytes"))?;
    let vk = VerifyingKey::from_bytes(&pk_arr)
        .map_err(|e| anyhow::anyhow!("invalid pubkey: {e}"))?;

    // Decode the signature.
    let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64)
        .context("signature is not base64url")?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("signature must be 64 bytes"))?;
    let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);

    // Reconstruct blob.
    let mut blob = Vec::with_capacity(reg.device_id.len() + 40);
    blob.extend_from_slice(reg.device_id.as_bytes());
    blob.extend_from_slice(&pk_arr);
    blob.extend_from_slice(&ts.to_le_bytes());

    // verify_strict rejects small-order pubkeys and non-canonical encodings.
    vk.verify_strict(&blob, &sig)
        .map_err(|_| anyhow::anyhow!("Register signature verification failed"))?;

    Ok(true)
}

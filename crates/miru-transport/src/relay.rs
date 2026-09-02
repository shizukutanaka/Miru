//! WebSocket relay transport.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use miru_common::{crypto::SessionCipher, message::Msg};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::info;

const RELAY_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

use crate::handshake::MsgChannel;

pub struct RelayTransport {
    tx: mpsc::Sender<Vec<u8>>,
    rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    /// Cipher used for outgoing messages (our send direction).
    tx_cipher: Arc<Mutex<Option<SessionCipher>>>,
    /// Cipher used for incoming messages (our receive direction).
    rx_cipher: Arc<Mutex<Option<SessionCipher>>>,
    rtt_ms: Arc<AtomicU32>,
}

impl RelayTransport {
    /// Connect to relay. Cipher is initially None; install it after handshake.
    pub async fn connect(relay_url: &str, token: &str, role: &str) -> Result<Self> {
        let url = format!("{relay_url}/relay?token={token}&role={role}");
        // Log the relay endpoint and role without the token to avoid exposing
        // the session token (allows relay session hijacking if logs are leaked).
        info!("Relay connect: {relay_url}/relay?role={role}");

        let (ws_stream, _) = tokio::time::timeout(RELAY_CONNECT_TIMEOUT, connect_async(&url))
            .await
            .context("relay WebSocket connect timed out")?
            .context("relay WebSocket connect failed")?;
        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(128);
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(128);
        let rtt_ms = Arc::new(AtomicU32::new(0));

        tokio::spawn(async move {
            while let Some(data) = out_rx.recv().await {
                if ws_tx.send(Message::Binary(data)).await.is_err() {
                    break;
                }
            }
            let _ = ws_tx.close().await;
        });

        tokio::spawn(async move {
            while let Some(msg) = ws_rx.next().await {
                match msg {
                    Ok(Message::Binary(data)) => {
                        if in_tx.send(data).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            info!("Relay reader closed");
        });

        Ok(Self {
            tx: out_tx,
            rx: Arc::new(Mutex::new(in_rx)),
            tx_cipher: Arc::new(Mutex::new(None)),
            rx_cipher: Arc::new(Mutex::new(None)),
            rtt_ms,
        })
    }

    /// Install the session ciphers after handshake completes.
    /// `tx` is used for outgoing messages, `rx` for incoming.
    pub async fn install_ciphers(&self, tx: SessionCipher, rx: SessionCipher) {
        *self.tx_cipher.lock().await = Some(tx);
        *self.rx_cipher.lock().await = Some(rx);
    }

    /// Send a message — encrypted if cipher is installed, plaintext otherwise (handshake).
    ///
    /// Uses MessagePack (rmp-serde with named fields) instead of JSON.
    /// Binary payloads (VideoFrame.data, AudioFrame.data) are serialized as
    /// MessagePack bytes type rather than JSON integer arrays, reducing wire
    /// size ~3x for video frames (e.g. 90KB JSON → 30KB msgpack for a 30KB VP9 frame).
    pub async fn send_msg(&self, msg: &Msg) -> Result<()> {
        let encoded = rmp_serde::to_vec_named(msg)?;
        // ChaCha20-Poly1305 adds 8-byte seq + 16-byte tag = 24 bytes overhead.
        // The receiver rejects frames whose total (4-byte header + payload) exceeds
        // MAX_FRAME_BYTES. Mirror that limit here so we error before sending a frame
        // that the peer will reject, rather than confusing the session.
        const MAX_PLAINTEXT_BYTES: usize = 4 * 1024 * 1024 - 24;
        if encoded.len() > MAX_PLAINTEXT_BYTES {
            anyhow::bail!(
                "relay: outgoing message too large ({} bytes > {} limit)",
                encoded.len(),
                MAX_PLAINTEXT_BYTES
            );
        }
        let payload = match self.tx_cipher.lock().await.as_ref() {
            Some(c) => c.encrypt(&encoded)?,
            None => encoded,
        };
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend(payload);
        self.tx
            .send(frame)
            .await
            .map_err(|_| anyhow::anyhow!("relay send channel closed"))
    }

    pub async fn send_raw(&self, data: &[u8]) -> Result<()> {
        const MAX_PLAINTEXT_BYTES: usize = 4 * 1024 * 1024 - 24; // match recv cap minus AEAD tag
        if data.len() > MAX_PLAINTEXT_BYTES {
            anyhow::bail!(
                "relay: raw payload too large ({} bytes > {} limit)",
                data.len(),
                MAX_PLAINTEXT_BYTES
            );
        }
        let payload = match self.tx_cipher.lock().await.as_ref() {
            Some(c) => c.encrypt(data)?,
            None => data.to_vec(),
        };
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend(payload);
        self.tx
            .send(frame)
            .await
            .map_err(|_| anyhow::anyhow!("relay send channel closed"))
    }

    pub async fn recv_msg(&self) -> Result<Option<Msg>> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            None => Ok(None),
            Some(frame) => {
                if frame.len() < 4 {
                    return Ok(None);
                }
                // Reject oversized frames before any decrypt/deserialize allocation.
                // Mirrors the relay server's MAX_RELAY_MSG_BYTES cap; also guards the
                // future direct P2P path where no relay intermediary exists.
                const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024 + 4; // 4 MiB payload + 4-byte header
                if frame.len() > MAX_FRAME_BYTES {
                    return Err(anyhow::anyhow!(
                        "relay: received oversized frame ({} bytes > {} limit)",
                        frame.len(),
                        MAX_FRAME_BYTES
                    ));
                }
                let payload = &frame[4..];
                let plain = match self.rx_cipher.lock().await.as_ref() {
                    Some(c) => c.decrypt(payload)?,
                    None => payload.to_vec(),
                };
                Ok(Some(rmp_serde::from_slice(&plain)?))
            }
        }
    }

    pub fn rtt_ms(&self) -> u32 {
        self.rtt_ms.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl MsgChannel for RelayTransport {
    async fn send_msg(&mut self, msg: &Msg) -> Result<()> {
        Self::send_msg(self, msg).await
    }
    async fn recv_msg(&mut self) -> Result<Option<Msg>> {
        Self::recv_msg(self).await
    }
}

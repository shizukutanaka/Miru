//! Unified connection — wraps QUIC or WebSocket relay transparently.

use anyhow::Result;
use miru_common::{crypto::SessionCipher, message::Msg};
use rmp_serde;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tokio::sync::mpsc;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    QuicP2p,
    WebSocketRelay,
    WebRtc, // for browser clients
}

/// A live session connection — direction-agnostic (host or viewer).
pub struct Connection {
    pub mode: ConnectionMode,
    pub peer_addr: String,
    /// Cipher for outgoing (send) frames.
    tx_cipher: Arc<SessionCipher>,
    /// Cipher for incoming (recv) frames.
    rx_cipher: Arc<SessionCipher>,
    tx: mpsc::Sender<Vec<u8>>,
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>>,
    rtt: Arc<AtomicU32>,
}

impl Connection {
    pub fn new(
        mode: ConnectionMode,
        peer_addr: String,
        tx_cipher: SessionCipher,
        rx_cipher: SessionCipher,
        tx: mpsc::Sender<Vec<u8>>,
        rx: mpsc::Receiver<Vec<u8>>,
    ) -> Self {
        info!("Connection established: mode={:?} peer={}", mode, peer_addr);
        Self {
            mode,
            peer_addr,
            tx_cipher: Arc::new(tx_cipher),
            rx_cipher: Arc::new(rx_cipher),
            tx,
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            rtt: Arc::new(AtomicU32::new(0)),
        }
    }

    pub async fn send(&self, msg: &Msg) -> Result<()> {
        let json = rmp_serde::to_vec_named(msg)?;
        let encrypted = self.tx_cipher.encrypt(&json)?;

        // Frame: [4-byte LE length][payload]
        let len = encrypted.len() as u32;
        let mut frame = len.to_le_bytes().to_vec();
        frame.extend(encrypted);

        self.tx
            .send(frame)
            .await
            .map_err(|_| anyhow::anyhow!("send channel closed"))
    }

    pub async fn recv(&self) -> Result<Option<Msg>> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            None => Ok(None),
            Some(frame) => {
                if frame.len() < 4 {
                    return Ok(None);
                }
                const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024 + 4;
                if frame.len() > MAX_FRAME_BYTES {
                    return Err(anyhow::anyhow!(
                        "connection: oversized frame ({} bytes)",
                        frame.len()
                    ));
                }
                let payload = &frame[4..];
                let plain = self.rx_cipher.decrypt(payload)?;
                let msg: Msg = rmp_serde::from_slice(&plain)?;
                Ok(Some(msg))
            }
        }
    }

    pub fn rtt_ms(&self) -> u32 {
        self.rtt.load(Ordering::Relaxed)
    }

    pub fn update_rtt(&self, rtt: u32) {
        self.rtt.store(rtt, Ordering::Relaxed);
    }
}

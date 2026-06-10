//! Signal server client — registers device, requests connections, handles relay offers.

use anyhow::{Context, Result};
use base64::Engine as _;
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
        relay: bool,
        relay_addr: Option<String>,
    },
    Error {
        code: u16,
        message: String,
    },
    Disconnected,
}

impl SignalClient {
    pub async fn connect(
        signal_url: &str,
        device_id: &DeviceId,
        identity_pubkey: Option<&[u8; 32]>,
    ) -> Result<Self> {
        // Encode pubkey for registration (base64url, no padding).
        let pubkey_b64 = identity_pubkey
            .map(|k| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(k))
            .unwrap_or_default();

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
                                relay_addr: ack.host_addr,
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
        };

        // Register immediately
        client.register(device_id).await?;
        info!("Signal: registered as {}", device_id);
        Ok(client)
    }

    async fn register(&self, device_id: &DeviceId) -> Result<()> {
        self.send(Msg::Register(Register {
            device_id: device_id.0.clone(),
            pubkey: self.identity_pubkey.clone(),
        }))
        .await
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

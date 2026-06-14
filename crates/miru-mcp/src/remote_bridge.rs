//! Remote `HostBridge` — connects to a Miru host via the relay protocol.
//!
//! Uses `MIRU_SIGNAL` + `MIRU_HOST_DEVICE_ID` to open a Miru viewer session
//! and forwards MCP tool calls through the encrypted relay.

use anyhow::{bail, Context, Result};
use miru_auth::DeviceIdentity;
use miru_codec::{i420_to_jpeg, Decoder};
use miru_common::{
    message::{
        AudioCodec, ClipboardFormat, ClipboardSync, Features, InputEvent, Msg, VideoCodec,
    },
    session::DeviceId,
};
use miru_transport::{
    handshake::viewer_handshake,
    relay::RelayTransport,
    signaling::{SignalClient, SignalEvent},
};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};

use crate::server::HostBridge;

/// A `HostBridge` backed by a live Miru relay session.
pub struct RemoteBridge {
    relay: Arc<RelayTransport>,
    /// Latest decoded frame as PNG bytes (updated by background receive task).
    latest_png: Arc<Mutex<Option<Vec<u8>>>>,
    /// Latest clipboard text pushed by host (updated on ClipboardSync).
    latest_clipboard: Arc<Mutex<Option<String>>>,
    host_device_id: String,
}

impl RemoteBridge {
    /// Connect to `host_id` via the signal server at `signal_url`.
    pub async fn connect(signal_url: &str, host_id: &str) -> Result<Self> {
        let identity = DeviceIdentity::generate();
        let viewer_did = DeviceId::new();

        let mut signal = SignalClient::connect_signed(
            signal_url,
            &viewer_did,
            Some(identity.verifying_key.as_bytes()),
            Some(&identity.signing_key),
        )
        .await
        .context("signal connect failed")?;

        while let Some(evt) = signal.next_event().await {
            if matches!(evt, SignalEvent::Registered { .. }) {
                break;
            }
        }
        info!("RemoteBridge: registered as {}", viewer_did);

        signal.request_connect(host_id, "0.0.0.0").await?;

        // Wait for relay offer via ConnectAck then IncomingConnection
        let (relay_addr, relay_port, token) = loop {
            match signal.next_event().await {
                Some(SignalEvent::IncomingConnection { token, relay_addr, relay_port }) => {
                    break (relay_addr, relay_port, token);
                }
                Some(_) => continue,
                None => bail!("signal disconnected before relay offer"),
            }
        };
        info!("RemoteBridge: relay offer {}:{}", relay_addr, relay_port);

        let mut relay = RelayTransport::connect(
            &format!("ws://{relay_addr}:{relay_port}"),
            &token,
            "viewer",
        )
        .await?;

        let viewer_features = Features {
            codecs: vec![VideoCodec::Vp9, VideoCodec::Vp8, VideoCodec::Jpeg],
            audio_codecs: vec![AudioCodec::Pcm],
            hw_decode: false,
            hw_encode: false,
            clipboard: true,
            file_transfer: false,
            audio: false,
            multi_monitor: false,
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            viewer_handshake(&mut relay, &identity.signing_key, viewer_features),
        )
        .await
        .context("handshake timeout")?
        .context("handshake failed")?;

        relay.install_ciphers(result.tx, result.rx).await;
        let video_codec = result.selected_video_codec.clone();
        info!("RemoteBridge: session established (codec={:?})", video_codec);

        let relay = Arc::new(relay);
        let latest_png: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let latest_clipboard: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        {
            let relay = Arc::clone(&relay);
            let latest_png = Arc::clone(&latest_png);
            let latest_clipboard = Arc::clone(&latest_clipboard);
            tokio::spawn(async move {
                recv_loop(relay, latest_png, latest_clipboard, video_codec).await;
            });
        }

        Ok(Self {
            relay,
            latest_png,
            latest_clipboard,
            host_device_id: host_id.to_string(),
        })
    }
}

async fn recv_loop(
    relay: Arc<RelayTransport>,
    latest_png: Arc<Mutex<Option<Vec<u8>>>>,
    latest_clipboard: Arc<Mutex<Option<String>>>,
    codec: VideoCodec,
) {
    let mut decoder: Option<Decoder> = None;

    loop {
        match relay.recv_msg().await {
            Ok(Some(Msg::ClipboardSync(cs))) => {
                if cs.format == ClipboardFormat::Text {
                    const MAX_CLIP_BYTES: usize = 1024 * 1024; // 1 MiB
                    if cs.data.len() <= MAX_CLIP_BYTES {
                        if let Ok(text) = String::from_utf8(cs.data) {
                            *latest_clipboard.lock() = Some(text);
                        }
                    } else {
                        warn!("RemoteBridge: clipboard sync too large ({} bytes), ignored", cs.data.len());
                    }
                }
            }
            Ok(Some(Msg::VideoFrame(vf))) => {
                let jpeg = if vf.codec == VideoCodec::Jpeg {
                    Some(vf.data)
                } else {
                    if decoder.is_none() {
                        match Decoder::new(codec.clone()) {
                            Ok(d) => { decoder = Some(d); }
                            Err(e) => { warn!("RemoteBridge: decoder init: {e}"); continue; }
                        }
                    }
                    match decoder.as_mut().and_then(|d| d.decode(&vf.data, vf.timestamp_ms).ok().flatten()) {
                        Some(frame) => i420_to_jpeg(&frame, 85).ok(),
                        None => None,
                    }
                };
                if let Some(j) = jpeg {
                    match jpeg_to_png(&j) {
                        Ok(png) => *latest_png.lock() = Some(png),
                        Err(e) => warn!("RemoteBridge: JPEG→PNG conversion failed: {e}"),
                    }
                }
            }
            Ok(Some(Msg::Close(_))) | Ok(None) => {
                info!("RemoteBridge: relay closed");
                break;
            }
            Ok(Some(_)) => {}
            Err(e) => {
                warn!("RemoteBridge: recv: {e}");
                break;
            }
        }
    }
}

#[async_trait::async_trait]
impl HostBridge for RemoteBridge {
    async fn send_input(&self, evt: InputEvent) -> Result<()> {
        self.relay
            .send_msg(&Msg::InputEvent(evt))
            .await
            .context("send_input relay write")
    }

    async fn send_clipboard(&self, sync: ClipboardSync) -> Result<()> {
        self.relay
            .send_msg(&Msg::ClipboardSync(sync))
            .await
            .context("send_clipboard relay write")
    }

    async fn capture_screen(&self, _disp_idx: u8) -> Result<Vec<u8>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            {
                let guard = self.latest_png.lock();
                if let Some(png) = guard.as_ref() {
                    return Ok(png.clone());
                }
            }
            if std::time::Instant::now() > deadline {
                bail!("capture_screen: no frame received within 3s");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    async fn read_clipboard(&self) -> Result<String> {
        // Clear any stale cached value before sending the request. Without this,
        // the polling loop below would immediately return whatever the host
        // pushed in a previous request, not the response to this one.
        *self.latest_clipboard.lock() = None;

        // Ask the host to push its current clipboard; wait up to 2s for the response.
        self.relay.send_msg(&Msg::RequestClipboard).await.context("send RequestClipboard")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            {
                let guard = self.latest_clipboard.lock();
                if let Some(ref text) = *guard {
                    return Ok(text.clone());
                }
            }
            if std::time::Instant::now() > deadline {
                bail!("read_clipboard: no response from host within 2s");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    async fn open_url(&self, url: &str) -> Result<()> {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            bail!("open_url: only http(s) URLs allowed");
        }
        self.relay
            .send_msg(&Msg::OpenUrl(miru_common::message::OpenUrlRequest {
                url: url.to_string(),
            }))
            .await
            .context("send OpenUrl")
    }

    async fn status(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "bridge": "remote",
            "host_device_id": self.host_device_id,
            "has_frame": self.latest_png.lock().is_some(),
            "has_clipboard": self.latest_clipboard.lock().is_some(),
        }))
    }
}

fn jpeg_to_png(jpeg: &[u8]) -> Result<Vec<u8>> {
    use image::ImageReader;
    use std::io::Cursor;

    let img = ImageReader::new(Cursor::new(jpeg))
        .with_guessed_format()
        .context("JPEG format probe")?
        .decode()
        .context("JPEG decode")?;

    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .context("PNG encode")?;
    Ok(out)
}

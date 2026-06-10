//! Real host bridge for miru-mcp.
//!
//! Uses miru-capture and miru-input directly when $DISPLAY is available.
//! Falls back to StubBridge on headless/CI environments.
//!
//! Security: all calls still pass through the McpServer gate (rate limit +
//! capability check + confirmation) before reaching this code.

use anyhow::{bail, Context, Result};
use miru_common::message::{ClipboardSync, InputEvent};
use serde_json::Value;
use tracing::{debug, info};

use crate::server::HostBridge;

pub struct LocalBridge {
    disp: u8,
}

impl LocalBridge {
    pub fn new(disp: u8) -> Self {
        info!("LocalBridge: init disp={}", disp);
        Self { disp }
    }
}

#[async_trait::async_trait]
impl HostBridge for LocalBridge {
    async fn send_input(&self, evt: InputEvent) -> Result<()> {
        debug!("LocalBridge: send_input");
        tokio::task::spawn_blocking(move || miru_input::inject(&evt))
            .await
            .context("spawn_blocking input")?
    }

    async fn send_clipboard(&self, sync: ClipboardSync) -> Result<()> {
        debug!("LocalBridge: send_clipboard {} bytes", sync.data.len());
        let text = String::from_utf8_lossy(&sync.data).into_owned();
        tokio::task::spawn_blocking(move || miru_input::set_clipboard(&text))
            .await
            .context("spawn_blocking clipboard")?
    }

    async fn capture_screen(&self, _disp_idx: u8) -> Result<Vec<u8>> {
        let disp = self.disp;
        debug!("LocalBridge: capture_screen disp={}", disp);
        tokio::task::spawn_blocking(move || capture_frame(disp))
            .await
            .context("spawn_blocking capture")?
    }

    async fn read_clipboard(&self) -> Result<String> {
        tokio::task::spawn_blocking(miru_input::get_clipboard)
            .await
            .context("spawn_blocking get_clipboard")?
    }

    async fn open_url(&self, url: &str) -> Result<()> {
        let url = url.to_string();
        if !url.starts_with("https://") && !url.starts_with("http://") {
            bail!("open_url: only http/https allowed");
        }
        info!("LocalBridge: open_url {}", url);
        tokio::task::spawn_blocking(move || open::that(&url).context("open URL"))
            .await
            .context("spawn_blocking open_url")?
    }

    async fn status(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "transport": "local",
            "display_idx": self.disp,
            "platform": std::env::consts::OS,
        }))
    }
}

fn capture_frame(disp: u8) -> Result<Vec<u8>> {
    use miru_capture::ScreenCapturer;
    let mut cap = miru_capture::create_capturer().context("create capturer")?;
    cap.select_display(disp).context("select display")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match cap.next_frame().context("next_frame")? {
            Some(frame) => return encode_png(&frame.data, frame.width, frame.height),
            None => {
                if std::time::Instant::now() > deadline {
                    bail!("capture timeout: no frame within 2s");
                }
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }
    }
}

fn encode_png(data: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    use std::io::Cursor;
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(Cursor::new(&mut out), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().context("png header")?;
    writer.write_image_data(data).context("png data")?;
    drop(writer);
    Ok(out)
}

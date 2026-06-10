//! macOS screen capture.
//!
//! Strategy:
//!   macOS 14+ : ScreenCaptureKit (SCStream) — IOSurface, GPU-resident
//!   macOS 12.3–13 : ScreenCaptureKit (older API surface)
//!   macOS 10.15–12.2 : CGDisplayStream (deprecated but works)
//!
//! Permission: requires Screen Recording entitlement
//!   System Settings → Privacy & Security → Screen Recording → Miru ✓
//!   First launch triggers prompt automatically.

#![cfg(target_os = "macos")]

use anyhow::{bail, Result};
use bytes::Bytes;
use core_graphics::display::{CGDisplay, CGMainDisplayID};
use miru_common::message::DisplayInfo;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

use crate::{
    frame::{PixelFormat, RawFrame},
    ScreenCapturer,
};

pub struct MacosCapturer {
    current_display: u8,
    displays_cache: Vec<DisplayInfo>,
    /// SCStream handle stored as opaque pointer (managed via objc2 in real impl).
    /// For now: CGDisplayStream fallback path below.
    stream_state: Arc<Mutex<StreamState>>,
}

#[derive(Default)]
struct StreamState {
    latest_frame: Option<RawFrame>,
    width: u32,
    height: u32,
}

impl MacosCapturer {
    pub fn new() -> Result<Self> {
        let displays = Self::enumerate_displays()?;
        if displays.is_empty() {
            bail!("no displays found");
        }
        info!("macOS capture: {} display(s)", displays.len());

        let (w, h) = (displays[0].width, displays[0].height);
        let stream_state = Arc::new(Mutex::new(StreamState {
            latest_frame: None,
            width: w,
            height: h,
        }));

        let cap = Self {
            current_display: 0,
            displays_cache: displays,
            stream_state: Arc::clone(&stream_state),
        };

        // Start CGDisplayStream as a working fallback
        cap.start_cg_stream()?;

        Ok(cap)
    }

    fn enumerate_displays() -> Result<Vec<DisplayInfo>> {
        let active = CGDisplay::active_displays()
            .map_err(|e| anyhow::anyhow!("CGDisplay::active_displays: {:?}", e))?;

        let main_id = unsafe { CGMainDisplayID() };

        let infos: Vec<DisplayInfo> = active
            .iter()
            .enumerate()
            .filter_map(|(i, &id)| {
                let dsp = CGDisplay::new(id);
                let mode = dsp.display_mode()?;
                Some(DisplayInfo {
                    index: i as u8,
                    width: mode.width() as u32,
                    height: mode.height() as u32,
                    refresh_hz: mode.refresh_rate().max(60.0) as u8,
                    name: format!("Display {}", i),
                    primary: id == main_id,
                })
            })
            .collect();

        Ok(infos)
    }

    /// Start a CGDisplayStream — provides BGRA frames via callback.
    /// This is a deprecated API but works on macOS 10.15+ without entitlement issues.
    /// Real impl would use SCStream from ScreenCaptureKit on macOS 12.3+.
    fn start_cg_stream(&self) -> Result<()> {
        // CGDisplayStreamCreate with kCGDisplayStreamSourceRectKeepLocked
        // Callback writes BGRA into shared `latest_frame`
        // TODO: implement via core-foundation + Objective-C runtime
        warn!("macOS capture: stream init pending — using polling fallback");
        Ok(())
    }

    /// Polling fallback — uses CGDisplayCreateImage every frame.
    /// Slow (~20fps max @ 1080p) but works without ScreenCaptureKit binding.
    fn capture_via_cg_image(&self) -> Result<Option<RawFrame>> {
        use core_graphics::{
            display::CGDisplay,
            geometry::{CGPoint, CGRect, CGSize},
            image::CGImage,
        };

        let id = self.displays_cache[self.current_display as usize];
        let dsp = CGDisplay::new(unsafe { CGMainDisplayID() });
        let img = dsp.image();
        let img = match img {
            Some(i) => i,
            None => return Ok(None),
        };

        let w = img.width() as u32;
        let h = img.height() as u32;
        let bytes_per_row = img.bytes_per_row() as u32;

        // Get raw BGRA bytes
        let data = img.data();
        let bytes = data.bytes();
        let raw_bytes = Bytes::copy_from_slice(bytes);

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Ok(Some(RawFrame {
            display_idx: self.current_display,
            width: w,
            height: h,
            stride: bytes_per_row,
            format: PixelFormat::Bgra32,
            data: raw_bytes,
            timestamp_ms: ts,
            dirty_rects: RawFrame::full_dirty(w, h),
        }))
    }
}

impl ScreenCapturer for MacosCapturer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(self.displays_cache.clone())
    }

    fn select_display(&mut self, index: u8) -> Result<()> {
        if index as usize >= self.displays_cache.len() {
            bail!("display index {} out of range", index);
        }
        self.current_display = index;
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<RawFrame>> {
        // Try ScreenCaptureKit-delivered frame first (when implemented)
        if let Ok(state) = self.stream_state.lock() {
            if let Some(frame) = state.latest_frame.as_ref() {
                return Ok(Some(frame.clone_shallow()));
            }
        }
        // Fallback: polling capture
        self.capture_via_cg_image()
    }

    fn close(self) {
        // TODO: stop SCStream
    }
}

impl RawFrame {
    /// Cheap clone — `data` is Arc-counted Bytes.
    fn clone_shallow(&self) -> Self {
        Self {
            display_idx: self.display_idx,
            width: self.width,
            height: self.height,
            stride: self.stride,
            format: self.format,
            data: self.data.clone(),
            timestamp_ms: self.timestamp_ms,
            dirty_rects: self.dirty_rects.clone(),
        }
    }
}

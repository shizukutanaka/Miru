//! macOS screen capture.
//!
//! Current implementation: polling `CGDisplayCreateImage` once per frame. It
//! works on every supported macOS and needs no Objective-C bridging, but it
//! copies the whole framebuffer through the CPU each time and tops out around
//! 20fps at 1080p.
//!
//! Reaching 60fps means ScreenCaptureKit (SCStream), which delivers
//! GPU-resident IOSurfaces via a callback. That is not implemented: it needs
//! Objective-C interop that can only be written and verified on macOS, and
//! replacing a working path with an unverifiable one would be a regression
//! risk for the users who have this today. See docs/FEATURE_AUDIT.md item 5.
//!
//! Permission: requires Screen Recording entitlement
//!   System Settings → Privacy & Security → Screen Recording → Miru ✓
//!   First launch triggers prompt automatically.

#![cfg(target_os = "macos")]

use anyhow::{bail, Result};
use bytes::Bytes;
use core_graphics::display::{CGDisplay, CGMainDisplayID};
use miru_common::message::DisplayInfo;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

use crate::{
    frame::{PixelFormat, RawFrame},
    ScreenCapturer,
};

pub struct MacosCapturer {
    current_display: u8,
    displays_cache: Vec<DisplayInfo>,
}

impl MacosCapturer {
    pub fn new() -> Result<Self> {
        let displays = Self::enumerate_displays()?;
        if displays.is_empty() {
            bail!("no displays found");
        }
        info!("macOS capture: {} display(s)", displays.len());

        Ok(Self { current_display: 0, displays_cache: displays })
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
                    refresh_hz: mode.refresh_rate().max(0.0) as u8,
                    name: format!("Display {}", i),
                    primary: id == main_id,
                })
            })
            .collect();

        Ok(infos)
    }

    /// Capture one frame with CGDisplayCreateImage. ~20fps at 1080p; see the
    /// module header for why this is still the only path.
    fn capture_via_cg_image(&self) -> Result<Option<RawFrame>> {
        use core_graphics::{
            display::CGDisplay,
            geometry::{CGPoint, CGRect, CGSize},
            image::CGImage,
        };

        // Re-query the active display list to obtain the real CGDirectDisplayID
        // for the currently selected display index. Using CGMainDisplayID() would
        // always capture display 0, ignoring select_display() calls.
        let active = CGDisplay::active_displays()
            .map_err(|e| anyhow::anyhow!("CGDisplay::active_displays: {:?}", e))?;
        let display_id = active
            .get(self.current_display as usize)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("display index {} out of range", self.current_display))?;
        let dsp = CGDisplay::new(display_id);
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
        self.capture_via_cg_image()
    }

    fn close(self) {}
}

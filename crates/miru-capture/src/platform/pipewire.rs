//! PipeWire screen capture for Wayland.
//!
//! Flow:
//!   1. D-Bus → org.freedesktop.portal.ScreenCast.CreateSession
//!   2. SelectSources (monitor or window picker)
//!   3. Start (returns PipeWire node ID)
//!   4. Connect to PipeWire stream → receive DMA-BUF or SHM buffers
//!
//! Currently a stub. Real implementation requires:
//!   - zbus (D-Bus) for portal communication
//!   - pipewire-rs for stream subscription
//!
//! To enable: build with `--features pipewire`

#![cfg(all(target_os = "linux", feature = "pipewire"))]

use anyhow::{bail, Result};
use miru_common::message::DisplayInfo;

use crate::{frame::RawFrame, ScreenCapturer};

pub struct PipeWireCapturer {
    // TODO: zbus::Connection, pipewire::stream::Stream, etc.
}

impl PipeWireCapturer {
    pub fn new() -> Result<Self> {
        bail!(
            "PipeWire capture not yet implemented. \
             Run on X11 (unset WAYLAND_DISPLAY) or wait for v0.3."
        )
    }
}

impl ScreenCapturer for PipeWireCapturer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> { Ok(vec![]) }
    fn select_display(&mut self, _i: u8) -> Result<()> { Ok(()) }
    fn next_frame(&mut self) -> Result<Option<RawFrame>> { Ok(None) }
    fn close(self) {}
}

//! PipeWire screen capture via xdg-desktop-portal (Wayland).
//!
//! Flow:
//!   1. Open D-Bus session: org.freedesktop.portal.ScreenCast
//!   2. CreateSession() → session_handle
//!   3. SelectSources(types=Monitor, multi=false) → user picks display
//!   4. Start() → returns PipeWire stream node ID
//!   5. Open PipeWire stream from node ID
//!   6. Receive DMA-BUF or SHM frames via callback
//!
//! Implementation status: D-Bus skeleton + zbus integration deferred.
//! For now: bail with helpful error pointing to X11 fallback.

#![cfg(all(target_os = "linux", feature = "pipewire"))]

use anyhow::{bail, Result};
use miru_common::message::DisplayInfo;

use crate::frame::RawFrame;

pub struct PipeWireCapturer {
    // session_handle: zbus::ObjectPath<'static>,
    // stream: pipewire::stream::Stream,
}

impl PipeWireCapturer {
    pub async fn new() -> Result<Self> {
        // 1. zbus::Connection::session().await
        // 2. proxy = ScreenCastProxy::new(&conn).await
        // 3. session = proxy.create_session().await
        // 4. proxy.select_sources(session, types=Monitor)
        // 5. (await user selection)
        // 6. (open_pipe_wire_remote_fd → pipewire::Stream)
        bail!(
            "PipeWire capture not yet implemented.\n\
             Use X11 fallback by unsetting WAYLAND_DISPLAY, or wait for v0.3."
        )
    }

    pub fn displays(&self) -> Result<Vec<DisplayInfo>> { Ok(vec![]) }
    pub fn select_display(&mut self, _i: u8) -> Result<()> { Ok(()) }
    pub fn next_frame(&mut self) -> Result<Option<RawFrame>> { Ok(None) }
}

// Notes for full implementation (when ready):
//
// dependencies:
//   zbus = "4"        // D-Bus client
//   pipewire = "0.8"  // PipeWire bindings
//
// The xdg-portal interaction requires showing the user a system dialog
// for source selection — this is mandatory by Wayland security model.
// User picks the screen/window, returns a fd that opens the PipeWire stream.
//
// For DMA-BUF zero-copy:
//   stream params: spa::param::format::MediaType::Video,
//                  MediaSubtype::Raw,
//                  with PlaneCount=2 for NV12 (Y + UV interleaved)
//   On frame: dmabuf fd → import to wgpu texture / VAAPI surface
//
// VRR/HDR considerations:
//   PipeWire 0.3.55+ exposes refresh rate hints
//   HDR metadata is in the works (PipeWire 1.x)

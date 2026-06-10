//! Linux screen capture.
//!
//! Strategy:
//!   Wayland → PipeWire (xdg-desktop-portal ScreenCast)
//!   X11     → MIT-SHM extension (shared memory, avoids IPC copy)
//!
//! Runtime detection: check WAYLAND_DISPLAY env var.

#![cfg(target_os = "linux")]

use anyhow::{bail, Result};
use bytes::Bytes;
use miru_common::message::DisplayInfo;
use tracing::{info, warn};

use crate::{
    frame::{PixelFormat, RawFrame},
    ScreenCapturer,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct LinuxCapturer {
    inner: LinuxCapturerImpl,
    current_display: u8,
}

enum LinuxCapturerImpl {
    X11(X11Capturer),
    #[cfg(feature = "pipewire")]
    PipeWire(PipeWireCapturer),
}

impl LinuxCapturer {
    pub fn new() -> Result<Self> {
        let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

        #[cfg(feature = "pipewire")]
        if is_wayland {
            info!("Linux capture: PipeWire (Wayland)");
            return Ok(Self {
                inner: LinuxCapturerImpl::PipeWire(PipeWireCapturer::new()?),
                current_display: 0,
            });
        }

        if is_wayland {
            warn!("Wayland detected but PipeWire feature not enabled — falling back to X11");
            warn!("Rebuild with --features pipewire for Wayland support");
        }

        info!("Linux capture: X11 SHM");
        Ok(Self {
            inner: LinuxCapturerImpl::X11(X11Capturer::new()?),
            current_display: 0,
        })
    }
}

impl ScreenCapturer for LinuxCapturer {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        match &self.inner {
            LinuxCapturerImpl::X11(c) => c.displays(),
            #[cfg(feature = "pipewire")]
            LinuxCapturerImpl::PipeWire(c) => c.displays(),
        }
    }

    fn select_display(&mut self, index: u8) -> Result<()> {
        self.current_display = index;
        match &mut self.inner {
            LinuxCapturerImpl::X11(c) => c.select_display(index),
            #[cfg(feature = "pipewire")]
            LinuxCapturerImpl::PipeWire(c) => c.select_display(index),
        }
    }

    fn next_frame(&mut self) -> Result<Option<RawFrame>> {
        match &mut self.inner {
            LinuxCapturerImpl::X11(c) => c.next_frame(),
            #[cfg(feature = "pipewire")]
            LinuxCapturerImpl::PipeWire(c) => c.next_frame(),
        }
    }

    fn close(self) {}
}

// ─── X11 SHM capturer ─────────────────────────────────────────────────────────

struct X11Capturer {
    conn: x11rb::rust_connection::RustConnection,
    screen_num: usize,
    shm_seg: u32, // x11rb shm::Seg is just u32
    shm_id: i32,
    shm_ptr: *mut u8,
    width: u32,
    height: u32,
    current_display: u8,
}

unsafe impl Send for X11Capturer {}

impl X11Capturer {
    fn new() -> Result<Self> {
        use x11rb::connection::Connection;
        use x11rb::protocol::shm;

        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let width = screen.width_in_pixels as u32;
        let height = screen.height_in_pixels as u32;

        // Allocate shared memory
        let size = (width * height * 4) as usize;
        let shm_id = unsafe {
            libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600)
        };
        if shm_id < 0 { bail!("shmget failed"); }

        let shm_ptr = unsafe { libc::shmat(shm_id, std::ptr::null(), 0) as *mut u8 };
        if shm_ptr as isize == -1 { bail!("shmat failed"); }

        let shm_seg = conn.generate_id()?;
        shm::attach(&conn, shm_seg, shm_id as u32, false)?;
        conn.flush()?;

        Ok(Self {
            conn,
            screen_num,
            shm_seg,
            shm_id,
            shm_ptr,
            width,
            height,
            current_display: 0,
        })
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        use x11rb::connection::Connection;
        // TODO: query XRandR for multi-monitor info
        let screen = &self.conn.setup().roots[self.screen_num];
        Ok(vec![DisplayInfo {
            index: 0,
            width: screen.width_in_pixels as u32,
            height: screen.height_in_pixels as u32,
            refresh_hz: 60,
            name: ":0".to_string(),
            primary: true,
        }])
    }

    fn select_display(&mut self, index: u8) -> Result<()> {
        // XRandR CRTC selection — simplified for now
        self.current_display = index;
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<RawFrame>> {
        use x11rb::protocol::shm;
        use x11rb::connection::Connection;

        let screen = &self.conn.setup().roots[self.screen_num];
        let root = screen.root;

        let cookie = shm::get_image(
            &self.conn,
            root,
            0, 0,
            self.width as u16,
            self.height as u16,
            !0u32, // all planes
            2,    // ZPixmap
            self.shm_seg,
            0,
        )?;
        let _reply = cookie.reply()?;

        let size = (self.width * self.height * 4) as usize;
        let data = unsafe { std::slice::from_raw_parts(self.shm_ptr, size) };
        let bytes = Bytes::copy_from_slice(data);

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Ok(Some(RawFrame {
            display_idx: self.current_display,
            width: self.width,
            height: self.height,
            stride: self.width * 4,
            format: PixelFormat::Bgra32,
            data: bytes,
            timestamp_ms: ts,
            dirty_rects: RawFrame::full_dirty(self.width, self.height),
        }))
    }
}

impl Drop for X11Capturer {
    fn drop(&mut self) {
        
        use x11rb::protocol::shm;
        let _ = shm::detach(&self.conn, self.shm_seg);
        unsafe {
            libc::shmdt(self.shm_ptr as *const _);
            libc::shmctl(self.shm_id, libc::IPC_RMID, std::ptr::null_mut());
        }
    }
}

// ─── PipeWire stub ────────────────────────────────────────────────────────────

#[cfg(feature = "pipewire")]
struct PipeWireCapturer;

#[cfg(feature = "pipewire")]
impl PipeWireCapturer {
    fn new() -> Result<Self> {
        // TODO: implement xdg-desktop-portal ScreenCast D-Bus session
        // then PipeWire stream capture via pipewire-rs crate
        anyhow::bail!("PipeWire capture not yet implemented")
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> { Ok(vec![]) }
    fn select_display(&mut self, _i: u8) -> Result<()> { Ok(()) }
    fn next_frame(&mut self) -> Result<Option<RawFrame>> { Ok(None) }
}

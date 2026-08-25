//! Linux screen capture.
//!
//! Strategy:
//!   Wayland → PipeWire (xdg-desktop-portal ScreenCast)
//!   X11     → MIT-SHM extension (shared memory, avoids IPC copy)
//!
//! Runtime detection: check WAYLAND_DISPLAY env var.

#![cfg(target_os = "linux")]

use anyhow::{bail, Context, Result};
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
    /// Full root-window dimensions (the SHM buffer is sized for these,
    /// so any monitor sub-region is guaranteed to fit).
    width: u32,
    height: u32,
    current_display: u8,
    /// XRandR monitor geometries, refreshed in displays(). Empty when the
    /// server lacks RandR 1.5 — capture then falls back to the full root.
    monitors: Vec<MonitorGeometry>,
}

#[derive(Debug, Clone)]
struct MonitorGeometry {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    name: String,
    primary: bool,
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

        // Allocate shared memory — use usize arithmetic to avoid u32 overflow
        // (a 32K×32K screen would overflow u32 × 4 before the cast).
        const MAX_CAPTURE_DIM: u32 = 32768;
        if width > MAX_CAPTURE_DIM || height > MAX_CAPTURE_DIM {
            bail!("X11 screen {width}×{height} exceeds capture limit {MAX_CAPTURE_DIM}×{MAX_CAPTURE_DIM}");
        }
        let size = (width as usize) * (height as usize) * 4;
        let shm_id = unsafe { libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600) };
        if shm_id < 0 {
            bail!("shmget failed");
        }

        let shm_ptr = unsafe { libc::shmat(shm_id, std::ptr::null(), 0) as *mut u8 };
        if shm_ptr as isize == -1 {
            bail!("shmat failed");
        }

        let shm_seg = conn.generate_id()?;
        shm::attach(&conn, shm_seg, shm_id as u32, false)?;
        conn.flush()?;

        let mut capturer = Self {
            conn,
            screen_num,
            shm_seg,
            shm_id,
            shm_ptr,
            width,
            height,
            current_display: 0,
            monitors: Vec::new(),
        };
        capturer.refresh_monitors();
        Ok(capturer)
    }

    /// Query XRandR 1.5 monitors. Best-effort: on failure (old server,
    /// no RandR) `monitors` stays empty and we capture the full root.
    fn refresh_monitors(&mut self) {
        use x11rb::connection::Connection;
        use x11rb::protocol::randr;

        let root = self.conn.setup().roots[self.screen_num].root;
        let reply = match randr::get_monitors(&self.conn, root, true).map(|c| c.reply()) {
            Ok(Ok(r)) => r,
            _ => {
                warn!("XRandR get_monitors unavailable — exposing the full root as one display");
                self.monitors.clear();
                return;
            }
        };

        self.monitors = reply
            .monitors
            .iter()
            .map(|m| {
                let name = x11rb::protocol::xproto::get_atom_name(&self.conn, m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_else(|| "unknown".to_string());
                MonitorGeometry {
                    x: m.x,
                    y: m.y,
                    width: m.width,
                    height: m.height,
                    name,
                    primary: m.primary,
                }
            })
            .collect();
        info!(
            "XRandR: {} monitor(s): {:?}",
            self.monitors.len(),
            self.monitors.iter().map(|m| &m.name).collect::<Vec<_>>()
        );
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        use x11rb::connection::Connection;

        if !self.monitors.is_empty() {
            return Ok(self
                .monitors
                .iter()
                .enumerate()
                .map(|(i, m)| DisplayInfo {
                    index: i as u8,
                    width: m.width as u32,
                    height: m.height as u32,
                    // RandR monitors don't carry a refresh rate directly;
                    // reporting the common default until mode lookup lands.
                    refresh_hz: 60,
                    name: m.name.clone(),
                    primary: m.primary,
                })
                .collect());
        }

        // Fallback: single full-root display.
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
        self.refresh_monitors();
        if !self.monitors.is_empty() && (index as usize) >= self.monitors.len() {
            bail!(
                "display index {} out of range ({} monitor(s) present)",
                index,
                self.monitors.len()
            );
        }
        self.current_display = index;
        Ok(())
    }

    /// Capture region for the selected display: (x, y, w, h).
    fn capture_region(&self) -> (i16, i16, u16, u16) {
        match self.monitors.get(self.current_display as usize) {
            Some(m) => (m.x, m.y, m.width, m.height),
            None => (0, 0, self.width as u16, self.height as u16),
        }
    }

    fn next_frame(&mut self) -> Result<Option<RawFrame>> {
        use x11rb::connection::Connection;
        use x11rb::protocol::shm;

        let screen = &self.conn.setup().roots[self.screen_num];
        let root = screen.root;
        let (x, y, w, h) = self.capture_region();

        let cookie = shm::get_image(
            &self.conn,
            root,
            x,
            y,
            w,
            h,
            !0u32, // all planes
            2,     // ZPixmap
            self.shm_seg,
            0,
        )?;
        let _reply = cookie.reply()?;

        // The SHM buffer was sized for the full root, so any monitor
        // sub-region (w*h <= root w*h) fits.
        let size = (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(4))
            .context("capture region dimensions overflow")?;
        let data = unsafe { std::slice::from_raw_parts(self.shm_ptr, size) };
        let bytes = Bytes::copy_from_slice(data);

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Ok(Some(RawFrame {
            display_idx: self.current_display,
            width: w as u32,
            height: h as u32,
            stride: w as u32 * 4,
            format: PixelFormat::Bgra32,
            data: bytes,
            timestamp_ms: ts,
            dirty_rects: RawFrame::full_dirty(w as u32, h as u32),
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

// ─── PipeWire / xdg-desktop-portal capturer ──────────────────────────────────

/// Wayland capture: negotiate a ScreenCast grant through xdg-desktop-portal,
/// then consume frames from the PipeWire node it returns.
///
/// Both halves live in `platform/portal_ffi.rs` over a C shim; see that module
/// for why there is no Rust dependency behind this.
#[cfg(feature = "pipewire")]
struct PipeWireCapturer {
    /// Held for the lifetime of the capture: dropping it revokes the grant.
    _session: crate::platform::portal_ffi::ScreenCastSession,
    stream: crate::platform::portal_ffi::PipeWireVideoStream,
    current_display: u8,
}

#[cfg(feature = "pipewire")]
impl PipeWireCapturer {
    fn new() -> Result<Self> {
        use crate::platform::portal_ffi::{PipeWireVideoStream, ScreenCastSession};

        // The portal may show a picker, so this can block on the user. It runs
        // once at capture start, never per frame.
        let session = ScreenCastSession::open(false)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("xdg-desktop-portal would not grant screen capture")?;

        let stream = PipeWireVideoStream::open(Some(session.pipewire_fd()), session.node_id())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "portal granted node {} but the PipeWire stream would not open",
                    session.node_id()
                )
            })?;

        info!("Linux capture: PipeWire node {}", session.node_id());
        Ok(Self { _session: session, stream, current_display: 0 })
    }

    /// The portal grants one source, chosen by the user in its own picker, so
    /// there is nothing for the host to enumerate or switch between. Reporting
    /// a single display is honest; reporting several would offer the viewer a
    /// choice that `select_display` cannot act on.
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let (width, height) = self.stream.size().unwrap_or((0, 0));
        Ok(vec![DisplayInfo {
            index: 0,
            name: "PipeWire (portal)".into(),
            width,
            height,
            refresh_hz: 0,
            primary: true,
        }])
    }

    fn select_display(&mut self, index: u8) -> Result<()> {
        if index != 0 {
            anyhow::bail!(
                "the portal grants a single source; pick a different screen in \
                 the system dialog and reconnect"
            );
        }
        self.current_display = 0;
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<RawFrame>> {
        if self.stream.errored() {
            anyhow::bail!("PipeWire stream entered its error state");
        }
        let Some((width, height)) = self.stream.size() else {
            // Format not agreed yet; the caller polls again.
            return Ok(None);
        };
        let Some(pixels) = self.stream.next_frame() else {
            return Ok(None);
        };
        let expected = (width as usize) * (height as usize) * 4;
        if pixels.len() < expected {
            // A short buffer would be read past by the encoder.
            anyhow::bail!(
                "PipeWire delivered {} bytes for a {width}x{height} frame, expected {expected}",
                pixels.len()
            );
        }
        let data = Bytes::copy_from_slice(&pixels[..expected]);
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Ok(Some(RawFrame {
            display_idx: self.current_display,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Bgra32,
            data,
            timestamp_ms,
            // The portal reports no damage regions, so every frame is full.
            dirty_rects: RawFrame::full_dirty(width, height),
        }))
    }
}

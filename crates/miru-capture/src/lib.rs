//! Screen & audio capture — platform abstraction layer.
//!
//! WHY: Each OS has a fundamentally different capture API.
//!   Windows  → DXGI Desktop Duplication (GPU texture, zero-copy)
//!   macOS    → ScreenCaptureKit         (macOS 12.3+, AVAssetWriter path)
//!   Linux    → PipeWire (Wayland) / XShm (X11)
//!
//! MAP:
//!   ScreenCapturer  trait  →  platform/windows.rs
//!                          →  platform/macos.rs
//!                          →  platform/linux.rs
//!   AudioCapturer   trait  →  uses `cpal` (cross-platform)

pub mod audio;
pub mod display;
pub mod frame;

#[cfg(target_os = "windows")]
pub mod platform {
    pub mod windows;
    pub use windows::WindowsCapturer as PlatformCapturer;
}

#[cfg(target_os = "macos")]
pub mod platform {
    pub mod macos;
    pub use macos::MacosCapturer as PlatformCapturer;
}

#[cfg(target_os = "linux")]
pub mod platform {
    pub mod linux;
    pub use linux::LinuxCapturer as PlatformCapturer;
}

use anyhow::Result;
pub use frame::RawFrame;
use miru_common::message::DisplayInfo;

/// Synchronous capture trait — runs on a dedicated OS thread.
/// Returns raw BGRA/NV12 frames as fast as possible.
pub trait ScreenCapturer: Send {
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    fn select_display(&mut self, index: u8) -> Result<()>;

    /// Block until a new frame is available; returns None on timeout.
    fn next_frame(&mut self) -> Result<Option<RawFrame>>;

    /// Release GPU/system resources.
    fn close(self);
}

/// Convenience: create the best available capturer for this OS.
pub fn create_capturer() -> Result<impl ScreenCapturer> {
    platform::PlatformCapturer::new()
}

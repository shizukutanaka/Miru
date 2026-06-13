//! Input injection — platform abstraction.
//! Windows: SendInput | macOS: CGEvent | Linux: /dev/uinput

use anyhow::Result;
use miru_common::message::InputEvent;

#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod platform;

pub fn inject(event: &InputEvent) -> Result<()> {
    platform::inject(event)
}

pub fn set_clipboard(text: &str) -> Result<()> {
    platform::set_clipboard(text)
}

pub fn set_clipboard_raw(data: &[u8], mime_type: &str) -> Result<()> {
    platform::set_clipboard_raw(data, mime_type)
}

pub fn get_clipboard() -> Result<String> {
    platform::get_clipboard()
}

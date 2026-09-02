#![cfg(target_os = "windows")]

use anyhow::Result;
use miru_common::message::{InputEvent, InputKind, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// Absolute mouse coordinates, mapped across the virtual desktop.
///
/// MOUSEEVENTF_ABSOLUTE alone maps to the primary monitor — Microsoft's
/// MOUSEINPUT documentation is explicit about this — so a click meant for a
/// secondary display landed on the primary. VIRTUALDESK maps the same 0..65535
/// range across every monitor instead.
///
/// This is only correct because InputHandler now hands us coordinates already
/// normalised to the whole virtual desktop. Setting this flag on its own would
/// have moved the single-monitor case, which is why the two changes ship
/// together. See miru-common/src/display_map.rs.
const ABS_VIRTUAL: MOUSE_EVENT_FLAGS = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;

pub fn inject(event: &InputEvent) -> Result<()> {
    match &event.kind {
        InputKind::MouseMove { x, y, .. } => {
            mouse_event(MOUSEEVENTF_MOVE | ABS_VIRTUAL, to_abs(*x), to_abs(*y), 0)
        }
        InputKind::MouseDown { button, x, y, .. } => {
            let (flags, data) = mouse_down_flags(button);
            mouse_event(flags | ABS_VIRTUAL, to_abs(*x), to_abs(*y), data)
        }
        InputKind::MouseUp { button, x, y, .. } => {
            let (flags, data) = mouse_up_flags(button);
            mouse_event(flags | ABS_VIRTUAL, to_abs(*x), to_abs(*y), data)
        }
        InputKind::Scroll { dx, dy, .. } => {
            if *dy != 0.0 {
                mouse_event(MOUSEEVENTF_WHEEL, 0, 0, (*dy * 120.0) as i32)?;
            }
            if *dx != 0.0 {
                mouse_event(MOUSEEVENTF_HWHEEL, 0, 0, (*dx * 120.0) as i32)?;
            }
            Ok(())
        }
        // Windows VK and the browser keyCode largely coincide, so the legacy
        // path mostly worked here — but `code` is still preferred because it
        // is layout-independent and distinguishes left/right modifiers.
        InputKind::KeyDown { key, code, .. } => key_event(vk_code(*key, code), false),
        InputKind::KeyUp { key, code, .. } => key_event(vk_code(*key, code), true),
        InputKind::Text { text } => {
            for ch in text.chars() {
                unicode_key(ch)?;
            }
            Ok(())
        }
    }
}

fn to_abs(v: f32) -> i32 {
    (v * 65535.0) as i32
}

/// Thin wrapper around `SendInput` that converts a 0 return value (failure)
/// into an error using the OS-provided last error code (e.g. ERROR_ACCESS_DENIED
/// when UIPI blocks injection into a higher-privilege window).
fn send_input(input: INPUT) -> Result<()> {
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if sent == 0 {
        return Err(anyhow::anyhow!(
            "SendInput failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn mouse_event(flags: MOUSE_EVENT_FLAGS, x: i32, y: i32, data: i32) -> Result<()> {
    send_input(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: x,
                dy: y,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

fn key_event(vk: u32, key_up: bool) -> Result<()> {
    let flags = if key_up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    send_input(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

/// Resolve the Windows virtual-key code, preferring the W3C physical `code`.
fn vk_code(legacy_key: u32, code: &Option<String>) -> u32 {
    code.as_deref()
        .and_then(crate::keymap::code_to_vk)
        .map(u32::from)
        .unwrap_or(legacy_key)
}

fn unicode_key(ch: char) -> Result<()> {
    let mut buf = [0u16; 2];
    ch.encode_utf16(&mut buf);
    for &wch in buf.iter().take(ch.len_utf16()) {
        for key_up in [false, true] {
            let flags = if key_up {
                KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
            } else {
                KEYEVENTF_UNICODE
            };
            send_input(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: wch,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })?;
        }
    }
    Ok(())
}

// Returns (event_flags, mouseData). For X buttons, Win32 requires mouseData to
// carry XBUTTON1 (1) or XBUTTON2 (2) — passing 0 is invalid and silently no-ops.
fn mouse_down_flags(btn: &MouseButton) -> (MOUSE_EVENT_FLAGS, i32) {
    match btn {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, 0),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, 0),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, 0),
        MouseButton::X1 => (MOUSEEVENTF_XDOWN, 1), // XBUTTON1
        MouseButton::X2 => (MOUSEEVENTF_XDOWN, 2), // XBUTTON2
    }
}

fn mouse_up_flags(btn: &MouseButton) -> (MOUSE_EVENT_FLAGS, i32) {
    match btn {
        MouseButton::Left => (MOUSEEVENTF_LEFTUP, 0),
        MouseButton::Right => (MOUSEEVENTF_RIGHTUP, 0),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEUP, 0),
        MouseButton::X1 => (MOUSEEVENTF_XUP, 1), // XBUTTON1
        MouseButton::X2 => (MOUSEEVENTF_XUP, 2), // XBUTTON2
    }
}

pub fn set_clipboard(text: &str) -> Result<()> {
    cli_clipboard::set_contents(text.to_string()).map_err(|e| anyhow::anyhow!("clipboard set: {e}"))
}

pub fn set_clipboard_raw(data: &[u8], mime_type: &str) -> Result<()> {
    if mime_type.starts_with("text/") {
        let text = String::from_utf8_lossy(data);
        set_clipboard(&text)
    } else {
        tracing::warn!(
            "Clipboard format '{}' not supported on Windows (text only)",
            mime_type
        );
        Ok(())
    }
}

pub fn get_clipboard() -> Result<String> {
    cli_clipboard::get_contents().map_err(|e| anyhow::anyhow!("clipboard get: {e}"))
}

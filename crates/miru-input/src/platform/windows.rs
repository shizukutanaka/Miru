#![cfg(target_os = "windows")]

use anyhow::Result;
use miru_common::message::{InputEvent, InputKind, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub fn inject(event: &InputEvent) -> Result<()> {
    match &event.kind {
        InputKind::MouseMove { x, y, .. } => {
            mouse_event(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, to_abs(*x), to_abs(*y), 0)
        }
        InputKind::MouseDown { button, x, y } => {
            let flags = mouse_down_flags(button);
            mouse_event(flags, to_abs(*x), to_abs(*y), 0)
        }
        InputKind::MouseUp { button, x, y } => {
            let flags = mouse_up_flags(button);
            mouse_event(flags, to_abs(*x), to_abs(*y), 0)
        }
        InputKind::Scroll { dy, .. } => {
            mouse_event(MOUSEEVENTF_WHEEL, 0, 0, (*dy * 120.0) as i32)
        }
        InputKind::KeyDown { key, .. } => key_event(*key, false),
        InputKind::KeyUp { key, .. } => key_event(*key, true),
        InputKind::Text { text } => {
            for ch in text.chars() {
                unicode_key(ch)?;
            }
            Ok(())
        }
    }
}

fn to_abs(v: f32) -> i32 { (v * 65535.0) as i32 }

fn mouse_event(flags: MOUSE_EVENT_FLAGS, x: i32, y: i32, data: i32) -> Result<()> {
    let input = INPUT {
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
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    Ok(())
}

fn key_event(vk: u32, key_up: bool) -> Result<()> {
    let flags = if key_up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    let input = INPUT {
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
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    Ok(())
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
            let input = INPUT {
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
            };
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        }
    }
    Ok(())
}

fn mouse_down_flags(btn: &MouseButton) -> MOUSE_EVENT_FLAGS {
    match btn {
        MouseButton::Left   => MOUSEEVENTF_LEFTDOWN,
        MouseButton::Right  => MOUSEEVENTF_RIGHTDOWN,
        MouseButton::Middle => MOUSEEVENTF_MIDDLEDOWN,
        MouseButton::X1     => MOUSEEVENTF_XDOWN,
        MouseButton::X2     => MOUSEEVENTF_XDOWN,
    }
}

fn mouse_up_flags(btn: &MouseButton) -> MOUSE_EVENT_FLAGS {
    match btn {
        MouseButton::Left   => MOUSEEVENTF_LEFTUP,
        MouseButton::Right  => MOUSEEVENTF_RIGHTUP,
        MouseButton::Middle => MOUSEEVENTF_MIDDLEUP,
        MouseButton::X1     => MOUSEEVENTF_XUP,
        MouseButton::X2     => MOUSEEVENTF_XUP,
    }
}

pub fn set_clipboard(text: &str) -> Result<()> {
    cli_clipboard::set_contents(text.to_string())
        .map_err(|e| anyhow::anyhow!("clipboard set: {e}"))
}

pub fn get_clipboard() -> Result<String> {
    cli_clipboard::get_contents()
        .map_err(|e| anyhow::anyhow!("clipboard get: {e}"))
}

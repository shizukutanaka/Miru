#![cfg(target_os = "macos")]
//! macOS input injection via CoreGraphics CGEvent.
//!
//! Requires: Accessibility permission (System Preferences → Privacy → Accessibility)
//! Prompts automatically on first use.

use anyhow::Result;
use core_graphics::{
    display::CGDisplay,
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
        ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use miru_common::message::{InputEvent, InputKind, MouseButton};

fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("CGEventSource creation failed"))
}

pub fn inject(event: &InputEvent) -> Result<()> {
    let src = event_source()?;

    match &event.kind {
        InputKind::MouseMove { x, y, .. } => {
            let pt = screen_point(*x, *y);
            let ev =
                CGEvent::new_mouse_event(src, CGEventType::MouseMoved, pt, CGMouseButton::Left)
                    .map_err(|_| anyhow::anyhow!("mouse move event"))?;
            ev.post(CGEventTapLocation::HID);
        }

        InputKind::MouseDown { button, x, y, .. } => {
            let pt = screen_point(*x, *y);
            let (ev_type, cg_btn) = mouse_down_type(button);
            let ev = CGEvent::new_mouse_event(src, ev_type, pt, cg_btn)
                .map_err(|_| anyhow::anyhow!("mouse down event"))?;
            ev.post(CGEventTapLocation::HID);
        }

        InputKind::MouseUp { button, x, y, .. } => {
            let pt = screen_point(*x, *y);
            let (ev_type, cg_btn) = mouse_up_type(button);
            let ev = CGEvent::new_mouse_event(src, ev_type, pt, cg_btn)
                .map_err(|_| anyhow::anyhow!("mouse up event"))?;
            ev.post(CGEventTapLocation::HID);
        }

        InputKind::Scroll { dy, dx, .. } => {
            let ev = CGEvent::new_scroll_event(
                src,
                ScrollEventUnit::Line,
                2,
                (*dy * 3.0) as i32,
                (*dx * 3.0) as i32,
                0,
            )
            .map_err(|_| anyhow::anyhow!("scroll event"))?;
            ev.post(CGEventTapLocation::HID);
        }

        InputKind::KeyDown { key, modifiers, code } => {
            let ev = CGEvent::new_keyboard_event(src, cg_code(*key, code), true)
                .map_err(|_| anyhow::anyhow!("keydown event"))?;
            ev.set_flags(modifier_flags(*modifiers));
            ev.post(CGEventTapLocation::HID);
        }

        InputKind::KeyUp { key, modifiers, code } => {
            let ev = CGEvent::new_keyboard_event(src, cg_code(*key, code), false)
                .map_err(|_| anyhow::anyhow!("keyup event"))?;
            ev.set_flags(modifier_flags(*modifiers));
            ev.post(CGEventTapLocation::HID);
        }

        InputKind::Text { text } => {
            // Post Unicode text directly
            for ch in text.chars() {
                let src2 = event_source()?;
                if let Ok(ev) = CGEvent::new_keyboard_event(src2, 0, true) {
                    let s: Vec<u16> = ch.encode_utf16(&mut [0u16; 2]).to_vec();
                    ev.set_string_from_utf16_unchecked(&s);
                    ev.post(CGEventTapLocation::HID);
                }
            }
        }
    }
    Ok(())
}

/// Resolve the CGKeyCode for a key event. Prefers the W3C physical `code`;
/// falls back to the legacy raw cast for pre-`code` viewers. The raw cast is
/// wrong for nearly every key (CGKeyCode numbering is unrelated to the browser
/// keyCode), but preserving it keeps old viewers no worse off than before.
fn cg_code(legacy_key: u32, code: &Option<String>) -> CGKeyCode {
    code.as_deref()
        .and_then(crate::keymap::code_to_cgkeycode)
        .unwrap_or(legacy_key as CGKeyCode)
}

/// Map a virtual-desktop-normalised position to global CGEvent coordinates.
///
/// CGEvent works in a single global space spanning every display, so the whole
/// virtual desktop is the right denominator. This previously scaled by
/// `CGDisplay::main().bounds()`, which put every click on the primary screen no
/// matter which display the viewer had selected.
///
/// The bounds are taken from the OS rather than from the protocol's
/// DisplayInfo: CoreGraphics already knows the layout, and reading it here
/// keeps injection correct even if the captured layout is stale.
fn screen_point(x: f32, y: f32) -> CGPoint {
    let (origin, size) = virtual_desktop_bounds();
    CGPoint::new(
        origin.0 + f64::from(x) * size.0,
        origin.1 + f64::from(y) * size.1,
    )
}

/// Union of every active display's bounds, as ((x, y), (w, h)).
///
/// Falls back to the main display if enumeration fails — a wrong-but-usable
/// pointer beats dropping the event.
fn virtual_desktop_bounds() -> ((f64, f64), (f64, f64)) {
    let main = CGDisplay::main().bounds();
    let fallback = (
        (main.origin.x, main.origin.y),
        (main.size.width, main.size.height),
    );
    let Ok(active) = CGDisplay::active_displays() else {
        return fallback;
    };
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for id in active {
        let b = CGDisplay::new(id).bounds();
        min_x = min_x.min(b.origin.x);
        min_y = min_y.min(b.origin.y);
        max_x = max_x.max(b.origin.x + b.size.width);
        max_y = max_y.max(b.origin.y + b.size.height);
    }
    if !(min_x.is_finite() && min_y.is_finite()) || max_x <= min_x || max_y <= min_y {
        return fallback;
    }
    ((min_x, min_y), (max_x - min_x, max_y - min_y))
}

fn modifier_flags(mods: u8) -> CGEventFlags {
    let mut flags = CGEventFlags::empty();
    if mods & 0x01 != 0 {
        flags |= CGEventFlags::CGEventFlagShift;
    }
    if mods & 0x02 != 0 {
        flags |= CGEventFlags::CGEventFlagControl;
    }
    if mods & 0x04 != 0 {
        flags |= CGEventFlags::CGEventFlagAlternate;
    }
    if mods & 0x08 != 0 {
        flags |= CGEventFlags::CGEventFlagCommand;
    }
    flags
}

fn mouse_down_type(btn: &MouseButton) -> (CGEventType, CGMouseButton) {
    match btn {
        MouseButton::Left => (CGEventType::LeftMouseDown, CGMouseButton::Left),
        MouseButton::Right => (CGEventType::RightMouseDown, CGMouseButton::Right),
        _ => (CGEventType::OtherMouseDown, CGMouseButton::Center),
    }
}

fn mouse_up_type(btn: &MouseButton) -> (CGEventType, CGMouseButton) {
    match btn {
        MouseButton::Left => (CGEventType::LeftMouseUp, CGMouseButton::Left),
        MouseButton::Right => (CGEventType::RightMouseUp, CGMouseButton::Right),
        _ => (CGEventType::OtherMouseUp, CGMouseButton::Center),
    }
}

pub fn set_clipboard(text: &str) -> Result<()> {
    let status = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            if let Some(stdin) = c.stdin.as_mut() {
                stdin.write_all(text.as_bytes())?;
            }
            c.wait()
        })?;
    if !status.success() {
        anyhow::bail!("pbcopy exited with status {status}");
    }
    Ok(())
}

pub fn set_clipboard_raw(data: &[u8], mime_type: &str) -> Result<()> {
    if mime_type.starts_with("text/") {
        let text = String::from_utf8_lossy(data);
        set_clipboard(&text)
    } else {
        tracing::warn!("Clipboard format '{}' not supported on macOS (text only)", mime_type);
        Ok(())
    }
}

pub fn get_clipboard() -> Result<String> {
    let out = std::process::Command::new("pbpaste").output()?;
    if !out.status.success() {
        anyhow::bail!("pbpaste exited with status {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

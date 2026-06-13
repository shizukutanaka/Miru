//! Linux input injection via /dev/uinput virtual device.
//!
//! Works on both X11 and Wayland — kernel-level, no display server dependency.
//! Requires: user in `input` group or CAP_SYS_ADMIN, or udev rule.

use anyhow::{Context, Result};
use miru_common::message::{InputEvent, InputKind, MouseButton};
use std::sync::LazyLock as Lazy;
use std::{fs::OpenOptions, io::Write, os::unix::io::RawFd, sync::Mutex};
use tracing::warn;

// Linux uinput ioctl numbers and structs
const UI_DEV_CREATE: u64 = 0x5501;
const UI_DEV_DESTROY: u64 = 0x5502;
const UI_SET_EVBIT: u64 = 0x40045564;
const UI_SET_KEYBIT: u64 = 0x40045565;
const UI_SET_RELBIT: u64 = 0x40045566;
const UI_SET_ABSBIT: u64 = 0x40045567;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;

const SYN_REPORT: u16 = 0;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_WHEEL: u16 = 0x08;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

#[repr(C)]
struct InputEventRaw {
    time_sec: i64,
    time_usec: i64,
    type_: u16,
    code: u16,
    value: i32,
}

static UINPUT: Lazy<Mutex<Option<UinputDevice>>> =
    Lazy::new(|| Mutex::new(UinputDevice::new().ok()));

// Use a resolution-independent 0-65535 coordinate space for ABS events,
// same as the Windows MOUSEEVENTF_ABSOLUTE convention. The compositor
// maps these to actual screen pixels via the axis calibration reported
// in UI_ABS_SETUP, so no hardcoded screen resolution is needed.
const ABS_MAX: i32 = 65535;

struct UinputDevice {
    fd: RawFd,
}

impl UinputDevice {
    fn new() -> Result<Self> {
        use std::os::unix::io::IntoRawFd;

        let file = OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .context("/dev/uinput: check 'input' group membership or udev rules")?;

        let fd = file.into_raw_fd();
        unsafe {
            // Enable event types
            libc::ioctl(fd, UI_SET_EVBIT as libc::c_ulong, EV_KEY as libc::c_int);
            libc::ioctl(fd, UI_SET_EVBIT as libc::c_ulong, EV_REL as libc::c_int);
            libc::ioctl(fd, UI_SET_EVBIT as libc::c_ulong, EV_ABS as libc::c_int);
            libc::ioctl(fd, UI_SET_EVBIT as libc::c_ulong, EV_SYN as libc::c_int);

            // Enable mouse buttons
            for btn in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
                libc::ioctl(fd, UI_SET_KEYBIT as libc::c_ulong, btn as libc::c_int);
            }

            // Enable keyboard keys (0–255)
            for key in 0..=255i32 {
                libc::ioctl(fd, UI_SET_KEYBIT as libc::c_ulong, key);
            }

            // Enable relative axes
            libc::ioctl(fd, UI_SET_RELBIT as libc::c_ulong, REL_X as libc::c_int);
            libc::ioctl(fd, UI_SET_RELBIT as libc::c_ulong, REL_Y as libc::c_int);
            libc::ioctl(fd, UI_SET_RELBIT as libc::c_ulong, REL_WHEEL as libc::c_int);

            // Enable absolute axes (for absolute mouse positioning)
            libc::ioctl(fd, UI_SET_ABSBIT as libc::c_ulong, ABS_X as libc::c_int);
            libc::ioctl(fd, UI_SET_ABSBIT as libc::c_ulong, ABS_Y as libc::c_int);

            // Device info struct
            #[repr(C)]
            struct UinputSetup {
                id: [u16; 4],
                name: [u8; 80],
                ff_effects_max: u32,
            }
            let mut setup = UinputSetup {
                id: [0x3, 0x4711, 0x0001, 0], // BUS_USB, vendor, product
                name: [0u8; 80],
                ff_effects_max: 0,
            };
            let name = b"Miru Virtual Input Device";
            setup.name[..name.len()].copy_from_slice(name);

            const UI_DEV_SETUP: libc::c_ulong = 0x405c5503;
            libc::ioctl(fd, UI_DEV_SETUP, &setup as *const _);

            // Configure ABS axis ranges so X11/Wayland can map [0, ABS_MAX]
            // to actual screen coordinates — resolution-independent.
            #[repr(C)]
            struct InputAbsinfo {
                value: i32,
                minimum: i32,
                maximum: i32,
                fuzz: i32,
                flat: i32,
                resolution: i32,
            }
            #[repr(C)]
            struct UinputAbsSetup {
                code: u16,
                _pad: u16,
                absinfo: InputAbsinfo,
            }
            const UI_ABS_SETUP: libc::c_ulong = 0x40186504;
            for code in [ABS_X, ABS_Y] {
                let abs_setup = UinputAbsSetup {
                    code,
                    _pad: 0,
                    absinfo: InputAbsinfo {
                        value: 0,
                        minimum: 0,
                        maximum: ABS_MAX,
                        fuzz: 0,
                        flat: 0,
                        resolution: 0,
                    },
                };
                libc::ioctl(fd, UI_ABS_SETUP, &abs_setup as *const _);
            }

            libc::ioctl(fd, UI_DEV_CREATE as libc::c_ulong);

            Ok(Self { fd })
        }
    }

    fn write_event(&self, type_: u16, code: u16, value: i32) {
        let ev = InputEventRaw {
            time_sec: 0,
            time_usec: 0,
            type_,
            code,
            value,
        };
        unsafe {
            libc::write(
                self.fd,
                &ev as *const _ as *const libc::c_void,
                std::mem::size_of::<InputEventRaw>(),
            );
        }
    }

    fn syn(&self) {
        self.write_event(EV_SYN, SYN_REPORT, 0);
    }

    fn mouse_move_abs(&self, x: f32, y: f32) {
        let abs_x = (x * ABS_MAX as f32) as i32;
        let abs_y = (y * ABS_MAX as f32) as i32;
        self.write_event(EV_ABS, ABS_X, abs_x);
        self.write_event(EV_ABS, ABS_Y, abs_y);
        self.syn();
    }

    fn mouse_button(&self, btn: u16, down: bool) {
        self.write_event(EV_KEY, btn, if down { 1 } else { 0 });
        self.syn();
    }

    fn scroll(&self, dy: f32) {
        let v = if dy > 0.0 { 1 } else { -1 };
        self.write_event(EV_REL, REL_WHEEL, v);
        self.syn();
    }

    fn key(&self, scancode: u32, down: bool) {
        self.write_event(EV_KEY, scancode as u16, if down { 1 } else { 0 });
        self.syn();
    }
}

impl Drop for UinputDevice {
    fn drop(&mut self) {
        unsafe {
            libc::ioctl(self.fd, UI_DEV_DESTROY as libc::c_ulong);
            libc::close(self.fd);
        }
    }
}

pub fn inject(event: &InputEvent) -> Result<()> {
    // Mutex poisoning isn't catastrophic here — uinput state is replaceable.
    let guard = UINPUT.lock().unwrap_or_else(|p| p.into_inner());
    let dev = guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("uinput not available"))?;

    match &event.kind {
        InputKind::MouseMove { x, y, .. } => dev.mouse_move_abs(*x, *y),
        InputKind::MouseDown { button, x, y } => {
            dev.mouse_move_abs(*x, *y);
            dev.mouse_button(btn_code(button), true);
        }
        InputKind::MouseUp { button, x, y } => {
            dev.mouse_move_abs(*x, *y);
            dev.mouse_button(btn_code(button), false);
        }
        InputKind::Scroll { dy, .. } => dev.scroll(*dy),
        InputKind::KeyDown { key, .. } => dev.key(*key, true),
        InputKind::KeyUp { key, .. } => dev.key(*key, false),
        InputKind::Text { text } => {
            // Clipboard-paste: write text to clipboard, then inject Ctrl+V.
            // Works on X11 and Wayland desktop apps.
            // Must release the uinput lock before xclip (avoids deadlock in
            // tests that call inject() and set_clipboard() concurrently).
            drop(guard);
            return inject_text(text);
        }
    }
    Ok(())
}

/// Inject text by writing to clipboard then pressing Ctrl+V.
fn inject_text(text: &str) -> Result<()> {
    if let Err(e) = set_clipboard(text) {
        warn!("Text injection: clipboard write failed: {e}");
        return Ok(());
    }
    let guard = UINPUT.lock().unwrap_or_else(|p| p.into_inner());
    let dev = guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("uinput not available"))?;
    const KEY_LEFTCTRL: u32 = 29;
    const KEY_V: u32 = 47;
    dev.key(KEY_LEFTCTRL, true);
    dev.key(KEY_V, true);
    dev.key(KEY_V, false);
    dev.key(KEY_LEFTCTRL, false);
    Ok(())
}

fn btn_code(btn: &MouseButton) -> u16 {
    match btn {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
        MouseButton::X1 | MouseButton::X2 => BTN_MIDDLE,
    }
}

pub fn set_clipboard(text: &str) -> Result<()> {
    set_clipboard_raw(text.as_bytes(), "text/plain")
}

pub fn set_clipboard_raw(data: &[u8], mime_type: &str) -> Result<()> {
    std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", mime_type])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            if let Some(stdin) = c.stdin.as_mut() {
                stdin.write_all(data)?;
            }
            c.wait()
        })
        .context("xclip not found — install xclip for clipboard support")?;
    Ok(())
}

pub fn get_clipboard() -> Result<String> {
    let out = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .context("xclip not found")?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

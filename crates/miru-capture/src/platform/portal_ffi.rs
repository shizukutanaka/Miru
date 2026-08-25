//! Safe wrapper over the xdg-desktop-portal ScreenCast client
//! (`csrc/miru_portal.c`).
//!
//! This is the negotiation half of Wayland capture: it performs the portal
//! handshake and returns the PipeWire node id plus the file descriptor for the
//! PipeWire remote. Reading frames from that node is the other half and is not
//! implemented yet — see `docs/FEATURE_AUDIT.md` item 4.
//!
//! All `unsafe` in this crate's ffmpeg/portal FFI lives under `platform/`, per
//! the workspace rule. No crates.io dependency: the shim is plain C over GDBus
//! and libpipewire, compiled by build.rs.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

#[link(name = "gio-2.0")]
#[link(name = "glib-2.0")]
#[link(name = "gobject-2.0")]
#[link(name = "pipewire-0.3")]
extern "C" {
    fn miru_portal_open(include_cursor: c_int) -> *mut c_void;
    fn miru_portal_ok(p: *mut c_void) -> c_int;
    fn miru_portal_stage(p: *mut c_void) -> *const c_char;
    fn miru_portal_detail(p: *mut c_void) -> *const c_char;
    fn miru_portal_node_id(p: *mut c_void) -> c_uint;
    fn miru_portal_fd(p: *mut c_void) -> c_int;
    fn miru_portal_close(p: *mut c_void);
}

/// A negotiated ScreenCast session. Closing it revokes the grant.
#[derive(Debug)]
pub struct ScreenCastSession {
    handle: *mut c_void,
    node_id: u32,
    fd: i32,
}

/// Why the handshake failed, named by the step that refused.
///
/// The step matters to the user: "SelectSources was refused" means the
/// compositor would not offer a monitor, while a failure at `Start` usually
/// means the picker was dismissed or the backend could not begin capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalError {
    pub stage: String,
    pub detail: Option<String>,
}

impl std::fmt::Display for PortalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "xdg-desktop-portal ScreenCast failed at {}", self.stage)?;
        if let Some(d) = &self.detail {
            write!(f, ": {d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PortalError {}

fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

impl ScreenCastSession {
    /// Run the portal handshake: CreateSession, SelectSources, Start,
    /// OpenPipeWireRemote. `Start` may block while the compositor shows a
    /// picker, so this is not for a latency-sensitive path.
    pub fn open(include_cursor: bool) -> Result<Self, PortalError> {
        let handle = unsafe { miru_portal_open(c_int::from(include_cursor)) };
        if handle.is_null() {
            return Err(PortalError { stage: "allocate".into(), detail: None });
        }
        if unsafe { miru_portal_ok(handle) } != 1 {
            let err = PortalError {
                stage: cstr(unsafe { miru_portal_stage(handle) })
                    .unwrap_or_else(|| "unknown".into()),
                detail: cstr(unsafe { miru_portal_detail(handle) }),
            };
            unsafe { miru_portal_close(handle) };
            return Err(err);
        }
        let node_id = unsafe { miru_portal_node_id(handle) };
        let fd = unsafe { miru_portal_fd(handle) };
        Ok(Self { handle, node_id, fd })
    }

    /// PipeWire node id to stream from.
    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    /// File descriptor for the PipeWire remote. Owned by this session; it is
    /// closed on drop, so a consumer must not close it itself.
    pub fn pipewire_fd(&self) -> i32 {
        self.fd
    }
}

impl Drop for ScreenCastSession {
    fn drop(&mut self) {
        unsafe { miru_portal_close(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portal_present() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
    }

    /// Without a session bus there is no portal, and the failure must be a
    /// clean typed error naming the step — never a hang or a panic.
    #[test]
    fn fails_cleanly_with_a_named_stage_when_there_is_no_session() {
        if portal_present() {
            return;
        }
        let err = ScreenCastSession::open(false).expect_err("no portal should fail");
        assert!(!err.stage.is_empty(), "failure must name the step that refused");
        assert!(format!("{err}").contains("ScreenCast"));
    }

    /// With a live portal, the handshake must reach at least `Start` — failing
    /// earlier means the negotiation itself is wrong rather than the backend
    /// declining. Kept as an assertion on the *stage* so it is meaningful on
    /// machines where capture cannot actually begin.
    #[test]
    fn reaches_start_against_a_live_portal() {
        if !portal_present() {
            return;
        }
        match ScreenCastSession::open(false) {
            Ok(s) => {
                assert!(s.node_id() > 0, "a granted session must name a node");
                assert!(s.pipewire_fd() >= 0);
            }
            Err(e) => assert_eq!(
                e.stage, "Start",
                "handshake broke before Start ({e}) — CreateSession and \
                 SelectSources must succeed against any conforming portal"
            ),
        }
    }
}

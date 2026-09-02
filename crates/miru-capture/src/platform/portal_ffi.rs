//! Safe wrapper over the xdg-desktop-portal ScreenCast client
//! (`csrc/miru_portal.c`).
//!
//! Two pieces: `ScreenCastSession` performs the portal handshake and yields a
//! PipeWire node id plus a file descriptor for the PipeWire remote, and
//! `PipeWireVideoStream` consumes frames from that node.
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

    fn miru_pw_open(fd: c_int, node_id: c_uint) -> *mut c_void;
    fn miru_pw_take(
        s: *mut c_void,
        out: *mut u8,
        cap: c_int,
        seq: *mut u64,
        need: *mut c_int,
    ) -> c_int;
    fn miru_pw_errored(s: *mut c_void) -> c_int;
    fn miru_pw_width(s: *mut c_void) -> c_int;
    fn miru_pw_height(s: *mut c_void) -> c_int;
    fn miru_pw_close(s: *mut c_void);
}

/// A video stream being consumed from a PipeWire node.
///
/// Frames are latest-wins: the newest frame replaces any the caller has not
/// collected. A remote desktop never wants a backlog — replaying stale frames
/// adds latency without adding information — which is the same policy the
/// video pipeline already applies (ADR 0013).
#[derive(Debug)]
pub struct PipeWireVideoStream {
    handle: *mut c_void,
    seq: u64,
    buf: Vec<u8>,
}

// The handle owns its own PipeWire thread loop and is not shared.
unsafe impl Send for PipeWireVideoStream {}

impl PipeWireVideoStream {
    /// Connect to `node_id`. `fd` is the remote from a `ScreenCastSession`; use
    /// `None` to connect to the session's default PipeWire daemon.
    pub fn open(fd: Option<i32>, node_id: u32) -> Option<Self> {
        let handle = unsafe { miru_pw_open(fd.unwrap_or(-1), node_id) };
        if handle.is_null() {
            return None;
        }
        Some(Self { handle, seq: 0, buf: Vec::new() })
    }

    /// Negotiated frame size, or None until the format has been agreed.
    pub fn size(&self) -> Option<(u32, u32)> {
        let (w, h) = unsafe { (miru_pw_width(self.handle), miru_pw_height(self.handle)) };
        (w > 0 && h > 0).then_some((w as u32, h as u32))
    }

    /// True once the stream has entered its error state.
    pub fn errored(&self) -> bool {
        (unsafe { miru_pw_errored(self.handle) }) != 0
    }

    /// The newest frame, if one has arrived since the last call. Returns None
    /// when nothing new is waiting, which is the common case between frames.
    pub fn next_frame(&mut self) -> Option<&[u8]> {
        let mut need: c_int = 0;
        loop {
            let cap = self.buf.len() as c_int;
            let n = unsafe {
                miru_pw_take(self.handle, self.buf.as_mut_ptr(), cap, &mut self.seq, &mut need)
            };
            if n > 0 {
                return Some(&self.buf[..n as usize]);
            }
            if n == 0 {
                return None;
            }
            // Too small: the shim reported the size it needs. Grow and retry
            // rather than dropping the frame.
            if need <= 0 || need as usize <= self.buf.len() {
                return None;
            }
            self.buf.resize(need as usize, 0);
        }
    }
}

impl Drop for PipeWireVideoStream {
    fn drop(&mut self) {
        unsafe { miru_pw_close(self.handle) };
    }
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

    // The test-only producer, compiled by the offline gate but never by
    // build.rs — a capture library has no business synthesising video.
    extern "C" {
        fn miru_testsrc_start() -> *mut c_void;
        fn miru_testsrc_node_id(t: *mut c_void) -> c_uint;
        fn miru_testsrc_width() -> c_int;
        fn miru_testsrc_height() -> c_int;
        fn miru_testsrc_stop(t: *mut c_void);
    }

    fn pipewire_present() -> bool {
        std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| {
            std::path::Path::new(&d).join("pipewire-0").exists()
        })
    }

    /// Real frames, through a real PipeWire daemon: the test brings up its own
    /// producer node and asserts the consumer negotiates the format and
    /// receives whole frames. Needs no compositor and no portal.
    #[test]
    fn consumes_full_frames_from_a_real_pipewire_node() {
        if !pipewire_present() {
            return;
        }
        let src = unsafe { miru_testsrc_start() };
        assert!(!src.is_null(), "could not start the test producer");
        let node = unsafe { miru_testsrc_node_id(src) };
        let (w, h) = unsafe { (miru_testsrc_width() as u32, miru_testsrc_height() as u32) };
        assert!(node > 0, "producer never got a node id");

        let mut got = 0;
        {
            let mut s = PipeWireVideoStream::open(None, node).expect("consumer open");
            for _ in 0..200 {
                if let Some(f) = s.next_frame() {
                    assert_eq!(
                        f.len(),
                        (w * h * 4) as usize,
                        "frame is not a whole {w}x{h} BGRx image"
                    );
                    got += 1;
                    if got >= 3 {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert_eq!(s.size(), Some((w, h)), "format negotiation disagreed");
            assert!(!s.errored());
        }
        unsafe { miru_testsrc_stop(src) };
        assert!(got >= 3, "only {got} frames arrived");
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

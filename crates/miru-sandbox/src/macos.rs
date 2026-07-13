//! macOS sandboxing — guidance + best-effort runtime restrictions.
//!
//! macOS's primary sandboxing mechanism is the App Sandbox (entitlements
//! plist + code signature). At runtime there is no kernel API equivalent
//! to Linux landlock. The strongest protection comes from the bundle's
//! `Info.plist` + entitlements file; this module documents the required
//! plist entries and applies the few runtime hardenings that are available.
//!
//! ## Required entitlements (`miru.entitlements`)
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
//!     "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
//! <plist version="1.0">
//! <dict>
//!     <key>com.apple.security.app-sandbox</key>
//!     <true/>
//!     <key>com.apple.security.network.client</key>
//!     <true/>
//!     <key>com.apple.security.network.server</key>
//!     <true/>
//!     <key>com.apple.security.cs.allow-jit</key>
//!     <false/>
//!     <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
//!     <false/>
//!     <key>com.apple.security.cs.disable-library-validation</key>
//!     <false/>
//!     <key>com.apple.security.cs.disable-executable-page-protection</key>
//!     <false/>
//!     <key>com.apple.security.get-task-allow</key>
//!     <false/>
//! </dict>
//! </plist>
//! ```
//!
//! Plus in `Info.plist`:
//!
//! - `NSScreenCaptureUsageDescription` — string explaining why screen capture
//! - `NSAppleEventsUsageDescription` — for accessibility / input injection
//!
//! Sign with `--options runtime` (Hardened Runtime). This is enforced by
//! the bundling step, not by this module.

use crate::{Outcome, Policy};
use anyhow::Result;

pub fn apply(_policy: &Policy) -> Result<Outcome> {
    let mut o = Outcome::empty();

    // Disable core dumps (resource limit), the one thing we can do at
    // runtime. Code signing + entitlements are the primary controls.
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) };
    if rc == 0 {
        o.notes.push("RLIMIT_CORE=0 (no core dumps)".into());
    } else {
        o.notes.push(format!(
            "setrlimit(RLIMIT_CORE) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    // PT_DENY_ATTACH — refuse ptrace from any other process.
    // Provided by the libsystem ptrace via private constant 31.
    extern "C" {
        fn ptrace(request: i32, pid: i32, addr: *const i8, data: i32) -> i32;
    }
    const PT_DENY_ATTACH: i32 = 31;
    let rc2 = unsafe { ptrace(PT_DENY_ATTACH, 0, std::ptr::null(), 0) };
    if rc2 == 0 {
        o.notes.push("PT_DENY_ATTACH applied".into());
    } else {
        o.notes.push(format!(
            "ptrace(PT_DENY_ATTACH) failed: {} (often expected when running unsigned)",
            std::io::Error::last_os_error()
        ));
    }

    o.notes.push(
        "macOS sandbox depends on the bundle's entitlements plist; \
         this runtime layer is supplementary."
            .into(),
    );

    tracing::info!(
        notes = ?o.notes,
        "miru-sandbox applied (macos, supplementary)"
    );
    Ok(o)
}

//! Windows sandboxing — AppContainer integrity-level guidance.
//!
//! Windows process sandboxing is most effective when configured at process
//! creation time (CreateProcess with PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES).
//! For Miru this means the parent process — typically the host UI — should
//! launch the agent worker as a child in an AppContainer.
//!
//! The MSI / Squirrel installer should set the AppContainer SID and
//! capabilities at install time. This crate's runtime layer only drops
//! privileges from the current token.

use crate::{Outcome, Policy};
use anyhow::Result;

pub fn apply(_policy: &Policy) -> Result<Outcome> {
    let mut o = Outcome::empty();

    // Drop unneeded privileges from the current token — defense-in-depth.
    // We do NOT attempt to lower integrity from within a running process;
    // that requires re-launch. The installer should handle that.
    drop_privileges(&mut o);

    o.notes.push(
        "Windows AppContainer is configured at process creation; \
         see installer/wix/miru.wxs for SID + capabilities."
            .into(),
    );
    tracing::info!(
        notes = ?o.notes,
        "miru-sandbox applied (windows, supplementary)"
    );
    Ok(o)
}

fn drop_privileges(outcome: &mut Outcome) {
    use windows::core::PCWSTR;
    use windows::Win32::{
        Foundation::{CloseHandle, LUID},
        Security::{
            AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES,
            SE_PRIVILEGE_REMOVED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    let mut token = Default::default();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .is_err()
    {
        outcome
            .notes
            .push("Windows: OpenProcessToken failed".into());
        return;
    }

    // Privileges we don't need; remove them.
    let to_drop = [
        "SeShutdownPrivilege",
        "SeDebugPrivilege",
        "SeLoadDriverPrivilege",
        "SeSystemEnvironmentPrivilege",
        "SeRestorePrivilege",
        "SeTakeOwnershipPrivilege",
        "SeTcbPrivilege",
        "SeCreateTokenPrivilege",
    ];

    let mut dropped = 0;
    for name in to_drop {
        let mut wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut luid = LUID::default();
        if unsafe { LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(wide.as_mut_ptr()), &mut luid) }
            .is_err()
        {
            continue;
        }

        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_REMOVED,
            }],
        };
        if unsafe { AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None) }.is_ok() {
            dropped += 1;
        }
    }

    unsafe {
        let _ = CloseHandle(token);
    }

    outcome.notes.push(format!(
        "Windows: dropped {}/{} privileges",
        dropped,
        to_drop.len()
    ));
}

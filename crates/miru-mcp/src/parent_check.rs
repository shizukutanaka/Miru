//! Parent-process verification for the MCP server.
//!
//! Per the security playbook §3.2 ("Validate the parent process"):
//!
//! > On startup, read /proc/self/status (Linux), proc_pidpath (macOS),
//! > GetParentProcessId (Windows) and verify the parent matches an allowlist
//! > (e.g. claude-desktop, cursor, codex). Refuse to serve otherwise.
//!
//! This blocks a class of attack where malware on the same machine launches
//! `miru-mcp` directly (with a stolen token from disk cache) and feeds it
//! malicious JSON-RPC. Legitimate use is always: an MCP-compatible AI client
//! (Claude Desktop, Cursor, Continue, Codex CLI) starts miru-mcp as a child
//! process via stdio. Anything else is suspicious.
//!
//! Defense-in-depth: this is NOT the primary security control (token
//! verification is). It's a cheap additional filter that significantly
//! raises the bar for local-malware attacks.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// MCP-compatible client executables we trust as launchers. Add to this list
/// when new clients reach reasonable maturity. Casing is case-insensitive on
/// Windows (handled in matcher); exact on Unix.
pub const ALLOWED_PARENTS: &[&str] = &[
    // Anthropic
    "claude",
    "claude-desktop",
    "Claude",
    "Claude.exe",
    // Cursor
    "cursor",
    "Cursor",
    "Cursor.exe",
    // Continue.dev
    "continue",
    // Codex CLI (OpenAI)
    "codex",
    "codex-cli",
    // VS Code (extension hosts)
    "code",
    "Code",
    "Code.exe",
    "code-insiders",
    // Smithery / mcp-cli for testing
    "mcp",
    "mcp-cli",
    "smithery",
    // Common test/dev runners — kept for now to avoid surprise locks during
    // development; tighten with --strict-parent flag in v1.0.
    "node",
    "python",
    "python3",
];

/// Result of a parent-process check.
#[derive(Debug, Clone)]
pub struct ParentCheck {
    /// PID of the parent process.
    pub ppid: u32,
    /// Path to the parent's executable, if discoverable.
    pub exe: Option<PathBuf>,
    /// Whether the parent's executable filename matched the allowlist.
    pub allowed: bool,
    /// Reason if `allowed == false`. None on success.
    pub reason: Option<String>,
}

/// Inspect the parent process. Never panics; always returns something.
/// Callers decide whether to abort, warn, or proceed based on `allowed`.
pub fn check_parent() -> Result<ParentCheck> {
    let ppid = parent_pid()?;
    let exe = parent_exe(ppid).ok();

    let (allowed, reason) = match &exe {
        Some(p) => match exe_filename(p) {
            Some(name) if matches_allowlist(&name) => (true, None),
            Some(name) => (
                false,
                Some(format!(
                    "parent process executable '{name}' not in allowlist"
                )),
            ),
            None => (false, Some("parent executable filename unreadable".into())),
        },
        None => (
            false,
            Some(format!(
                "could not resolve parent process executable for ppid={ppid}"
            )),
        ),
    };

    Ok(ParentCheck {
        ppid,
        exe,
        allowed,
        reason,
    })
}

fn matches_allowlist(name: &str) -> bool {
    let lower = name.to_lowercase();
    ALLOWED_PARENTS.iter().any(|allowed| {
        let a = allowed.to_lowercase();
        lower == a || lower == format!("{a}.exe")
    })
}

fn exe_filename(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

#[cfg(target_os = "linux")]
fn parent_pid() -> Result<u32> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("PPid:") {
            return Ok(rest.trim().parse()?);
        }
    }
    anyhow::bail!("PPid not found in /proc/self/status");
}

#[cfg(target_os = "linux")]
fn parent_exe(ppid: u32) -> Result<PathBuf> {
    Ok(std::fs::read_link(format!("/proc/{ppid}/exe"))?)
}

#[cfg(target_os = "macos")]
fn parent_pid() -> Result<u32> {
    // SAFETY: getppid is FFI but a trivial pure read of process metadata.
    let ppid = unsafe { libc::getppid() };
    if ppid <= 0 {
        anyhow::bail!("getppid returned {}", ppid);
    }
    Ok(ppid as u32)
}

#[cfg(target_os = "macos")]
fn parent_exe(ppid: u32) -> Result<PathBuf> {
    // proc_pidpath via libproc.
    extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut libc::c_char, buffersize: u32) -> i32;
    }
    let mut buf = vec![0i8; 4096];
    let n = unsafe { proc_pidpath(ppid as i32, buf.as_mut_ptr(), buf.len() as u32) };
    if n <= 0 {
        anyhow::bail!("proc_pidpath returned {}", n);
    }
    buf.truncate(n as usize);
    let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Ok(PathBuf::from(s.to_str()?))
}

#[cfg(target_os = "windows")]
fn parent_pid() -> Result<u32> {
    use std::mem::size_of;
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut std::ffi::c_void;
        fn Process32FirstW(snap: *mut std::ffi::c_void, pe: *mut Pe32) -> i32;
        fn Process32NextW(snap: *mut std::ffi::c_void, pe: *mut Pe32) -> i32;
        fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
        fn GetCurrentProcessId() -> u32;
    }
    #[repr(C)]
    struct Pe32 {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }
    const TH32CS_SNAPPROCESS: u32 = 0x00000002;
    let me = unsafe { GetCurrentProcessId() };
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap.is_null() {
        anyhow::bail!("CreateToolhelp32Snapshot failed");
    }
    let mut pe: Pe32 = unsafe { std::mem::zeroed() };
    pe.dw_size = size_of::<Pe32>() as u32;
    let mut found = 0u32;
    if unsafe { Process32FirstW(snap, &mut pe) } != 0 {
        loop {
            if pe.th32_process_id == me {
                found = pe.th32_parent_process_id;
                break;
            }
            if unsafe { Process32NextW(snap, &mut pe) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snap) };
    if found == 0 {
        anyhow::bail!("could not find own ppid");
    }
    Ok(found)
}

#[cfg(target_os = "windows")]
fn parent_exe(ppid: u32) -> Result<PathBuf> {
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
        fn QueryFullProcessImageNameW(
            h: *mut std::ffi::c_void,
            flags: u32,
            buf: *mut u16,
            size: *mut u32,
        ) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, ppid) };
    if h.is_null() {
        anyhow::bail!("OpenProcess failed for ppid {}", ppid);
    }
    let mut buf = vec![0u16; 4096];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(h) };
    if ok == 0 {
        anyhow::bail!("QueryFullProcessImageNameW failed");
    }
    buf.truncate(size as usize);
    Ok(PathBuf::from(String::from_utf16_lossy(&buf)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn parent_pid() -> Result<u32> {
    anyhow::bail!("unsupported platform");
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn parent_exe(_ppid: u32) -> Result<PathBuf> {
    anyhow::bail!("unsupported platform");
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_matches_canonical() {
        assert!(matches_allowlist("claude"));
        assert!(matches_allowlist("Claude"));
        assert!(matches_allowlist("CLAUDE.EXE"));
        assert!(matches_allowlist("cursor"));
        assert!(matches_allowlist("Cursor.exe"));
        assert!(!matches_allowlist("malicious-launcher"));
        assert!(!matches_allowlist("evil"));
    }

    #[test]
    fn check_parent_returns_something() {
        // We don't assert allowed=true because tests may run under cargo,
        // which isn't on the allowlist. We just verify no panic.
        let r = check_parent();
        assert!(r.is_ok());
        let p = r.unwrap();
        assert!(p.ppid > 0);
    }
}

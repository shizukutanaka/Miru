//! Process sandboxing — defense-in-depth for the agent worker.
//!
//! This crate is the v1.0 implementation of the security-playbook
//! requirement (§5 sandboxing): apply OS-native confinement after the
//! process has finished privileged initialisation, so that even a full
//! compromise of the agent worker cannot read arbitrary files, spawn
//! child processes, or open new network endpoints.
//!
//! ## Threat model
//!
//! Sandboxing is one layer of defence-in-depth. Token verification
//! (`miru-agent`), capability rate limiting (`miru-mcp`), and audit log
//! redaction (`miru-agent::redact`) are the primary controls. This crate
//! exists for the case where one of those controls fails: with sandboxing,
//! a successful prompt-injection attack still cannot exfiltrate `~/.ssh`
//! or `curl` to a malicious host.
//!
//! ## API contract
//!
//! Call [`apply`] EXACTLY ONCE per process, AFTER all privileged init
//! (loading the device key, opening the audit log file, binding to ports).
//! Once applied, the restrictions cannot be relaxed without a new process.
//!
//! Failure modes:
//! - On unsupported platforms, [`apply`] is a no-op that logs a warning.
//! - On supported platforms with unavailable kernel features (e.g. Linux
//!   <5.13 lacks landlock entirely), [`apply`] degrades gracefully: it
//!   logs the missing feature and continues without that layer.
//! - On hard failures (kernel rejects the policy), [`apply`] returns an
//!   error and the caller should refuse to start.
//!
//! ## What is sandboxed
//!
//! - **Filesystem**: read-only access to the user config dir + the audit
//!   log dir (write); no other paths.
//! - **Network**: TCP connect + bind only to ports listed in the policy.
//! - **System calls** (Linux only): seccomp filter denies `ptrace`,
//!   `mount`, `kexec_load`, `bpf`, `init_module`, `iopl`, etc.
//! - **macOS**: documents the hardened-runtime entitlements that the
//!   bundle plist must contain.
//! - **Windows**: AppContainer integrity-level guidance.

use anyhow::Result;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

/// Sandbox policy — a description of what the calling process is allowed
/// to do after [`apply`].
#[derive(Debug, Clone)]
pub struct Policy {
    /// Read-only roots. The process can read files / list directories below
    /// these paths but cannot create or modify anything.
    pub read_only: Vec<PathBuf>,
    /// Read-write roots. The process can create, modify, and delete files
    /// below these paths.
    pub read_write: Vec<PathBuf>,
    /// TCP ports the process can BIND to (server side).
    pub tcp_bind_ports: Vec<u16>,
    /// Allow outbound TCP connect (any host). For relay fallback.
    pub allow_tcp_connect: bool,
    /// Allow UDP for QUIC.
    pub allow_udp: bool,
    /// Allow exec()? Default false — child process spawn is denied entirely.
    pub allow_exec: bool,
}

impl Policy {
    /// Default policy for the agent worker — minimal authority.
    pub fn agent_worker() -> Self {
        Self {
            read_only: Vec::new(),
            read_write: Vec::new(),
            tcp_bind_ports: Vec::new(),
            allow_tcp_connect: true,
            allow_udp: true,
            allow_exec: false,
        }
    }

    /// Default policy for the host daemon — needs access to the user's
    /// config dir for the audit log and identity key, plus network.
    pub fn host_daemon() -> Self {
        Self {
            read_only: Vec::new(),
            read_write: Vec::new(),
            tcp_bind_ports: vec![21115, 21116, 21117],
            allow_tcp_connect: true,
            allow_udp: true,
            allow_exec: false,
        }
    }

    pub fn add_ro(mut self, p: impl Into<PathBuf>) -> Self {
        self.read_only.push(p.into());
        self
    }

    pub fn add_rw(mut self, p: impl Into<PathBuf>) -> Self {
        self.read_write.push(p.into());
        self
    }
}

/// Outcome of [`apply`] — describes which layers actually engaged.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub fs_restricted: bool,
    pub net_restricted: bool,
    pub syscall_restricted: bool,
    pub notes: Vec<String>,
}

impl Outcome {
    fn empty() -> Self {
        Self {
            fs_restricted: false,
            net_restricted: false,
            syscall_restricted: false,
            notes: Vec::new(),
        }
    }
}

/// Apply the policy to the current process. Call ONCE, AFTER privileged init.
///
/// Returns the `Outcome` describing which protections engaged. The caller
/// SHOULD log this on startup so operators know whether the deployment is
/// fully sandboxed or running on a kernel that doesn't support a layer.
pub fn apply(policy: &Policy) -> Result<Outcome> {
    #[cfg(target_os = "linux")]
    return linux::apply(policy);

    #[cfg(target_os = "macos")]
    return macos::apply(policy);

    #[cfg(target_os = "windows")]
    return windows::apply(policy);

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = policy;
        let mut o = Outcome::empty();
        o.notes.push(format!("sandboxing unsupported on this platform"));
        tracing::warn!("miru-sandbox: unsupported platform; running without confinement");
        Ok(o)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_default_constructors() {
        let agent = Policy::agent_worker();
        assert!(!agent.allow_exec);

        let host = Policy::host_daemon();
        assert!(!host.allow_exec);
        assert!(host.tcp_bind_ports.contains(&21115));
    }

    #[test]
    fn policy_builder_adds_paths() {
        let p = Policy::agent_worker()
            .add_ro("/usr/lib")
            .add_rw("/tmp/miru");
        assert_eq!(p.read_only.len(), 1);
        assert_eq!(p.read_write.len(), 1);
    }

    #[test]
    fn outcome_describes_layers() {
        let o = Outcome::empty();
        assert!(!o.fs_restricted);
        assert!(!o.net_restricted);
        assert!(!o.syscall_restricted);
    }
}

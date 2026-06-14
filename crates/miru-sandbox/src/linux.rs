//! Linux sandboxing — landlock (FS + net) + seccomp (syscalls).
//!
//! Linux kernel requirements:
//! - Landlock ABI v3 (kernel 6.1+) for FS access rules
//! - Landlock ABI v4 (kernel 6.4+) for `AccessNet::BindTcp` / `ConnectTcp`
//! - seccomp BPF (kernel 3.5+) — universally available
//!
//! On older kernels, the layer is silently dropped and recorded in the
//! `Outcome.notes` so operators know to upgrade. We DO NOT refuse to start
//! on older kernels because that would penalise the user for kernel age
//! they may not control (managed VMs, corporate images).

use crate::{Outcome, Policy};
use anyhow::Result;
use landlock::{
    Access, AccessFs, AccessNet, NetPort, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, ABI,
};

pub fn apply(policy: &Policy) -> Result<Outcome> {
    let mut outcome = Outcome::empty();

    apply_landlock(policy, &mut outcome);
    apply_seccomp(policy, &mut outcome);

    tracing::info!(
        fs = outcome.fs_restricted,
        net = outcome.net_restricted,
        syscall = outcome.syscall_restricted,
        "miru-sandbox applied (linux)"
    );

    Ok(outcome)
}

fn apply_landlock(policy: &Policy, outcome: &mut Outcome) {
    // Try ABI v4 (network rules) first; fall back to v3 (FS only).
    let abi = ABI::V4;

    let fs_access = AccessFs::from_all(abi);
    let net_access = AccessNet::from_all(abi);

    // landlock 0.4 builder consumes self on add_rule. Use Option<_> shuttle
    // to thread the ruleset through loops without move-out errors.
    let mut rs_opt = match Ruleset::default()
        .handle_access(fs_access)
        .and_then(|rs| rs.handle_access(net_access))
        .and_then(|rs| rs.create())
    {
        Ok(r) => Some(r),
        Err(e) => {
            outcome.notes.push(format!("landlock unavailable: {e}"));
            return;
        }
    };

    // Read-only paths.
    for p in &policy.read_only {
        let Some(rs) = rs_opt.take() else {
            return;
        };
        match PathFd::new(p) {
            Ok(fd) => {
                let rule = PathBeneath::new(fd, AccessFs::ReadFile | AccessFs::ReadDir);
                match rs.add_rule(rule) {
                    Ok(r) => rs_opt = Some(r),
                    Err(e) => {
                        outcome
                            .notes
                            .push(format!("landlock ro {}: {}", p.display(), e));
                        return;
                    }
                }
            }
            Err(e) => {
                outcome
                    .notes
                    .push(format!("landlock open ro {}: {}", p.display(), e));
                rs_opt = Some(rs);
            }
        }
    }

    // Read-write paths.
    for p in &policy.read_write {
        let Some(rs) = rs_opt.take() else {
            return;
        };
        match PathFd::new(p) {
            Ok(fd) => {
                let rule = PathBeneath::new(fd, AccessFs::from_all(abi));
                match rs.add_rule(rule) {
                    Ok(r) => rs_opt = Some(r),
                    Err(e) => {
                        outcome
                            .notes
                            .push(format!("landlock rw {}: {}", p.display(), e));
                        return;
                    }
                }
            }
            Err(e) => {
                outcome
                    .notes
                    .push(format!("landlock open rw {}: {}", p.display(), e));
                rs_opt = Some(rs);
            }
        }
    }

    // Bind ports.
    for port in &policy.tcp_bind_ports {
        let Some(rs) = rs_opt.take() else {
            return;
        };
        let rule = NetPort::new(*port, AccessNet::BindTcp);
        match rs.add_rule(rule) {
            Ok(r) => rs_opt = Some(r),
            Err(e) => {
                outcome.notes.push(format!("landlock bind {port}: {e}"));
                return;
            }
        }
    }

    // Common outbound ports for TCP connect.
    if policy.allow_tcp_connect {
        for port in [80u16, 443, 21115, 21116, 21117, 3478, 5349] {
            let Some(rs) = rs_opt.take() else {
                return;
            };
            let rule = NetPort::new(port, AccessNet::ConnectTcp);
            match rs.add_rule(rule) {
                Ok(r) => rs_opt = Some(r),
                Err(e) => {
                    outcome.notes.push(format!("landlock connect {port}: {e}"));
                    return;
                }
            }
        }
    }

    // Engage.
    let Some(ruleset) = rs_opt else {
        return;
    };
    match ruleset.restrict_self() {
        Ok(status) => {
            outcome.fs_restricted = matches!(status.ruleset, RulesetStatus::FullyEnforced)
                || matches!(status.ruleset, RulesetStatus::PartiallyEnforced);
            outcome.net_restricted = outcome.fs_restricted;
            outcome
                .notes
                .push(format!("landlock status: {:?}", status.ruleset));
        }
        Err(e) => outcome.notes.push(format!("landlock restrict_self: {e}")),
    }
}

fn apply_seccomp(policy: &Policy, outcome: &mut Outcome) {
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch};

    let arch = match std::env::consts::ARCH {
        "x86_64" => TargetArch::x86_64,
        "aarch64" => TargetArch::aarch64,
        other => {
            outcome
                .notes
                .push(format!("seccomp: unsupported arch {other}"));
            return;
        }
    };

    // Hardcoded syscall numbers per arch. seccompiler 0.5 doesn't re-export
    // its sys_to_num helper, so we use canonical Linux numbers from
    // include/uapi/asm-generic/unistd.h and arch-specific tables.
    //
    // Format: (x86_64_nr, aarch64_nr, name). A negative number means
    // "this syscall doesn't exist on this arch" — skip silently.
    const DENY: &[(i64, i64, &str)] = &[
        // (x86_64, aarch64, name)
        (101, 117, "ptrace"),
        (165, 40, "mount"),
        (166, 39, "umount2"),
        (246, 104, "kexec_load"),
        (321, 280, "bpf"),
        (175, 105, "init_module"),
        (176, 106, "delete_module"), // x86_64: 176 (not 313 — that is finit_module)
        (172, -1, "iopl"),
        (173, -1, "ioperm"),
        (171, 162, "setdomainname"),
        (170, 161, "sethostname"),
        (135, 92, "personality"),
        (174, -1, "create_module"), // x86_64 only
        (313, 273, "finit_module"),
        (134, -1, "uselib"),
        (323, 282, "userfaultfd"),
        (227, 112, "clock_settime"),
        (164, 170, "settimeofday"),
    ];

    let mut rules: std::collections::BTreeMap<i64, Vec<SeccompRule>> = Default::default();

    for (x64, a64, _name) in DENY {
        let nr = match arch {
            TargetArch::x86_64 => *x64,
            TargetArch::aarch64 => *a64,
            _ => continue,
        };
        if nr < 0 {
            continue;
        }
        rules.insert(nr, Vec::new());
    }

    // Block exec / fork unless allowed.
    // NOTE: clone/clone3 are intentionally NOT blocked here because both
    // glibc's pthread_create and the Tokio runtime use them for thread
    // creation. Blocking them with KillProcess would kill the daemon the
    // first time the runtime grows its thread pool after the sandbox
    // engages. Blocking execve + execveat is sufficient to prevent any
    // child process from running a new program; fork + vfork are also
    // blocked as defence-in-depth (useless without exec, but belt+braces).
    if !policy.allow_exec {
        const EXEC_FAMILY: &[(i64, i64)] = &[
            // (x86_64, aarch64)
            (59, 221),  // execve
            (322, 281), // execveat
            (57, -1),   // fork (x86_64 only)
            (58, -1),   // vfork (x86_64 only)
        ];
        for (x64, a64) in EXEC_FAMILY {
            let nr = match arch {
                TargetArch::x86_64 => *x64,
                TargetArch::aarch64 => *a64,
                _ => continue,
            };
            if nr < 0 {
                continue;
            }
            rules.insert(nr, Vec::new());
        }
    }

    let filter = match SeccompFilter::new(
        rules,
        SeccompAction::Allow,       // default: pass through
        SeccompAction::KillProcess, // matched (denied) syscalls
        arch,
    ) {
        Ok(f) => f,
        Err(e) => {
            outcome.notes.push(format!("seccomp build: {e}"));
            return;
        }
    };

    let prog: BpfProgram = match filter.try_into() {
        Ok(p) => p,
        Err(e) => {
            outcome.notes.push(format!("seccomp compile: {e}"));
            return;
        }
    };

    if let Err(e) = seccompiler::apply_filter(&prog) {
        outcome.notes.push(format!("seccomp apply: {e}"));
        return;
    }

    outcome.syscall_restricted = true;
    outcome.notes.push("seccomp filter active".into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_compiles_without_apply() {
        // We don't actually call apply() in tests because it would lock
        // the test process. Just verify the policy types compile.
        let _p = Policy::agent_worker();
    }
}

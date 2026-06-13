//! Miru Host — picks up identity, registers, awaits viewers.

use anyhow::Result;
use miru_auth::{AclStore, DeviceIdentity};
use miru_capture::ScreenCapturer as _; // trait import for .displays()
use miru_common::session::DeviceId;
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::info;

mod agent_handler;
mod backpressure;
mod capture_loop;
#[allow(dead_code)]
mod headless;
mod input_handler;
mod metrics;
mod qos;
#[allow(dead_code)]
mod qos_bbr;
mod recording;
#[allow(dead_code)]
mod safe_fs;
mod session;
mod ui;

use session::HostConfig;

#[tokio::main]
async fn main() -> Result<()> {
    // Lightweight subcommand dispatch (no clap — zero-dep philosophy).
    // `miru-host audit verify|show [path]` inspects the agent audit log without
    // starting the daemon. Default (no args) runs the host daemon.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("audit") {
        return run_audit_command(&args[2..]);
    }
    if args.get(1).map(|s| s.as_str()) == Some("token") {
        return run_token_command(&args[2..]);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("MIRU_LOG").unwrap_or_else(|_| "miru_host=debug,info".to_string()),
        )
        .init();

    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("miru");
    std::fs::create_dir_all(&config_dir)?;

    // Identity (Ed25519, persistent)
    let identity = Arc::new(DeviceIdentity::load_or_create(
        &config_dir.join("identity"),
    )?);

    // ACL (TOFU peer database)
    let acl = Arc::new(Mutex::new(
        AclStore::load(&config_dir.join("acl.json")).unwrap_or_default(),
    ));

    // Device ID
    let device_id = load_or_create_device_id(&config_dir)?;

    info!("Miru Host v{}", miru_common::APP_VERSION);
    info!("Device ID:   {}", device_id);
    info!("Fingerprint: {}", identity.pubkey_fingerprint());

    // Probe capture — graceful on headless servers.
    match miru_capture::create_capturer() {
        Ok(cap) => match cap.displays() {
            Ok(displays) if displays.is_empty() => info!("No displays found (headless mode)"),
            Ok(displays) => {
                for d in &displays {
                    info!(
                        "Display {}: {}×{} @ {}Hz [{}]{}",
                        d.index,
                        d.width,
                        d.height,
                        d.refresh_hz,
                        d.name,
                        if d.primary { " (primary)" } else { "" }
                    );
                }
            }
            Err(e) => tracing::warn!("display enumeration: {}", e),
        },
        Err(e) => tracing::warn!(
            "Screen capture unavailable (headless?): {}. \
             Set DISPLAY=:99 and start Xvfb for screen sharing.",
            e
        ),
    }

    // Print available codecs
    let codecs = miru_codec::available_codecs();
    info!("Codecs: {:?}", codecs);
    let hw = miru_codec::probe_hw();
    if !hw.is_empty() {
        for h in &hw {
            info!("HW: {} → {:?}", h.name(), h.codecs());
        }
    }

    // Print startup banner (Device ID + fingerprint for out-of-band verification).
    ui::print_banner(&device_id, &identity);

    let signal_url = std::env::var("MIRU_SIGNAL")
        .unwrap_or_else(|_| "ws://signal.miru.app:21115/ws".to_string());
    info!("Signal: {}", signal_url);

    // Apply OS-level sandboxing AFTER all privileged init is done.
    // - Identity and ACL files are already open (read-write access captured).
    // - Network sockets are opened inside session::run — sandbox must allow TCP.
    // - Screen capture requires no additional filesystem paths on Linux/macOS.
    // We deliberately continue on sandbox failure (non-fatal) but log loudly so
    // operators know when a layer didn't engage.
    let sandbox_policy = miru_sandbox::Policy::host_daemon()
        .add_rw(&config_dir) // audit log, ACL, identity
        .add_ro("/usr") // dynamically-linked libs
        .add_ro("/lib")
        .add_ro("/lib64")
        .add_ro("/etc/ssl") // TLS CA bundles
        .add_ro("/etc/resolv.conf");
    match miru_sandbox::apply(&sandbox_policy) {
        Ok(outcome) => {
            info!(
                "Sandbox: fs={} net={} syscall={}",
                outcome.fs_restricted, outcome.net_restricted, outcome.syscall_restricted
            );
            for note in &outcome.notes {
                info!("  sandbox: {}", note);
            }
        }
        Err(e) => {
            tracing::warn!("Sandbox apply failed (non-fatal): {}", e);
        }
    }

    let config = HostConfig {
        identity,
        acl,
        config_dir,
    };
    session::run(device_id, signal_url, config).await
}

fn load_or_create_device_id(dir: &std::path::Path) -> Result<DeviceId> {
    let path = dir.join("device_id");
    if path.exists() {
        Ok(DeviceId(std::fs::read_to_string(&path)?.trim().to_string()))
    } else {
        let id = DeviceId::new();
        std::fs::write(&path, id.0.as_str())?;
        Ok(id)
    }
}

/// `miru-host audit <verify|show> [path]` — inspect the agent audit log.
/// Zero-dep CLI: no clap, just positional args. Defaults the log path to the
/// standard config dir if not given.
fn run_audit_command(args: &[String]) -> Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("help");
    let default_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("miru")
        .join("agent_audit.log");
    let path = args
        .get(1)
        .map(std::path::PathBuf::from)
        .unwrap_or(default_path);

    match sub {
        "verify" => {
            if !path.exists() {
                println!(
                    "No audit log at {} (no agent activity yet).",
                    path.display()
                );
                return Ok(());
            }
            match miru_agent::audit::verify_path(&path) {
                Ok(n) => {
                    println!("✓ Audit chain VALID: {n} entries, hash-chain intact.");
                    println!("  {}", path.display());
                    Ok(())
                }
                Err(e) => {
                    eprintln!("✗ Audit chain BROKEN: {e}");
                    eprintln!(
                        "  The log at {} has been tampered with or truncated.",
                        path.display()
                    );
                    std::process::exit(1);
                }
            }
        }
        "show" => {
            if !path.exists() {
                println!("No audit log at {}.", path.display());
                return Ok(());
            }
            let entries = match miru_agent::audit::read_all_verified(&path) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("✗ Cannot show — audit chain BROKEN: {e}");
                    std::process::exit(1);
                }
            };
            println!(
                "Audit log: {} ({} entries, chain verified)",
                path.display(),
                entries.len()
            );
            for e in &entries {
                let confirmed = match e.confirmed {
                    Some(true) => "confirmed",
                    Some(false) => "declined",
                    None => "-",
                };
                println!(
                    "  #{:<4} {:<14} {:<13} {:<9} {}",
                    e.seq,
                    format!("{:?}", e.capability),
                    format!("{:?}", e.outcome),
                    confirmed,
                    e.action,
                );
            }
            Ok(())
        }
        _ => {
            println!("Usage: miru-host audit <verify|show> [log-path]");
            println!("  verify  Check the hash-chain integrity of the audit log.");
            println!("  show    Print all audit entries (verifies chain first).");
            Ok(())
        }
    }
}

/// `miru-host token issue [--cap <c1,c2,...>] [--ttl-hours N] [--label L]`
/// Mints an agent capability token signed by this host's identity, for use in
/// an MCP client config (`MIRU_AGENT_TOKEN`). Without `--cap`, issues the safe
/// assistant default (screen read + pointer + typing; no shell/file write).
fn run_token_command(args: &[String]) -> Result<()> {
    use miru_agent::{AgentToken, Capability};
    use std::collections::HashSet;

    let sub = args.first().map(|s| s.as_str()).unwrap_or("help");
    if sub != "issue" {
        println!("Usage: miru-host token issue [--cap c1,c2,...] [--ttl-hours N] [--label L]");
        println!("  Capabilities: screen_read pointer_move pointer_click key_type key_combo");
        println!("                clipboard_read clipboard_write file_read file_write");
        println!("                shell_exec open_url");
        println!("  Default (no --cap): safe assistant set (read screen + pointer + typing).");
        return Ok(());
    }

    // Parse flags.
    let mut cap_str: Option<String> = None;
    let mut ttl_hours: u64 = 8;
    let mut label = "miru-cli".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--cap" => {
                cap_str = args.get(i + 1).cloned();
                i += 2;
            }
            "--ttl-hours" => {
                ttl_hours = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(8);
                i += 2;
            }
            "--label" => {
                label = args.get(i + 1).cloned().unwrap_or(label);
                i += 2;
            }
            other => {
                eprintln!("unknown flag: {other}");
                std::process::exit(2);
            }
        }
    }

    let caps: HashSet<Capability> = match cap_str {
        None => Capability::assistant_default(),
        Some(s) => {
            let parsed: HashSet<Capability> = s
                .split(',')
                .filter_map(|c| parse_capability(c.trim()))
                .collect();
            if parsed.is_empty() {
                eprintln!("no valid capabilities in '{s}'");
                std::process::exit(2);
            }
            parsed
        }
    };

    // Load this host's identity (same key the daemon verifies tokens against).
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("miru");
    std::fs::create_dir_all(&config_dir)?;
    let identity = DeviceIdentity::load_or_create(&config_dir.join("identity"))?;

    let token = AgentToken::issue(
        &identity.signing_key,
        label,
        caps.clone(),
        std::time::Duration::from_secs(ttl_hours * 3600),
        None,
    );

    // Print the token + guidance to stdout.
    let mut cap_names: Vec<String> = caps.iter().map(|c| format!("{c:?}")).collect();
    cap_names.sort();
    eprintln!("Issued agent token:");
    eprintln!("  capabilities: {}", cap_names.join(", "));
    eprintln!("  ttl:          {ttl_hours}h");
    eprintln!("  issuer:       {}", identity.pubkey_fingerprint());
    eprintln!("  Set this in your MCP client config as MIRU_AGENT_TOKEN:");
    // The token itself goes to stdout alone, so it can be piped/captured.
    println!("{token}");
    Ok(())
}

fn parse_capability(s: &str) -> Option<miru_agent::Capability> {
    use miru_agent::Capability::*;
    Some(match s {
        "screen_read" => ScreenRead,
        "pointer_move" => PointerMove,
        "pointer_click" => PointerClick,
        "key_type" => KeyType,
        "key_combo" => KeyCombo,
        "clipboard_read" => ClipboardRead,
        "clipboard_write" => ClipboardWrite,
        "file_read" => FileRead,
        "file_write" => FileWrite,
        "shell_exec" => ShellExec,
        "open_url" => OpenUrl,
        _ => return None,
    })
}

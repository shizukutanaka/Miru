//! `miru-mcp` binary.
//!
//! Configured in Claude Desktop's settings.json as:
//!   {
//!     "mcpServers": {
//!       "miru": {
//!         "command": "miru-mcp",
//!         "env": {
//!           "MIRU_AGENT_TOKEN": "miru-agent.<...>",
//!           "MIRU_SIGNAL": "ws://your-signal-server:21115/ws",
//!           "MIRU_HOST_DEVICE_ID": "ABCD-1234"
//!         }
//!       }
//!     }
//!   }
//!
//! The token is issued from the host's UI ("Allow AI" button) and copied
//! into Claude Desktop's config. The token's signing pubkey is fetched
//! out-of-band (currently embedded in the token; verified server-side).

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use clap::Parser;
use ed25519_dalek::VerifyingKey;
use miru_agent::{AgentSession, AgentToken, AuditLog, ConfirmFn, ConfirmRequest};
use miru_mcp::{
    run_stdio,
    server::{HostBridge, McpServer},
};
use std::{path::PathBuf, sync::Arc};
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "miru-mcp",
    about = "Miru MCP server — exposes Miru host control to Claude"
)]
struct Args {
    /// Agent token, normally provided via the MIRU_AGENT_TOKEN env var.
    /// Mint one with `miru-host token issue`.
    #[arg(env = "MIRU_AGENT_TOKEN")]
    token: Option<String>,

    /// Signal server URL. Defaults to a local server — Miru is self-hosted
    /// by design; point this at your own deployment.
    #[arg(long, env = "MIRU_SIGNAL", default_value = "ws://localhost:21115/ws")]
    signal: String,

    /// Target host's device ID.
    #[arg(long, env = "MIRU_HOST_DEVICE_ID")]
    host_id: Option<String>,

    /// Issuer pubkey (base64 url-safe). If unset, extracted from the token's
    /// `iss` claim — but verifying with the same key the token was signed by
    /// is *not* security; the token must be trusted out-of-band.
    #[arg(long, env = "MIRU_ISSUER_PUBKEY")]
    issuer_pubkey: Option<String>,

    /// Audit log file. Defaults to ~/.miru/agent-audit.log.
    #[arg(long, env = "MIRU_AUDIT_LOG")]
    audit: Option<PathBuf>,

    /// Skip parent-process verification. Use only for development.
    /// In production, miru-mcp should be launched by an MCP-compatible AI
    /// client (Claude Desktop, Cursor, Codex, etc.). See
    /// `miru_mcp::parent_check::ALLOWED_PARENTS`.
    /// Also honored via the MIRU_INSECURE_NO_PARENT_CHECK env var (any value).
    #[arg(long)]
    insecure_no_parent_check: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // MCP uses stdout for protocol; logs MUST go to stderr only.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(std::env::var("MIRU_LOG").unwrap_or_else(|_| "miru_mcp=info,info".into()))
        .init();

    let args = Args::parse();

    // The dev-only parent-check bypass is honored from either the CLI flag or
    // the env var (set to any non-empty value), avoiding clap's strict bool
    // parsing of env values like "1".
    let skip_parent_check = args.insecure_no_parent_check
        || std::env::var("MIRU_INSECURE_NO_PARENT_CHECK")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

    // Friendly guidance when no token is configured — the single most common
    // setup mistake. Point the user at exactly how to fix it.
    let token_str = match args.token.as_deref() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => {
            eprintln!("error: no agent token configured.");
            eprintln!();
            eprintln!("miru-mcp needs a capability token to control the host. Mint one with:");
            eprintln!("    miru-host token issue");
            eprintln!();
            eprintln!("then set it in your MCP client config (e.g. claude_desktop_config.json):");
            eprintln!("    \"env\": {{ \"MIRU_AGENT_TOKEN\": \"miru-agent.<...>\" }}");
            eprintln!();
            eprintln!("See the README \"Claude Desktop と統合\" section for the full example.");
            std::process::exit(2);
        }
    };

    // Defense-in-depth: verify the parent process is a known MCP client.
    // The token verification below is the primary security control; this
    // additional check raises the bar for local-malware attacks.
    match miru_mcp::parent_check::check_parent() {
        Ok(p) if p.allowed => {
            info!(
                "Parent process check passed (ppid={}, exe={:?})",
                p.ppid, p.exe
            );
        }
        Ok(p) if skip_parent_check => {
            warn!(
                "Parent process check FAILED but bypassed via --insecure-no-parent-check: \
                 ppid={}, exe={:?}, reason={:?}",
                p.ppid, p.exe, p.reason
            );
        }
        Ok(p) => {
            bail!(
                "parent process verification failed: {}. \
                 If you are running miru-mcp from a trusted MCP client that \
                 isn't recognised, set MIRU_INSECURE_NO_PARENT_CHECK=1 (not recommended). \
                 Detected: ppid={}, exe={:?}",
                p.reason.as_deref().unwrap_or("unknown reason"),
                p.ppid,
                p.exe
            );
        }
        Err(e) if skip_parent_check => {
            warn!("Parent check error bypassed: {}", e);
        }
        Err(e) => {
            bail!(
                "parent process verification could not run: {e}. \
                 Set MIRU_INSECURE_NO_PARENT_CHECK=1 to bypass (not recommended)."
            );
        }
    }

    // Parse + verify the agent token
    let issuer_pubkey = parse_issuer(&token_str, args.issuer_pubkey.as_deref())?;
    let token = AgentToken::parse_and_verify(&token_str, &issuer_pubkey)
        .context("agent token verification failed")?;

    info!(
        "Agent token verified: sub={} caps={:?} expires_in={}s",
        token.payload.sub,
        token.payload.caps,
        token.seconds_remaining(),
    );

    // Open audit log
    let audit_path = args.audit.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".miru/agent-audit.log")
    });
    let audit = Arc::new(AuditLog::open(&audit_path)?);

    // Apply minimal sandbox AFTER privileged init (audit log already open).
    sandbox_mcp(&audit_path);

    // Confirmation policy for capability authorization.
    //
    // AUTO-APPROVE (Normal / Conditional security level):
    //   ScreenRead, PointerMove, PointerClick, KeyType, KeyCombo
    //   These are standard interactive-assistant operations with low impact.
    //
    // AUTO-DENY (Dangerous security level — requires native UI dialog, v0.3):
    //   ClipboardRead, ClipboardWrite, FileRead, FileWrite, ShellExec, OpenUrl
    //
    // ClipboardRead is intentionally NOT auto-approved even though it is read-only:
    // clipboard frequently contains passwords, 2FA codes, and private keys. Token
    // issuance is not a per-use consent signal; the user must be prompted each time
    // so they are aware the AI is reading potentially-sensitive clipboard content.
    // (token.rs::security_level() classifies ClipboardRead as Dangerous, and
    // Capability::assistant_default() excludes it for this same reason.)
    let confirm: ConfirmFn = Arc::new(|req: &ConfirmRequest| {
        use miru_agent::token::Capability;
        let approved = matches!(
            req.capability,
            Capability::ScreenRead
                | Capability::PointerMove
                | Capability::PointerClick
                | Capability::KeyType
                | Capability::KeyCombo
        );
        if approved {
            tracing::debug!(
                "Auto-approved: agent={} cap={:?}",
                req.agent_label,
                req.capability
            );
        } else {
            warn!(
                "Auto-denied: agent={} cap={:?} summary={}",
                req.agent_label, req.capability, req.action_summary
            );
        }
        approved
    });

    let session = Arc::new(AgentSession::open(token, audit, confirm));

    // Connect to host. v0.1 uses a stub bridge; the real bridge is wired in
    // host_bridge.rs by miru-host.
    let disp_idx: u8 = std::env::var("MIRU_DISPLAY_IDX")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let bridge: Arc<dyn HostBridge> = if let Some(ref host_id) = args.host_id {
        // Remote bridge: connect to the host via signal server + relay.
        // Requires MIRU_HOST_DEVICE_ID + MIRU_SIGNAL env vars (or CLI flags).
        let signal_url = &args.signal;
        info!("RemoteBridge: connecting to {} via {}", host_id, signal_url);
        let rb = miru_mcp::remote_bridge::RemoteBridge::connect(signal_url, host_id)
            .await
            .context("RemoteBridge connect failed")?;
        Arc::new(rb)
    } else if std::env::var("DISPLAY").is_ok() || std::env::var("MIRU_FORCE_LOCAL").is_ok() {
        info!("LocalBridge: real capture/input on display {}", disp_idx);
        Arc::new(miru_mcp::local_bridge::LocalBridge::new(disp_idx))
    } else if std::env::var("MIRU_ALLOW_STUB").is_ok() {
        // Opt-in only: the stub returns fake data (1x1 PNG, no-op input),
        // which is useful for protocol testing but must never silently
        // masquerade as a real session.
        warn!("StubBridge: MIRU_ALLOW_STUB set — ALL operations return fake data");
        Arc::new(stub::StubBridge::new(args.host_id, args.signal))
    } else {
        bail!(
            "no host configured: set MIRU_SIGNAL + MIRU_HOST_DEVICE_ID for remote access, \
                 DISPLAY for local capture, or MIRU_ALLOW_STUB=1 for testing"
        );
    };

    let server = Arc::new(McpServer::new(session, bridge));

    run_stdio(server).await
}

fn parse_issuer(token: &str, issuer_override: Option<&str>) -> Result<VerifyingKey> {
    // Token format: miru-agent.<base64_payload>.<base64_sig>
    // The payload contains `iss` = base64 of the issuer's pubkey. We can extract
    // it without verifying first (we're parsing untrusted bytes only to get the
    // claimed key), then verify against it. A defense in depth: the user may
    // also pass `--issuer-pubkey` explicitly to refuse tokens claiming different
    // issuers.
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts[0] != "miru-agent" {
        bail!("malformed token (expected miru-agent.<payload>.<sig>)");
    }

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .context("token payload is not base64url")?;

    #[derive(serde::Deserialize)]
    struct ClaimedIssuer {
        iss: String,
    }
    let claimed: ClaimedIssuer =
        serde_json::from_slice(&payload_bytes).context("token payload is not valid JSON")?;

    let claimed_bytes = URL_SAFE_NO_PAD
        .decode(&claimed.iss)
        .context("iss claim is not base64url")?;
    let claimed_arr: [u8; 32] = claimed_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("iss is not 32 bytes"))?;

    if let Some(explicit) = issuer_override {
        let explicit_bytes = B64
            .decode(explicit)
            .or_else(|_| URL_SAFE_NO_PAD.decode(explicit))
            .context("--issuer-pubkey is not base64")?;
        if explicit_bytes != claimed_arr {
            bail!("token's iss does not match --issuer-pubkey (token claims a different signer)");
        }
    }

    VerifyingKey::from_bytes(&claimed_arr).map_err(|e| anyhow::anyhow!("issuer pubkey: {e}"))
}

// ─── Stub bridge (replaced by real transport in host integration) ────────────

mod stub {
    use super::*;
    use anyhow::Result;
    use miru_common::message::{ClipboardSync, InputEvent};
    use serde_json::Value;

    pub struct StubBridge {
        host_id: Option<String>,
        signal_url: String,
    }

    impl StubBridge {
        pub fn new(host_id: Option<String>, signal_url: String) -> Self {
            Self {
                host_id,
                signal_url,
            }
        }
    }

    #[async_trait::async_trait]
    impl HostBridge for StubBridge {
        async fn send_input(&self, evt: InputEvent) -> Result<()> {
            tracing::info!("stub: send_input {:?}", evt.kind);
            Ok(())
        }
        async fn send_clipboard(&self, _sync: ClipboardSync) -> Result<()> {
            tracing::info!("stub: send_clipboard");
            Ok(())
        }
        async fn capture_screen(&self, disp_idx: u8) -> Result<Vec<u8>> {
            tracing::info!("stub: capture_screen display={}", disp_idx);
            // Tiny PNG (1x1 transparent) for stub.
            Ok(vec![
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
                0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
                0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
                0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
                0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
            ])
        }
        async fn read_clipboard(&self) -> Result<String> {
            Ok("(stub clipboard)".into())
        }
        async fn open_url(&self, url: &str) -> Result<()> {
            tracing::info!("stub: open_url {}", url);
            Ok(())
        }
        async fn status(&self) -> Result<Value> {
            Ok(serde_json::json!({
                "transport": "stub",
                "host_id": self.host_id,
                "signal": self.signal_url,
            }))
        }
    }
}

/// Apply OS-level sandboxing for the MCP server process.
/// Called once, after the audit log file descriptor is open.
/// Non-fatal on failure — logs a warning rather than refusing to start.
fn sandbox_mcp(audit_path: &std::path::Path) {
    let audit_dir = audit_path.parent().unwrap_or(std::path::Path::new("."));
    let policy = miru_sandbox::Policy::agent_worker().add_rw(audit_dir); // read+write the audit log dir only
    match miru_sandbox::apply(&policy) {
        Ok(o) => tracing::info!(
            "MCP sandbox applied: fs={} syscall={}",
            o.fs_restricted,
            o.syscall_restricted
        ),
        Err(e) => tracing::warn!("MCP sandbox non-fatal: {}", e),
    }
}

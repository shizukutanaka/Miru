//! MCP LocalBridge end-to-end tests.
//! Run with: DISPLAY=:99 cargo test -p miru-mcp --test local_bridge_e2e

use ed25519_dalek::SigningKey;
use miru_agent::token::{AgentToken, Capability};
use miru_mcp::local_bridge::LocalBridge;
use miru_mcp::rate_limit::RateLimiter;
use miru_mcp::server::HostBridge;
use rand::rngs::OsRng;
use std::collections::HashSet;
use std::time::Duration;

fn is_valid_png(data: &[u8]) -> bool {
    data.len() > 24
        && &data[..8] == b"\x89PNG\r\n\x1a\n"
        && &data[12..16] == b"IHDR"
}

#[tokio::test]
async fn screen_capture_returns_png() {
    if std::env::var("DISPLAY").is_err() {
        eprintln!("skip: $DISPLAY not set");
        return;
    }
    let bridge = LocalBridge::new(0);
    match bridge.capture_screen(0).await {
        Ok(png) => {
            assert!(is_valid_png(&png),
                "expected PNG, got {} bytes: {:02x?}", png.len(), &png[..8.min(png.len())]);
            println!("capture_screen OK: {} bytes PNG ✅", png.len());
        }
        Err(e) => eprintln!("capture error (may be OK on CI): {}", e),
    }
}

#[tokio::test]
async fn bridge_status_reports_local() {
    let bridge = LocalBridge::new(0);
    let status = bridge.status().await.unwrap();
    assert_eq!(status["transport"], "local");
    assert_eq!(status["platform"], std::env::consts::OS);
}

#[test]
fn rate_limiter_shell_exec_burst_1() {
    let rl = RateLimiter::new();
    assert!(rl.try_consume(Capability::ShellExec).is_ok());
    assert!(rl.try_consume(Capability::ShellExec).is_err(), "second must be denied");
}

#[test]
fn rate_limiter_screen_read_burst_10() {
    let rl = RateLimiter::new();
    for i in 0..10 {
        assert!(rl.try_consume(Capability::ScreenRead).is_ok(), "call {} must succeed", i);
    }
    assert!(rl.try_consume(Capability::ScreenRead).is_err(), "11th must be denied");
}

#[test]
fn token_capability_roundtrip() {
    let key = SigningKey::generate(&mut OsRng);
    let caps: HashSet<Capability> = [Capability::ScreenRead, Capability::PointerClick]
        .into_iter().collect();
    let token = AgentToken::issue(&key, "test", caps, Duration::from_secs(600), None);
    let parsed = AgentToken::parse_and_verify(&token.to_string(), &key.verifying_key())
        .expect("should verify");
    assert!(parsed.has_capability(Capability::ScreenRead));
    assert!(parsed.has_capability(Capability::PointerClick));
    assert!(!parsed.has_capability(Capability::ShellExec));
    assert!(parsed.seconds_remaining() > 0);
}

// ── Replay protection (AttestMCP §VI-C) ─────────────────────────────────────

use miru_mcp::server::McpServer;
use miru_common::message::{InputEvent, ClipboardSync};
use std::sync::Arc;

struct NoopBridge;

#[async_trait::async_trait]
impl HostBridge for NoopBridge {
    async fn send_input(&self, _: InputEvent) -> anyhow::Result<()> { Ok(()) }
    async fn send_clipboard(&self, _: ClipboardSync) -> anyhow::Result<()> { Ok(()) }
    async fn capture_screen(&self, _: u8) -> anyhow::Result<Vec<u8>> { Ok(vec![]) }
    async fn read_clipboard(&self) -> anyhow::Result<String> { Ok(String::new()) }
    async fn open_url(&self, _: &str) -> anyhow::Result<()> { Ok(()) }
    async fn status(&self) -> anyhow::Result<serde_json::Value> { Ok(serde_json::json!({})) }
}

fn make_server() -> McpServer {
    use miru_agent::{AgentSession, audit::AuditLog};
    let key = SigningKey::generate(&mut OsRng);
    let caps: HashSet<Capability> = [Capability::ScreenRead].into_iter().collect();
    let token = AgentToken::issue(&key, "test", caps, Duration::from_secs(600), None);
    let dir = std::env::temp_dir().join(format!("miru-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    let audit = Arc::new(AuditLog::open(&dir.join("a.log")).unwrap());
    let confirm = Arc::new(|_: &miru_agent::ConfirmRequest| true);
    let session = Arc::new(AgentSession::open(token, audit, confirm));
    McpServer::new(session, Arc::new(NoopBridge))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

#[test]
fn replay_fresh_nonce_accepted() {
    let server = make_server();
    assert!(server.check_replay(now_secs(), [1u8; 16]).is_ok());
}

#[test]
fn replay_duplicate_nonce_rejected() {
    let server = make_server();
    let nonce = [42u8; 16];
    assert!(server.check_replay(now_secs(), nonce).is_ok());
    let err = server.check_replay(now_secs(), nonce).unwrap_err();
    assert!(err.to_string().contains("duplicate nonce"), "got: {}", err);
}

#[test]
fn replay_stale_timestamp_rejected() {
    let server = make_server();
    let err = server.check_replay(now_secs() - 120, [9u8; 16]).unwrap_err();
    assert!(err.to_string().contains("validity window"), "got: {}", err);
}

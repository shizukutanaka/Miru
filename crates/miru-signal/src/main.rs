//! Miru Signal Server — rendezvous (hbbs) + relay (hbbr) in one binary.
//!
//! Ports:
//!   21115/tcp — rendezvous WebSocket (/ws)
//!   21117/tcp — relay WebSocket (/relay?token=XXX)

use anyhow::Result;
use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use dashmap::DashMap;
use miru_common::message::{ConnectAck, Msg, RegisterAck, RelayOffer};
use serde::Deserialize;
use std::{net::{IpAddr, SocketAddr}, sync::Arc, time::{Duration, Instant}};
use tokio::sync::mpsc;
use tracing::{info, warn};
use rand::Rng;

// ─── State ────────────────────────────────────────────────────────────────────

/// Per-IP bucket for Connect request rate limiting.
struct ConnectBucket {
    count: u32,
    window_start: Instant,
}

/// Max Connect requests per IP per window.
const CONNECT_RATE_LIMIT: u32 = 20;
const CONNECT_RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct AppState {
    registry: Arc<DashMap<String, DeviceEntry>>,
    relay_sessions: Arc<DashMap<String, RelaySlot>>,
    /// Per-IP rate limiter for Connect requests (prevents connection flooding).
    connect_rate: Arc<DashMap<IpAddr, ConnectBucket>>,
    max_devices: usize,
    max_relay_sessions: usize,
    /// Port the relay WebSocket server is listening on (for advertising to peers).
    relay_port: u16,
    /// Public hostname/IP for advertising to peers — read once at startup.
    public_host: String,
}

struct DeviceEntry {
    tx: mpsc::Sender<Msg>,
    /// STUN-discovered public address of the host (filled in from Register.pub_addr).
    pub_addr: Option<String>,
    pub_port: Option<u16>,
    /// Base64url Ed25519 pubkey from the original signed Register, if any.
    /// Non-empty means this entry is "identity-locked": re-registrations must
    /// present a valid signature for the same pubkey or be rejected.
    locked_pubkey: Option<String>,
}

/// How long an issued relay token may sit completely unclaimed before the
/// slot is reaped. Generous compared to the 30s in-handler peer wait.
const UNCLAIMED_SLOT_TTL: Duration = Duration::from_secs(60);

/// Maximum number of simultaneously registered devices.
/// Prevents memory exhaustion from a flood of fake Register messages.
/// Tuned for typical self-hosted deployments (households / small teams).
/// Override via MIRU_MAX_DEVICES env var.
const DEFAULT_MAX_DEVICES: usize = 10_000;

/// Maximum number of simultaneous relay sessions.
const DEFAULT_MAX_RELAY_SESSIONS: usize = 5_000;

/// Maximum binary message size forwarded by the relay (bytes).
/// 4 MiB — larger than any realistic single frame at 4K JPEG,
/// prevents a single session from blocking the channel with a giant blob.
const MAX_RELAY_MSG_BYTES: usize = 4 * 1024 * 1024;

/// Two-slot relay session — waits for both host and viewer to connect.
struct RelaySlot {
    /// Raw byte sender for each side.
    host: Option<mpsc::Sender<Vec<u8>>>,
    viewer: Option<mpsc::Sender<Vec<u8>>>,
}

impl AppState {
    fn new(relay_port: u16) -> Self {
        let max_devices = std::env::var("MIRU_MAX_DEVICES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_DEVICES);
        let max_relay_sessions = std::env::var("MIRU_MAX_RELAY_SESSIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_RELAY_SESSIONS);
        let public_host = std::env::var("MIRU_PUBLIC_HOST")
            .unwrap_or_else(|_| "localhost".to_string());
        Self {
            registry: Arc::new(DashMap::new()),
            relay_sessions: Arc::new(DashMap::new()),
            connect_rate: Arc::new(DashMap::new()),
            max_devices,
            max_relay_sessions,
            relay_port,
            public_host,
        }
    }

    /// Returns true if the IP is within rate limit, false if it should be blocked.
    fn check_connect_rate(&self, ip: IpAddr) -> bool {
        let now = Instant::now();

        // Prune IPs whose window has long since expired to prevent unbounded growth.
        // Retain only entries where the window might still be active (with 2× slack
        // to avoid racing a legitimate near-window-boundary request).
        self.connect_rate
            .retain(|_, b| now.duration_since(b.window_start) < CONNECT_RATE_WINDOW * 2);

        let mut entry = self.connect_rate.entry(ip).or_insert_with(|| ConnectBucket {
            count: 0,
            window_start: now,
        });
        if now.duration_since(entry.window_start) >= CONNECT_RATE_WINDOW {
            entry.count = 0;
            entry.window_start = now;
        }
        if entry.count < CONNECT_RATE_LIMIT {
            entry.count += 1;
            true
        } else {
            false
        }
    }
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("MIRU_LOG").unwrap_or_else(|_| "miru_signal=info,warn".to_string()),
        )
        .init();

    let relay_port: u16 = std::env::var("MIRU_RELAY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(21117);
    let state = AppState::new(relay_port);

    let rdv = Router::new()
        .route("/ws", get(rendezvous_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(state.clone());

    let relay = Router::new()
        .route("/relay", get(relay_handler))
        .with_state(state.clone());

    // Ports are configurable via env for testing and multi-instance deployments.
    let rdv_port: u16 = std::env::var("MIRU_RDV_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(21115);
    let rdv_addr: SocketAddr = format!("0.0.0.0:{rdv_port}").parse()?;
    let relay_addr: SocketAddr = format!("0.0.0.0:{relay_port}").parse()?;

    info!("Rendezvous: {}", rdv_addr);
    info!("Relay:      {}", relay_addr);

    tokio::try_join!(
        run_server_with_connect_info(rdv, rdv_addr),
        run_server(relay, relay_addr),
    )?;
    Ok(())
}

async fn run_server(app: Router, addr: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await.map_err(Into::into)
}

async fn run_server_with_connect_info(app: Router, addr: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(Into::into)
}

// ─── Rendezvous ───────────────────────────────────────────────────────────────

async fn rendezvous_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |sock| rendezvous_session(sock, s, peer.ip()))
}

/// Interval between server-initiated WebSocket pings.
const RENDEZVOUS_PING_INTERVAL: Duration = Duration::from_secs(30);
/// Max time to wait for a single WebSocket send before treating the peer as
/// wedged (stopped reading but hasn't closed the TCP connection). Without
/// this, a client that fills its receive buffer and never drains it can
/// stall a session task — and its registry/relay-slot cleanup — indefinitely.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);

async fn rendezvous_session(mut sock: WebSocket, state: AppState, peer_ip: IpAddr) {
    let (tx, mut rx) = mpsc::channel::<Msg>(32);
    let mut my_id: Option<String> = None;
    let mut ping_ticker = tokio::time::interval(RENDEZVOUS_PING_INTERVAL);
    ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // Forward outbound messages to WebSocket
            Some(msg) = rx.recv() => {
                let Ok(json) = serde_json::to_string(&msg) else { continue };
                match tokio::time::timeout(SEND_TIMEOUT, sock.send(Message::Text(json))).await {
                    Ok(Ok(())) => {}
                    _ => break,
                }
            }
            // Handle inbound WebSocket messages
            incoming = sock.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Msg>(&text) {
                            Ok(m) => process_rdv_msg(&state, m, &tx, &mut my_id, peer_ip).await,
                            Err(e) => warn!("parse error: {e}"),
                        }
                    }
                    Some(Ok(Message::Ping(d))) => {
                        let _ = tokio::time::timeout(SEND_TIMEOUT, sock.send(Message::Pong(d))).await;
                    }
                    Some(Ok(Message::Pong(_))) => {} // keepalive echo
                    _ => break,
                }
            }
            // Server-initiated keepalive ping to detect dead connections.
            _ = ping_ticker.tick() => {
                match tokio::time::timeout(SEND_TIMEOUT, sock.send(Message::Ping(vec![]))).await {
                    Ok(Ok(())) => {}
                    _ => break,
                }
            }
        }
    }

    // Cleanup on disconnect
    if let Some(id) = my_id {
        state.registry.remove(&id);
        info!("Device {} disconnected", id);
    }
}

async fn process_rdv_msg(
    state: &AppState,
    msg: Msg,
    tx: &mpsc::Sender<Msg>,
    my_id: &mut Option<String>,
    peer_ip: IpAddr,
) {
    match msg {
        Msg::Register(reg) => {
            // Rate-limit Register the same way Connect already is: each Register
            // performs an Ed25519 signature verification (non-trivial CPU cost)
            // plus a registry insert/remove, so a single connection looping
            // Register requests would otherwise have no per-IP throttle.
            if !state.check_connect_rate(peer_ip) {
                warn!("Register rate limit exceeded for {}", peer_ip);
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 429,
                        message: "too many requests — retry later".to_string(),
                    }))
                    .await;
                return;
            }
            // Reject absurdly long device IDs before they reach the DashMap.
            // A UUID hex + optional prefix is always < 128 bytes; 256 is generous.
            if reg.device_id.len() > 256 {
                warn!("Register rejected: device_id too long ({} bytes)", reg.device_id.len());
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 400,
                        message: "device_id too long".to_string(),
                    }))
                    .await;
                return;
            }
            // Cap pubkey length — base64url of 32-byte Ed25519 key is exactly 43 chars;
            // allow 64 for forward-compat (e.g., post-quantum hybrid). Anything larger
            // would trigger a large allocation inside verify_register_signature().
            if reg.pubkey.len() > 64 {
                warn!("Register rejected: pubkey too long ({} bytes)", reg.pubkey.len());
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 400,
                        message: "pubkey too long".to_string(),
                    }))
                    .await;
                return;
            }
            // Cap pub_addr length — an IPv6 + port is at most ~47 chars.
            // An unbounded string here lets a single client inflate the per-entry
            // allocation to an arbitrary size (up to max_devices × whatever).
            if reg.pub_addr.as_deref().is_some_and(|a| a.len() > 128) {
                warn!("Register rejected: pub_addr too long");
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 400,
                        message: "pub_addr too long".to_string(),
                    }))
                    .await;
                return;
            }
            // Cap signature length — base64url of 64-byte Ed25519 signature is exactly
            // 86 chars; allow 128 for forward-compat. Oversized signature strings would
            // trigger large allocations in verify_register_signature() before failing.
            if reg.signature.as_deref().is_some_and(|s| s.len() > 128) {
                warn!("Register rejected: signature too long");
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 400,
                        message: "signature too long".to_string(),
                    }))
                    .await;
                return;
            }
            // Verify Ed25519 ownership proof when signature is present.
            // This prevents an attacker from squatting another device's ID on
            // the signal server. Absent signatures are accepted with a warning
            // for backward compatibility; a future version will require them.
            match miru_transport::signaling::verify_register_signature(&reg) {
                Ok(true) => {} // verified — device owns the claimed pubkey
                Ok(false) => {
                    // Legacy client with no signature. Accept but log so operators
                    // can track rollout progress.
                    warn!(
                        "Register: {} has no ownership proof (upgrade miru-host/miru-client)",
                        reg.device_id
                    );
                }
                Err(e) => {
                    warn!("Register rejected: invalid ownership proof for {}: {}", reg.device_id, e);
                    let _ = tx
                        .send(Msg::Error(miru_common::message::ErrorMsg {
                            code: 403,
                            message: "invalid registration signature".to_string(),
                        }))
                        .await;
                    return;
                }
            }
            // If there's an existing identity-locked entry for this device_id,
            // require a valid signature for the same pubkey before allowing
            // overwrite. Without this, an unsigned client could hijack a
            // signed device's entry and intercept subsequent connection offers.
            if let Some(existing) = state.registry.get(&reg.device_id) {
                if let Some(ref locked_pk) = existing.locked_pubkey {
                    // Existing entry is identity-locked — new registration must
                    // present a valid signature for the same pubkey.
                    let valid_same_pk = reg.signature.is_some()
                        && !reg.pubkey.is_empty()
                        && reg.pubkey == *locked_pk;
                    if !valid_same_pk {
                        warn!(
                            "Register rejected: {} is identity-locked but new registration lacks \
                            matching signed pubkey",
                            reg.device_id
                        );
                        let _ = tx
                            .send(Msg::Error(miru_common::message::ErrorMsg {
                                code: 403,
                                message: "device_id is identity-locked; ownership proof required".to_string(),
                            }))
                            .await;
                        return;
                    }
                }
            }

            // Reject if we've hit the registry cap (DoS prevention).
            if state.registry.len() >= state.max_devices {
                warn!(
                    "Registry full ({}/{}) — rejecting {}",
                    state.registry.len(),
                    state.max_devices,
                    reg.device_id
                );
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 503,
                        message: "server at capacity".to_string(),
                    }))
                    .await;
                return;
            }
            info!(
                "Register: {} pub={}",
                reg.device_id,
                reg.pub_addr.as_deref().unwrap_or("none")
            );
            *my_id = Some(reg.device_id.clone());
            let locked_pubkey = if reg.signature.is_some() && !reg.pubkey.is_empty() {
                Some(reg.pubkey.clone())
            } else {
                None
            };
            state.registry.insert(
                reg.device_id.clone(),
                DeviceEntry {
                    tx: tx.clone(),
                    pub_addr: reg.pub_addr,
                    pub_port: reg.pub_port,
                    locked_pubkey,
                },
            );
            let _ = tx
                .send(Msg::RegisterAck(RegisterAck {
                    device_id: reg.device_id,
                    relay_addr: Some(format!("{}:{}", state.public_host, state.relay_port)),
                }))
                .await;
        }
        Msg::Connect(req) => {
            let target = req.target_id.clone();
            if target.len() > 256 {
                warn!("Connect rejected: target_id too long ({} bytes)", target.len());
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 400,
                        message: "target_id too long".to_string(),
                    }))
                    .await;
                return;
            }
            if !state.check_connect_rate(peer_ip) {
                warn!("Rate limit exceeded for {} — dropping Connect to {}", peer_ip, target);
                let _ = tx
                    .send(Msg::Error(miru_common::message::ErrorMsg {
                        code: 429,
                        message: "too many requests — retry in 60s".to_string(),
                    }))
                    .await;
                return;
            }
            info!("Connect request {} → {}", peer_ip, target);

            match state.registry.get(&target) {
                Some(entry) => {
                    // Create relay session token — 128 bits of OS-random entropy
                    // encoded as lowercase hex (32 chars). UUID v4 only provides
                    // 122 random bits; a raw random fill is strictly stronger.
                    let raw: [u8; 16] = rand::thread_rng().gen();
                    let token = raw.iter().fold(String::with_capacity(32), |mut s, b| {
                        use std::fmt::Write;
                        let _ = write!(s, "{b:02x}");
                        s
                    });
                    state.relay_sessions.insert(
                        token.clone(),
                        RelaySlot {
                            host: None,
                            viewer: None,
                        },
                    );

                    // Reap the slot if neither peer ever claims it — otherwise
                    // every unanswered Connect leaks an entry forever.
                    {
                        let sessions = state.relay_sessions.clone();
                        let token = token.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(UNCLAIMED_SLOT_TTL).await;
                            // remove_if is atomic: no TOCTOU between the guard check and removal.
                            if sessions
                                .remove_if(&token, |_, s| s.host.is_none() && s.viewer.is_none())
                                .is_some()
                            {
                                warn!("Relay slot {} expired unclaimed", &token[..8]);
                            }
                        });
                    }

                    // Notify host
                    let _ = entry
                        .tx
                        .send(Msg::Relay(RelayOffer {
                            relay_addr: state.public_host.clone(),
                            relay_port: state.relay_port,
                            token: token.clone(),
                        }))
                        .await;

                    // Respond to viewer — include host's public address if known.
                    // relay: false tells the viewer it MAY attempt QUIC P2P directly;
                    // relay: true means the host has no known public address and the
                    // viewer must use the relay. The viewer always receives a relay
                    // token too, so it can fall back regardless.
                    let (host_pub_addr, host_pub_port) = {
                        let entry_ref = state.registry.get(&req.target_id);
                        entry_ref
                            .as_ref()
                            .map(|e| (e.pub_addr.clone(), e.pub_port))
                            .unwrap_or((None, None))
                    };
                    let must_relay = host_pub_addr.is_none();
                    let _ = tx
                        .send(Msg::ConnectAck(ConnectAck {
                            target_id: target,
                            host_addr: host_pub_addr,
                            host_port: host_pub_port,
                            relay: must_relay,
                        }))
                        .await;

                    // Viewer also needs the relay token
                    let _ = tx
                        .send(Msg::Relay(RelayOffer {
                            relay_addr: state.public_host.clone(),
                            relay_port: state.relay_port,
                            token,
                        }))
                        .await;
                }
                None => {
                    warn!("Device {} not found", target);
                    let _ = tx
                        .send(Msg::Error(miru_common::message::ErrorMsg {
                            code: 404,
                            message: format!("Device '{target}' not found or offline"),
                        }))
                        .await;
                }
            }
        }
        other => warn!("Unexpected rendezvous msg: {other:?}"),
    }
}

// ─── Relay ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RelayQuery {
    token: String,
    role: Option<String>, // "host" | "viewer"
}

async fn relay_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<RelayQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |sock| relay_session(sock, q.token, q.role, state))
}

async fn relay_session(sock: WebSocket, token: String, role: Option<String>, state: AppState) {
    // A valid token is exactly a 32-char UUID simple hex string.
    // Reject malformed tokens immediately to avoid unnecessary DashMap lookups.
    if token.len() != 32 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        warn!("Relay: rejecting malformed token (len={}, valid_hex={})", token.len(), token.chars().all(|c| c.is_ascii_hexdigit()));
        return;
    }

    let is_host = role.as_deref() == Some("host");
    info!(
        "Relay join: token={}... role={}",
        &token[..8.min(token.len())],
        role.as_deref().unwrap_or("?")
    );

    // Reject ONLY when the token is unknown AND we're at capacity.
    // Peers joining a pre-created slot must be allowed through even when at capacity —
    // their slot was counted when it was created, so this is not a new allocation.
    let slot_known = state.relay_sessions.contains_key(&token);
    if !slot_known && state.relay_sessions.len() >= state.max_relay_sessions {
        warn!(
            "Relay at capacity ({}) — dropping unknown token",
            state.max_relay_sessions
        );
        return;
    }

    // Channel for receiving forwarded bytes from the other peer
    let (fwd_tx, mut fwd_rx) = mpsc::channel::<Vec<u8>>(256);

    // Register this side in the relay slot.
    // Only pre-registered tokens (created by the rendezvous server) are accepted;
    // unknown tokens are rejected to prevent resource exhaustion by unauthenticated peers.
    let peer_tx = {
        let mut slot = match state.relay_sessions.get_mut(&token) {
            Some(s) => s,
            None => {
                warn!(
                    "Relay token {}… unknown — rejecting",
                    &token[..8.min(token.len())]
                );
                return;
            }
        };
        if is_host {
            slot.host = Some(fwd_tx.clone());
            slot.viewer.clone() // peer is viewer
        } else {
            slot.viewer = Some(fwd_tx.clone());
            slot.host.clone() // peer is host
        }
    };

    let (mut ws_tx, mut ws_rx) = sock.split();
    use futures_util::{SinkExt, StreamExt};

    // If peer not yet connected — wait up to 30s.
    // Also watch ws_rx so we detect early disconnect of the waiting peer.
    let peer_tx = match peer_tx {
        Some(tx) => tx,
        None => {
            let deadline = tokio::time::sleep(Duration::from_secs(30));
            tokio::pin!(deadline);
            // Reuse a single interval instead of constructing a fresh `sleep`
            // every loop iteration — avoids per-tick allocation churn when
            // many connections are simultaneously waiting for their peer.
            let mut poll = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                    // Deadline fired — give up and clean up the slot.
                    _ = &mut deadline => {
                        warn!("Relay timeout waiting for peer");
                        state.relay_sessions.remove(&token);
                        return;
                    }
                    // Waiting peer disconnected early — no point keeping the slot.
                    msg = ws_rx.next() => {
                        if msg.is_none() || matches!(msg, Some(Err(_))) {
                            state.relay_sessions.remove(&token);
                            return;
                        }
                        // Ignore any data frames during the wait phase
                    }
                    // Poll check every 100ms
                    _ = poll.tick() => {
                        let slot = state.relay_sessions.get(&token);
                        let found = if is_host {
                            slot.and_then(|s| s.viewer.clone())
                        } else {
                            slot.and_then(|s| s.host.clone())
                        };
                        if let Some(tx) = found {
                            break tx;
                        }
                    }
                }
            }
        }
    };

    info!("Relay: both peers connected, forwarding");

    // Bidirectional forward
    loop {
        tokio::select! {
            // WebSocket → peer channel
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if data.len() > MAX_RELAY_MSG_BYTES {
                            warn!(
                                "Relay: dropping oversized frame ({} bytes > {} limit)",
                                data.len(), MAX_RELAY_MSG_BYTES
                            );
                            break;
                        }
                        match tokio::time::timeout(SEND_TIMEOUT, peer_tx.send(data)).await {
                            Ok(Ok(())) => {}
                            _ => break,
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            // Peer channel → WebSocket
            data = fwd_rx.recv() => {
                match data {
                    Some(d) => {
                        match tokio::time::timeout(SEND_TIMEOUT, ws_tx.send(Message::Binary(d))).await {
                            Ok(Ok(())) => {}
                            _ => break,
                        }
                    }
                    None => break,
                }
            }
        }
    }

    // Cleanup
    state.relay_sessions.remove(&token);
    info!("Relay session {} ended", &token[..8.min(token.len())]);
}

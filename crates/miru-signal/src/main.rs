//! Miru Signal Server — rendezvous (hbbs) + relay (hbbr) in one binary.
//!
//! Ports:
//!   21115/tcp — rendezvous WebSocket (/ws)
//!   21117/tcp — relay WebSocket (/relay?token=XXX)

use anyhow::Result;
use axum::{
    extract::{
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
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

// ─── State ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    registry: Arc<DashMap<String, DeviceEntry>>,
    relay_sessions: Arc<DashMap<String, RelaySlot>>,
}

struct DeviceEntry {
    tx: mpsc::Sender<Msg>,
}

/// How long an issued relay token may sit completely unclaimed before the
/// slot is reaped. Generous compared to the 30s in-handler peer wait.
const UNCLAIMED_SLOT_TTL: Duration = Duration::from_secs(60);

/// Two-slot relay session — waits for both host and viewer to connect.
struct RelaySlot {
    /// Raw byte sender for each side.
    host: Option<mpsc::Sender<Vec<u8>>>,
    viewer: Option<mpsc::Sender<Vec<u8>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            registry: Arc::new(DashMap::new()),
            relay_sessions: Arc::new(DashMap::new()),
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

    let state = AppState::new();

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
    let relay_port: u16 = std::env::var("MIRU_RELAY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(21117);
    let rdv_addr: SocketAddr = format!("0.0.0.0:{rdv_port}").parse()?;
    let relay_addr: SocketAddr = format!("0.0.0.0:{relay_port}").parse()?;

    info!("Rendezvous: {}", rdv_addr);
    info!("Relay:      {}", relay_addr);

    tokio::try_join!(run_server(rdv, rdv_addr), run_server(relay, relay_addr),)?;
    Ok(())
}

async fn run_server(app: Router, addr: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await.map_err(Into::into)
}

// ─── Rendezvous ───────────────────────────────────────────────────────────────

async fn rendezvous_handler(ws: WebSocketUpgrade, State(s): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |sock| rendezvous_session(sock, s))
}

async fn rendezvous_session(mut sock: WebSocket, state: AppState) {
    let (tx, mut rx) = mpsc::channel::<Msg>(32);
    let mut my_id: Option<String> = None;

    loop {
        tokio::select! {
            // Forward outbound messages to WebSocket
            Some(msg) = rx.recv() => {
                let Ok(json) = serde_json::to_string(&msg) else { continue };
                if sock.send(Message::Text(json)).await.is_err() { break }
            }
            // Handle inbound WebSocket messages
            incoming = sock.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Msg>(&text) {
                            Ok(m) => process_rdv_msg(&state, m, &tx, &mut my_id).await,
                            Err(e) => warn!("parse error: {}", e),
                        }
                    }
                    Some(Ok(Message::Ping(d))) => { let _ = sock.send(Message::Pong(d)).await; }
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
) {
    match msg {
        Msg::Register(reg) => {
            info!("Register: {}", reg.device_id);
            *my_id = Some(reg.device_id.clone());
            state
                .registry
                .insert(reg.device_id.clone(), DeviceEntry { tx: tx.clone() });
            let _ = tx
                .send(Msg::RegisterAck(RegisterAck {
                    device_id: reg.device_id,
                    relay_addr: Some(format!(
                        "{}:21117",
                        std::env::var("MIRU_PUBLIC_HOST")
                            .unwrap_or_else(|_| "localhost".to_string())
                    )),
                }))
                .await;
        }
        Msg::Connect(req) => {
            let target = req.target_id.clone();
            info!("Connect request → {}", target);

            match state.registry.get(&target) {
                Some(entry) => {
                    // Create relay session token
                    let token = Uuid::new_v4().simple().to_string();
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
                            let unclaimed = sessions
                                .get(&token)
                                .map(|s| s.host.is_none() && s.viewer.is_none())
                                .unwrap_or(false);
                            if unclaimed {
                                sessions.remove(&token);
                                warn!("Relay slot {} expired unclaimed", &token[..8]);
                            }
                        });
                    }

                    // Notify host
                    let public_host = std::env::var("MIRU_PUBLIC_HOST")
                        .unwrap_or_else(|_| "localhost".to_string());
                    let _ = entry
                        .tx
                        .send(Msg::Relay(RelayOffer {
                            relay_addr: public_host.clone(),
                            relay_port: 21117,
                            token: token.clone(),
                        }))
                        .await;

                    // Respond to viewer
                    let _ = tx
                        .send(Msg::ConnectAck(ConnectAck {
                            target_id: target,
                            host_addr: None,
                            host_port: None,
                            relay: true,
                        }))
                        .await;

                    // Viewer also needs the relay token
                    let _ = tx
                        .send(Msg::Relay(RelayOffer {
                            relay_addr: public_host,
                            relay_port: 21117,
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
        other => warn!("Unexpected rendezvous msg: {:?}", other),
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
    let is_host = role.as_deref() == Some("host");
    info!(
        "Relay join: token={}... role={}",
        &token[..8.min(token.len())],
        role.as_deref().unwrap_or("?")
    );

    // Channel for receiving forwarded bytes from the other peer
    let (fwd_tx, mut fwd_rx) = mpsc::channel::<Vec<u8>>(256);

    // Register this side in the relay slot
    let peer_tx = {
        let mut slot = state
            .relay_sessions
            .entry(token.clone())
            .or_insert(RelaySlot {
                host: None,
                viewer: None,
            });
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

    // If peer not yet connected — wait up to 30s
    let peer_tx = match peer_tx {
        Some(tx) => tx,
        None => {
            // Poll until other side connects
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    warn!("Relay timeout waiting for peer");
                    // Drop the slot so the abandoned token doesn't sit in the
                    // map forever (the late peer re-creates and times out too).
                    state.relay_sessions.remove(&token);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
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
    };

    info!("Relay: both peers connected, forwarding");

    // Bidirectional forward
    loop {
        tokio::select! {
            // WebSocket → peer channel
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if peer_tx.send(data).await.is_err() { break }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            // Peer channel → WebSocket
            data = fwd_rx.recv() => {
                match data {
                    Some(d) => {
                        if ws_tx.send(Message::Binary(d)).await.is_err() { break }
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

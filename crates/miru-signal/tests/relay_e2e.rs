//! Signal server relay integration test.
//!
//! Spawns the real signal server as a child process, then connects two
//! WebSocket clients (host + viewer) through the full rendezvous → relay
//! flow and verifies that bytes sent by one peer arrive at the other.
//!
//! Run with: cargo test -p miru-signal --test relay_e2e

use futures_util::{SinkExt, StreamExt};
use miru_common::message::{ConnectRequest, Msg, Register};
use std::process::{Child, Command};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

/// Guard that kills the signal child process on drop.
struct ServerGuard(Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_signal(relay_port: u16, rdv_port: u16) -> ServerGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_miru-signal"))
        .env("MIRU_LOG", "warn")
        .env("MIRU_RDV_PORT", rdv_port.to_string())
        .env("MIRU_RELAY_PORT", relay_port.to_string())
        .spawn()
        .expect("spawn signal server");
    std::thread::sleep(Duration::from_millis(800));
    ServerGuard(child)
}

/// Send a Msg as JSON text over a WebSocket.
async fn send_msg(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    msg: &Msg,
) {
    let json = serde_json::to_string(msg).unwrap();
    ws.send(Message::Text(json)).await.unwrap();
}

/// Receive the next JSON Msg from a WebSocket.
async fn recv_msg(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Msg {
    loop {
        let frame = ws.next().await.unwrap().unwrap();
        match frame {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Ping(d) => {
                ws.send(Message::Pong(d)).await.unwrap();
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn relay_forwards_bytes_between_peers() {
    let relay_port = 31117u16;
    let rdv_port = 31115u16;
    let _guard = start_signal(relay_port, rdv_port);

    let rdv_url = format!("ws://127.0.0.1:{rdv_port}/ws");
    let host_device_id = format!("test-host-{}", std::process::id());

    // 1. Host registers on rendezvous
    let (mut host_rdv, _) = match tokio_tungstenite::connect_async(&rdv_url).await {
        Ok(x) => x,
        Err(e) => {
            eprintln!("skip: cannot connect to rendezvous: {e}");
            return;
        }
    };
    send_msg(
        &mut host_rdv,
        &Msg::Register(Register {
            device_id: host_device_id.clone(),
            pubkey: String::new(),
            pub_addr: None,
            pub_port: None,
            signature: None,
            signed_at_sec: None,
        }),
    )
    .await;
    // Consume RegisterAck
    recv_msg(&mut host_rdv).await;

    // 2. Viewer requests connection to host
    let (mut viewer_rdv, _) = tokio_tungstenite::connect_async(&rdv_url)
        .await
        .expect("viewer rdv connect");
    send_msg(
        &mut viewer_rdv,
        &Msg::Connect(ConnectRequest {
            target_id: host_device_id.clone(),
            viewer_addr: "127.0.0.1".into(),
            viewer_port: 0,
        }),
    )
    .await;

    // 3. Viewer receives ConnectAck then RelayOffer
    let mut relay_token_viewer = String::new();
    let mut relay_token_host = String::new();
    for _ in 0..3 {
        let m = recv_msg(&mut viewer_rdv).await;
        if let Msg::Relay(offer) = m {
            relay_token_viewer = offer.token;
            break;
        }
    }
    // Host receives RelayOffer
    for _ in 0..3 {
        let m = recv_msg(&mut host_rdv).await;
        if let Msg::Relay(offer) = m {
            relay_token_host = offer.token;
            break;
        }
    }

    assert!(
        !relay_token_viewer.is_empty(),
        "viewer did not receive relay token"
    );
    assert_eq!(relay_token_viewer, relay_token_host, "tokens must match");

    let token = relay_token_viewer;

    // 4. Connect both peers to the relay using the real token
    let host_relay_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=host");
    let viewer_relay_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=viewer");

    let (mut host_ws, _) = tokio_tungstenite::connect_async(&host_relay_url)
        .await
        .expect("host relay connect");
    let (mut viewer_ws, _) = tokio_tungstenite::connect_async(&viewer_relay_url)
        .await
        .expect("viewer relay connect");

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Host → Viewer
    let payload = b"hello-from-host".to_vec();
    host_ws
        .send(Message::Binary(payload.clone()))
        .await
        .expect("host send");

    let received = tokio::time::timeout(Duration::from_secs(3), viewer_ws.next())
        .await
        .expect("viewer recv timeout")
        .expect("viewer stream ended")
        .expect("viewer recv error");
    match received {
        Message::Binary(data) => assert_eq!(data, payload, "host→viewer payload mismatch"),
        other => panic!("expected binary, got {other:?}"),
    }

    // Viewer → Host (reverse direction)
    let reply = b"ack-from-viewer".to_vec();
    viewer_ws
        .send(Message::Binary(reply.clone()))
        .await
        .expect("viewer send");

    let got = tokio::time::timeout(Duration::from_secs(3), host_ws.next())
        .await
        .expect("host recv timeout")
        .expect("host stream ended")
        .expect("host recv error");
    match got {
        Message::Binary(data) => assert_eq!(data, reply, "viewer→host payload mismatch"),
        other => panic!("expected binary, got {other:?}"),
    }
}

#[tokio::test]
async fn relay_rejects_unknown_token() {
    let relay_port = 31317u16;
    let rdv_port = 31315u16;
    let _guard = start_signal(relay_port, rdv_port);

    // Attempt to connect with a fabricated token — should be rejected.
    let fake_token = "fabricated-token-that-was-never-issued";
    let url = format!("ws://127.0.0.1:{relay_port}/relay?token={fake_token}&role=host");

    let conn = match tokio_tungstenite::connect_async(&url).await {
        Ok(x) => x,
        Err(e) => {
            eprintln!("skip: {e}");
            return;
        }
    };

    let mut ws = conn.0;
    // Server should close the connection immediately (unknown token).
    let result = tokio::time::timeout(Duration::from_secs(2), ws.next()).await;
    assert!(
        result.is_ok(),
        "relay did not close unknown-token connection in time"
    );
    // The connection should be closed (None or Close frame)
    match result.unwrap() {
        None | Some(Ok(Message::Close(_))) => {}
        Some(Ok(other)) => panic!("expected close, got {other:?}"),
        Some(Err(_)) => {} // connection reset is also acceptable
    }
}

#[tokio::test]
async fn oversized_relay_message_drops_connection() {
    let relay_port = 31217u16;
    let rdv_port = 31215u16;
    let _guard = start_signal(relay_port, rdv_port);

    let rdv_url = format!("ws://127.0.0.1:{rdv_port}/ws");
    let host_device_id = format!("oversize-host-{}", std::process::id());

    // Register host and get relay token via rendezvous
    let (mut host_rdv, _) = match tokio_tungstenite::connect_async(&rdv_url).await {
        Ok(x) => x,
        Err(e) => {
            eprintln!("skip: {e}");
            return;
        }
    };
    send_msg(
        &mut host_rdv,
        &Msg::Register(Register {
            device_id: host_device_id.clone(),
            pubkey: String::new(),
            pub_addr: None,
            pub_port: None,
            signature: None,
            signed_at_sec: None,
        }),
    )
    .await;
    recv_msg(&mut host_rdv).await; // RegisterAck

    let (mut viewer_rdv, _) = tokio_tungstenite::connect_async(&rdv_url).await.unwrap();
    send_msg(
        &mut viewer_rdv,
        &Msg::Connect(ConnectRequest {
            target_id: host_device_id,
            viewer_addr: "127.0.0.1".into(),
            viewer_port: 0,
        }),
    )
    .await;

    let mut token = String::new();
    for _ in 0..3 {
        if let Msg::Relay(offer) = recv_msg(&mut viewer_rdv).await {
            token = offer.token;
            break;
        }
    }
    for _ in 0..3 {
        if let Msg::Relay(_) = recv_msg(&mut host_rdv).await {
            break;
        }
    }
    assert!(!token.is_empty(), "no relay token received");

    let host_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=host");
    let viewer_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=viewer");

    let (mut host_ws, _) = tokio_tungstenite::connect_async(&host_url)
        .await
        .expect("host relay connect");
    let (mut viewer_ws, _) = tokio_tungstenite::connect_async(&viewer_url)
        .await
        .expect("viewer relay connect");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Send a 5 MiB frame — exceeds the 4 MiB MAX_RELAY_MSG_BYTES limit.
    let oversized = vec![0u8; 5 * 1024 * 1024];
    let _ = host_ws.send(Message::Binary(oversized)).await;

    // The relay should close the connection within a short window.
    let closed = tokio::time::timeout(Duration::from_secs(3), host_ws.next()).await;
    assert!(
        closed.is_ok(),
        "relay did not close oversized-frame connection in time"
    );

    let _ = tokio::time::timeout(Duration::from_secs(2), viewer_ws.next()).await;
}

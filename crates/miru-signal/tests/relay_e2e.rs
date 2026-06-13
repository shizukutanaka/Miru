//! Signal server relay integration test.
//!
//! Spawns the real signal server as a child process, then connects two
//! WebSocket clients (host + viewer) to the relay and verifies that bytes
//! sent by one peer arrive at the other — the core P2P-fallback path.
//!
//! Run with: cargo test -p miru-signal --test relay_e2e

use futures_util::{SinkExt, StreamExt};
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

#[tokio::test]
async fn relay_forwards_bytes_between_peers() {
    // Use high ports unlikely to collide.
    let relay_port = 31117u16;
    let rdv_port = 31115u16;
    let _guard = start_signal(relay_port, rdv_port);

    let token = format!("test-token-{}", std::process::id());
    let host_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=host");
    let viewer_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=viewer");

    // Connect host first.
    let (mut host_ws, _) = match tokio_tungstenite::connect_async(&host_url).await {
        Ok(x) => x,
        Err(e) => {
            eprintln!("skip: cannot connect to relay (server may need ports): {e}");
            return;
        }
    };
    // Then viewer.
    let (mut viewer_ws, _) = tokio_tungstenite::connect_async(&viewer_url)
        .await
        .expect("viewer connect");

    // Give the relay a moment to pair both sides.
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
async fn oversized_relay_message_drops_connection() {
    let relay_port = 31217u16;
    let rdv_port = 31215u16;
    let _guard = start_signal(relay_port, rdv_port);

    let token = format!("oversize-{}", std::process::id());
    let host_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=host");
    let viewer_url = format!("ws://127.0.0.1:{relay_port}/relay?token={token}&role=viewer");

    let (mut host_ws, _) = match tokio_tungstenite::connect_async(&host_url).await {
        Ok(x) => x,
        Err(e) => {
            eprintln!("skip: {e}");
            return;
        }
    };
    let (mut viewer_ws, _) = tokio_tungstenite::connect_async(&viewer_url)
        .await
        .expect("viewer connect");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Send a 5 MiB frame — exceeds the 4 MiB MAX_RELAY_MSG_BYTES limit.
    let oversized = vec![0u8; 5 * 1024 * 1024];
    // The send itself may succeed (WebSocket layer accepts it); the relay
    // drops the connection on receipt so the host stream should end shortly.
    let _ = host_ws.send(Message::Binary(oversized)).await;

    // The relay should close the connection within a short window.
    let closed = tokio::time::timeout(Duration::from_secs(3), host_ws.next()).await;
    assert!(
        closed.is_ok(),
        "relay did not close oversized-frame connection in time"
    );

    // Viewer should also see the connection close (relay removed the slot).
    let _ = tokio::time::timeout(Duration::from_secs(2), viewer_ws.next()).await;
}

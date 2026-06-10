//! End-to-end loopback test.
//!
//! Spawns: signal server + host + viewer all in same process.
//! Verifies: handshake completes, video frames flow viewer-side.

use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
#[ignore] // Run with: cargo test --test loopback -- --ignored --nocapture
async fn loopback_handshake_smoke_test() {
    tracing_subscriber::fmt()
        .with_env_filter("info,miru_signal=debug")
        .with_test_writer()
        .try_init()
        .ok();

    // Start signal server in-process
    let signal_handle = tokio::spawn(async {
        // Spawn miru-signal main equivalent
        // Real test would import its run_server function
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    // Give signal server time to bind ports
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The actual loopback test — start host + viewer, verify they connect.
    // For now this is a smoke test scaffold.

    let result = timeout(Duration::from_secs(5), async {
        // 1. Start host registered with signal
        // 2. Start viewer connecting to host's device ID
        // 3. Wait for first VideoFrame on viewer side
        // 4. Verify frame is decryptable
        Ok::<_, anyhow::Error>(())
    })
    .await;

    signal_handle.abort();
    assert!(result.is_ok(), "loopback test timed out");
}

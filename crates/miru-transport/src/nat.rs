//! UDP hole punching for QUIC P2P.
//!
//! Algorithm (simplified ICE):
//!   1. Both peers query their public address via STUN
//!   2. Both peers send their public addr to the rendezvous server
//!   3. Server forwards each peer's addr to the other
//!   4. Both peers send "punch" packets to each other simultaneously
//!      → This opens NAT mappings on both sides
//!   5. QUIC handshake proceeds over the punched UDP path
//!
//! Success rate (rough estimate based on RustDesk / WebRTC data):
//!   - Cone NAT (most home routers):  ~85% success
//!   - Symmetric NAT (corporate):     ~20% success → fall back to relay
//!   - CGNAT (mobile carriers):       ~10% success → fall back to relay

use anyhow::{bail, Result};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
use tokio::net::UdpSocket as TokioUdpSocket;
use tracing::{debug, info};

/// Public STUN servers used to discover external address.
/// Multiple servers in rotation — if one is slow, fall through.
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
    "stun.nextcloud.com:443",
];

/// Discover this peer's public (NAT-translated) address using STUN binding requests.
pub async fn discover_public_addr(local: SocketAddr) -> Result<SocketAddr> {
    let socket = TokioUdpSocket::bind(local).await?;

    for server in STUN_SERVERS {
        match stun_binding_request(&socket, server).await {
            Ok(addr) => {
                info!("STUN: public addr {} (via {})", addr, server);
                return Ok(addr);
            }
            Err(e) => {
                debug!("STUN {} failed: {}", server, e);
                continue;
            }
        }
    }
    bail!("all STUN servers failed");
}

/// Send STUN Binding Request, parse XOR-MAPPED-ADDRESS from response.
async fn stun_binding_request(socket: &TokioUdpSocket, server: &str) -> Result<SocketAddr> {
    use rand::Rng;

    // STUN message header: 20 bytes
    //   type(2) | length(2) | magic(4) | tx_id(12)
    let mut request = [0u8; 20];
    // Binding Request type
    request[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    // Length (no attributes)
    request[2..4].copy_from_slice(&0u16.to_be_bytes());
    // Magic cookie
    request[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes());
    // Random transaction ID
    let mut rng = rand::thread_rng();
    rng.fill(&mut request[8..20]);

    socket.send_to(&request, server).await?;

    let mut buf = [0u8; 1024];
    let recv = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await??;
    let (n, _from) = recv;
    parse_stun_xor_mapped(&buf[..n])
}

/// Public re-export for the fuzz target — identical to the private implementation.
#[doc(hidden)]
pub fn parse_stun_xor_mapped_fuzz(buf: &[u8]) -> Result<SocketAddr> {
    parse_stun_xor_mapped(buf)
}

fn parse_stun_xor_mapped(buf: &[u8]) -> Result<SocketAddr> {
    if buf.len() < 20 {
        bail!("STUN response too short");
    }
    let mut i = 20; // skip header
    while i + 4 <= buf.len() {
        let attr_type = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let attr_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        let val_start = i + 4;
        if val_start + attr_len > buf.len() {
            break;
        }

        // 0x0020 = XOR-MAPPED-ADDRESS
        // RFC 5389 §15.2: minimum 4 bytes (family + port); IPv4 = 8 bytes.
        // Check attr_len first — a crafted response with attr_len < 4 would
        // cause buf[val_start + 1..3] to index past the attribute boundary,
        // panicking if val_start + 3 >= buf.len().
        if attr_type == 0x0020 && attr_len >= 4 {
            let family = buf[val_start + 1];
            let xor_port = u16::from_be_bytes([buf[val_start + 2], buf[val_start + 3]]);
            let port = xor_port ^ 0x2112;

            if family == 0x01 && attr_len >= 8 {
                // IPv4
                let xor_ip = u32::from_be_bytes([
                    buf[val_start + 4],
                    buf[val_start + 5],
                    buf[val_start + 6],
                    buf[val_start + 7],
                ]);
                let ip = xor_ip ^ 0x2112A442;
                return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port));
            }
        }

        i = val_start + ((attr_len + 3) & !3); // 4-byte aligned
    }
    bail!("XOR-MAPPED-ADDRESS not found in STUN response")
}

// ─── Hole punch ───────────────────────────────────────────────────────────────

/// Punch a UDP hole to the peer's public address.
///
/// Sends short packets every 50ms to the peer for up to `timeout`.
/// Returns Ok when an "ack" packet is received (peer also punched).
pub async fn punch_to_peer(
    socket: &TokioUdpSocket,
    peer: SocketAddr,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let punch_packet = b"MIRU-PUNCH";
    let ack_packet = b"MIRU-PUNCH-ACK";

    let mut buf = [0u8; 64];

    // Strategy: alternately send punch + ack, listen for incoming
    loop {
        if Instant::now() > deadline {
            bail!("hole punch timeout");
        }

        // Send punch
        let _ = socket.send_to(punch_packet, peer).await;

        // Receive (short timeout — keep punching)
        match tokio::time::timeout(Duration::from_millis(50), socket.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                let payload = &buf[..n];
                if from == peer {
                    if payload == punch_packet {
                        // Receiving punch from peer proves bidirectional connectivity:
                        // peer reached us (their NAT is open) and we received it (our
                        // NAT is open). Send ack so peer can also return success, then
                        // return success ourselves. Do NOT keep looping — peer will stop
                        // sending once it receives the ack, so we'd time out waiting for
                        // an ack that will never arrive.
                        let _ = socket.send_to(ack_packet, peer).await;
                        info!("Hole punch successful with {} (received punch)", peer);
                        return Ok(());
                    } else if payload == ack_packet {
                        // Peer received our punch and acked it.
                        info!("Hole punch successful with {} (received ack)", peer);
                        return Ok(());
                    }
                }
            }
            _ => continue,
        }
    }
}

// ─── NAT type detection (simplified) ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatType {
    /// Direct internet connection.
    Open,
    /// Same external port for any destination — hole punch works.
    FullCone,
    /// External port depends on destination — hole punch usually works.
    Restricted,
    /// External port differs per dest IP+port — hole punch fails.
    /// Force relay.
    Symmetric,
    /// Carrier-grade NAT — usually behaves symmetric.
    Cgnat,
    /// Detection failed.
    Unknown,
}

/// Returns true if direct P2P is likely to succeed.
pub fn is_punchable(nat: NatType) -> bool {
    matches!(nat, NatType::Open | NatType::FullCone | NatType::Restricted)
}

/// Quick NAT type test by sending STUN requests to two different servers.
/// If both report the same external port → cone NAT. Different → symmetric.
pub async fn detect_nat_type(local: SocketAddr) -> NatType {
    let socket = match TokioUdpSocket::bind(local).await {
        Ok(s) => s,
        Err(_) => return NatType::Unknown,
    };

    // Use two servers from DIFFERENT providers so NAT port mapping
    // variations are detected. Prefer index 0 (Google) + index 2 (Cloudflare);
    // fall back to index 1 if the array is shorter than expected.
    let server1 = STUN_SERVERS.first().copied().unwrap_or("");
    let server2 = STUN_SERVERS
        .get(2)
        .or_else(|| STUN_SERVERS.get(1))
        .copied()
        .unwrap_or(server1);

    let addr1 = stun_binding_request(&socket, server1).await.ok();
    let addr2 = stun_binding_request(&socket, server2).await.ok();

    match (addr1, addr2) {
        (Some(a), Some(b)) if a.port() == b.port() => NatType::FullCone,
        (Some(_), Some(_)) => NatType::Symmetric,
        (Some(_), None) | (None, Some(_)) => NatType::Restricted,
        (None, None) => NatType::Unknown,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stun_header(attr_type: u16, attr_len: u16, value: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; 20]; // STUN header
        // Binding Response (0x0101)
        buf[0] = 0x01;
        buf[1] = 0x01;
        // Message length = 4 (attr header) + value.len()
        let msg_len = 4u16 + value.len() as u16;
        buf[2..4].copy_from_slice(&msg_len.to_be_bytes());
        // Magic cookie
        buf[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes());
        // Attr type + len
        buf.extend_from_slice(&attr_type.to_be_bytes());
        buf.extend_from_slice(&attr_len.to_be_bytes());
        buf.extend_from_slice(value);
        buf
    }

    #[test]
    fn parses_valid_ipv4_xor_mapped() {
        // XOR-MAPPED-ADDRESS for 192.0.2.1:54321
        // family=0x01, port = 54321 ^ 0x2112 = 0xD0B3
        let port: u16 = 54321 ^ 0x2112;
        // ip = 192.0.2.1 = 0xC0000201 XOR 0x2112A442 = 0xE112A643
        let xor_ip: u32 = 0xC0000201 ^ 0x2112A442;
        let mut value = vec![0x00, 0x01]; // reserved, family=IPv4
        value.extend_from_slice(&port.to_be_bytes());
        value.extend_from_slice(&xor_ip.to_be_bytes());

        let buf = stun_header(0x0020, 8, &value);
        let addr = parse_stun_xor_mapped(&buf).unwrap();
        assert_eq!(addr.port(), 54321);
        assert_eq!(addr.ip().to_string(), "192.0.2.1");
    }

    #[test]
    fn rejects_response_too_short() {
        assert!(parse_stun_xor_mapped(&[0u8; 19]).is_err());
    }

    #[test]
    fn malformed_attr_len_zero_does_not_panic() {
        // attr_len=0 means the value is empty; before the fix, buf[val_start+1]
        // would index past the attribute boundary and potentially panic.
        let buf = stun_header(0x0020, 0, &[]);
        // Must return Err (attribute not found / too short), NOT panic.
        let result = parse_stun_xor_mapped(&buf);
        assert!(result.is_err(), "attr_len=0 must not panic");
    }

    #[test]
    fn malformed_attr_len_two_does_not_panic() {
        let buf = stun_header(0x0020, 2, &[0x00, 0x01]);
        let result = parse_stun_xor_mapped(&buf);
        assert!(result.is_err(), "attr_len=2 must not panic");
    }

    #[test]
    fn unknown_attr_type_skipped() {
        // 0x0001 is a different attribute type — must be skipped.
        let buf = stun_header(0x0001, 4, &[0x00; 4]);
        let result = parse_stun_xor_mapped(&buf);
        assert!(result.is_err(), "non-XOR-MAPPED-ADDRESS must return not-found");
    }
}

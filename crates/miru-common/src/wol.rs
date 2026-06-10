//! Wake-on-LAN.
//!
//! Send a magic packet to wake a sleeping host before connecting.
//! Useful when the host PC sleeps but you want to start a session.
//!
//! Requirements on host:
//!   - WoL enabled in BIOS/UEFI
//!   - OS-level WoL enabled (Windows: NIC properties; Linux: `ethtool -s eth0 wol g`)
//!   - Connected via wired Ethernet (Wi-Fi WoL is unreliable)

use anyhow::{bail, Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// Build a magic packet for the given MAC address.
/// Format: 6 bytes of 0xFF, then the MAC repeated 16 times = 102 bytes.
pub fn build_magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0u8; 102];
    packet[..6].fill(0xFF);
    for i in 0..16 {
        let offset = 6 + i * 6;
        packet[offset..offset + 6].copy_from_slice(&mac);
    }
    packet
}

/// Parse "AA:BB:CC:DD:EE:FF" or "AA-BB-CC-DD-EE-FF" into 6 bytes.
pub fn parse_mac(s: &str) -> Result<[u8; 6]> {
    let cleaned: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() != 12 {
        bail!("MAC must be 12 hex digits, got {}", cleaned.len());
    }
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("invalid hex at byte {i}"))?;
    }
    Ok(mac)
}

/// Send the magic packet via UDP broadcast on port 9.
/// Optional `target_ip` lets you target a specific subnet's broadcast.
pub fn wake(mac: [u8; 6], target_ip: Option<Ipv4Addr>) -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0").context("bind UDP")?;
    socket.set_broadcast(true).context("set_broadcast")?;

    let packet = build_magic_packet(mac);
    let dest = target_ip.unwrap_or(Ipv4Addr::BROADCAST);
    let target = SocketAddr::new(IpAddr::V4(dest), 9);

    socket.send_to(&packet, target).context("send")?;
    // Send to port 7 too (some BIOSes only listen on one)
    let target7 = SocketAddr::new(IpAddr::V4(dest), 7);
    let _ = socket.send_to(&packet, target7);

    eprintln!(
        "[wol] {}",
        format_args!("WoL: magic packet sent to {} → {}", format_mac(&mac), dest)
    );
    Ok(())
}

pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colon_format() {
        let mac = parse_mac("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn parses_dash_format() {
        let mac = parse_mac("aa-bb-cc-dd-ee-ff").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn rejects_too_short() {
        assert!(parse_mac("AA:BB:CC").is_err());
    }

    #[test]
    fn rejects_invalid_hex() {
        assert!(parse_mac("AA:BB:CC:DD:EE:GG").is_err());
    }

    #[test]
    fn magic_packet_format() {
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34];
        let pkt = build_magic_packet(mac);
        // First 6 bytes are 0xFF
        assert_eq!(&pkt[..6], &[0xFF; 6]);
        // Then MAC × 16
        for i in 0..16 {
            assert_eq!(&pkt[6 + i * 6..6 + i * 6 + 6], &mac);
        }
    }

    #[test]
    fn format_mac_roundtrip() {
        let mac = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC];
        let s = format_mac(&mac);
        assert_eq!(parse_mac(&s).unwrap(), mac);
    }
}

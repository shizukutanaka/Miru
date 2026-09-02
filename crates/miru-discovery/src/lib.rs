//! mDNS-based local-network discovery.
//!
//! Each Miru device announces itself on the LAN via `_miru._tcp.local.`,
//! advertising:
//!   - device_id (opaque string matching miru_common::session::DeviceId)
//!   - constellation_pubkey (base64) — devices group by this
//!   - port (signaling port the device listens on)
//!   - friendly_name, form_factor, os_family

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use miru_constellation::{DeviceCapabilities, FormFactor};
use parking_lot::RwLock;
use std::{collections::HashMap, net::IpAddr, sync::Arc, time::Duration};
use tracing::{debug, info, warn};

const SERVICE_TYPE: &str = "_miru._tcp.local.";
/// Maximum number of LAN peers retained in memory.
/// Protects against a malicious device flooding mDNS with unique device IDs.
const MAX_PEERS: usize = 256;

/// What we advertise about ourselves.
#[derive(Debug, Clone)]
pub struct LocalAdvertisement {
    /// Opaque device ID string (e.g. "ABCD-1234" from miru_common::session::DeviceId).
    pub device_id: String,
    /// Base64 of the constellation pubkey (devices group by this).
    pub constellation_pubkey: String,
    pub friendly_name: String,
    pub form_factor: FormFactor,
    pub port: u16,
    pub capabilities: DeviceCapabilities,
}

/// What we learn about peers.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    pub device_id: String,
    pub constellation_pubkey: String,
    pub friendly_name: String,
    pub form_factor: FormFactor,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub last_seen: std::time::Instant,
    /// mDNS fully-qualified service name — used to match ServiceRemoved events.
    pub(crate) full_name: String,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    /// Retained for future unregister()/re-advertise.
    #[allow(dead_code)]
    instance_name: String,
    peers: Arc<RwLock<HashMap<String, DiscoveredPeer>>>,
}

impl Discovery {
    pub fn start(advert: LocalAdvertisement) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("create mDNS daemon")?;

        let txt = [
            ("device_id".to_string(), advert.device_id.clone()),
            (
                "constellation".to_string(),
                advert.constellation_pubkey.clone(),
            ),
            ("name".to_string(), advert.friendly_name.clone()),
            (
                "form".to_string(),
                match advert.form_factor {
                    FormFactor::Desktop => "desktop",
                    FormFactor::Laptop => "laptop",
                    FormFactor::Phone => "phone",
                    FormFactor::Tablet => "tablet",
                    FormFactor::Server => "server",
                    FormFactor::Embedded => "embedded",
                }
                .to_string(),
            ),
            ("os".to_string(), advert.capabilities.os_family.clone()),
        ];

        // Sanitize device_id for use as a DNS label (strip non-alphanumeric).
        let safe_id: String = advert
            .device_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(12)
            .collect::<String>()
            .to_lowercase();
        // If device_id has no ASCII-alphanumeric characters at all, every such
        // device would otherwise collapse to the same "miru-" instance name,
        // silently colliding on the LAN. Fall back to a deterministic hash of
        // the raw device_id so distinct devices stay distinguishable.
        let safe_id = if safe_id.is_empty() {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            advert.device_id.hash(&mut hasher);
            format!("{:x}", hasher.finish() & 0xFFFF_FFFF)
        } else {
            safe_id
        };
        let instance_name = format!("miru-{safe_id}");
        let hostname = format!("{instance_name}.local.");

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &hostname,
            "",
            advert.port,
            &txt[..],
        )?
        .enable_addr_auto();

        daemon.register(info).context("register service")?;
        info!("Discovery: announcing as {}", instance_name);

        let peers: Arc<RwLock<HashMap<String, DiscoveredPeer>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let receiver = daemon.browse(SERVICE_TYPE).context("start browse")?;
        let peers_clone = Arc::clone(&peers);
        let our_id = advert.device_id;

        tokio::spawn(async move {
            while let Ok(event) = receiver.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if let Some(peer) = parse_peer(&info) {
                            if peer.device_id == our_id {
                                continue;
                            }
                            let mut map = peers_clone.write();
                            if map.len() >= MAX_PEERS && !map.contains_key(&peer.device_id) {
                                warn!(
                                    "Discovery: peer limit {MAX_PEERS} reached, ignoring {}",
                                    peer.device_id
                                );
                            } else {
                                debug!(
                                    "Discovery: found {} ({})",
                                    peer.friendly_name, peer.device_id
                                );
                                map.insert(peer.device_id.clone(), peer);
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, full_name) => {
                        // Graceful departure: remove immediately so the UI reflects
                        // the change at once rather than waiting for prune_stale().
                        let mut map = peers_clone.write();
                        map.retain(|_, p| p.full_name != full_name);
                        debug!("Discovery: removed {}", full_name);
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            daemon,
            instance_name,
            peers,
        })
    }

    /// Snapshot of currently known peers, optionally filtered to one constellation.
    pub fn snapshot(&self, constellation_pubkey: Option<&str>) -> Vec<DiscoveredPeer> {
        let peers = self.peers.read();
        peers
            .values()
            .filter(|p| {
                constellation_pubkey
                    .map(|c| p.constellation_pubkey == c)
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Remove peers not seen within `older_than`.
    pub fn prune_stale(&self, older_than: Duration) {
        // Do not use checked_sub fallback — Instant::now() as fallback would
        // prune ALL peers (none can have last_seen in the future).
        let now = std::time::Instant::now();
        self.peers
            .write()
            .retain(|_, p| now.duration_since(p.last_seen) < older_than);
    }

    pub fn shutdown(self) {
        if let Err(e) = self.daemon.shutdown() {
            warn!("Discovery shutdown: {:?}", e);
        }
    }
}

fn parse_peer(info: &ServiceInfo) -> Option<DiscoveredPeer> {
    // Built via an explicit loop (rather than .collect() into a HashMap) so a
    // malformed/hostile peer advertising a duplicate TXT key doesn't silently
    // drop one of the two values with no diagnostic trail.
    let mut txt: HashMap<String, String> = HashMap::new();
    for p in info.get_properties().iter() {
        if txt
            .insert(p.key().to_string(), p.val_str().to_string())
            .is_some()
        {
            warn!(
                "Discovery: duplicate TXT key '{}' from {}",
                p.key(),
                info.get_fullname()
            );
        }
    }

    let device_id = txt.get("device_id")?.clone();
    // device_id must match "XXXX-XXXX" format (9 chars); reject oversized values.
    if device_id.is_empty() || device_id.len() > 32 {
        return None;
    }
    let constellation_pubkey = txt
        .get("constellation")
        .cloned()
        .unwrap_or_default()
        .chars()
        .take(128)
        .collect();
    let friendly_name: String = txt
        .get("name")
        .cloned()
        .unwrap_or_default()
        .chars()
        .take(64)
        .collect();
    let form_factor = match txt.get("form").map(String::as_str) {
        Some("desktop") => FormFactor::Desktop,
        Some("laptop") => FormFactor::Laptop,
        Some("phone") => FormFactor::Phone,
        Some("tablet") => FormFactor::Tablet,
        Some("server") => FormFactor::Server,
        Some("embedded") => FormFactor::Embedded,
        _ => FormFactor::Desktop,
    };

    let addresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();

    Some(DiscoveredPeer {
        device_id,
        constellation_pubkey,
        friendly_name,
        form_factor,
        addresses,
        port: info.get_port(),
        last_seen: std::time::Instant::now(),
        full_name: info.get_fullname().to_string(),
    })
}

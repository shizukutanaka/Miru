//! mDNS-based local-network discovery.
//!
//! Each Miru device announces itself on the LAN via `_miru._tcp.local.`,
//! advertising:
//!   - device_id (UUID)
//!   - constellation_pubkey (base64) — devices match by constellation
//!   - port (signaling port the device listens on)
//!   - friendly_name, form_factor, status
//!
//! Discovery happens passively (background browse). Discovered devices that
//! belong to the same constellation are surfaced to the UI with handoff/
//! direct-connect options.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use miru_constellation::{DeviceCapabilities, FormFactor};
use parking_lot::RwLock;
use std::{collections::HashMap, net::IpAddr, sync::Arc, time::Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

const SERVICE_TYPE: &str = "_miru._tcp.local.";

/// What we advertise about ourselves.
#[derive(Debug, Clone)]
pub struct LocalAdvertisement {
    pub device_id: Uuid,
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
    pub device_id: Uuid,
    pub constellation_pubkey: String,
    pub friendly_name: String,
    pub form_factor: FormFactor,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub last_seen: std::time::Instant,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    /// Retained for future unregister()/re-advertise; not read in v0.1.
    #[allow(dead_code)]
    instance_name: String,
    peers: Arc<RwLock<HashMap<Uuid, DiscoveredPeer>>>,
}

impl Discovery {
    pub fn start(advert: LocalAdvertisement) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("create mDNS daemon")?;

        // Encode our advertisement as TXT records.
        let txt = [
            ("device_id".to_string(), advert.device_id.to_string()),
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

        let instance_name = format!("miru-{}", &advert.device_id.simple().to_string()[..12]);
        let hostname = format!("{instance_name}.local.");

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &hostname,
            "", // empty IP → uses local interfaces
            advert.port,
            &txt[..],
        )?
        .enable_addr_auto();

        daemon.register(info).context("register service")?;
        info!("Discovery: announcing as {}", instance_name);

        let peers = Arc::new(RwLock::new(HashMap::new()));
        let receiver = daemon.browse(SERVICE_TYPE).context("start browse")?;
        let peers_clone = Arc::clone(&peers);
        let our_id = advert.device_id;

        tokio::spawn(async move {
            while let Ok(event) = receiver.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if let Some(peer) = parse_peer(&info) {
                            // Skip self
                            if peer.device_id == our_id {
                                continue;
                            }
                            debug!(
                                "Discovery: found {} ({})",
                                peer.friendly_name, peer.device_id
                            );
                            peers_clone.write().insert(peer.device_id, peer);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, name) => {
                        debug!("Discovery: removed {}", name);
                        // We can't easily map name back to UUID without more bookkeeping;
                        // entries naturally age out via `prune_stale()`.
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

    /// Drop peers we haven't seen in `older_than`.
    pub fn prune_stale(&self, older_than: Duration) {
        let cutoff = std::time::Instant::now()
            .checked_sub(older_than)
            .unwrap_or_else(std::time::Instant::now);
        self.peers.write().retain(|_, p| p.last_seen > cutoff);
    }

    pub fn shutdown(self) {
        if let Err(e) = self.daemon.shutdown() {
            warn!("Discovery shutdown: {:?}", e);
        }
    }
}

fn parse_peer(info: &ServiceInfo) -> Option<DiscoveredPeer> {
    let txt: HashMap<String, String> = info
        .get_properties()
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect();

    let device_id = txt.get("device_id")?.parse::<Uuid>().ok()?;
    let constellation_pubkey = txt.get("constellation")?.clone();
    let friendly_name = txt.get("name").cloned().unwrap_or_default();
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
    })
}

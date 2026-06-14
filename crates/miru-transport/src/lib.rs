//! Transport layer.

pub mod connection;
pub mod handshake;
pub mod nat;
pub mod quic;
pub mod relay;
pub mod signaling;
pub mod tls;

pub use connection::{Connection, ConnectionMode};
pub use nat::{detect_nat_type, discover_public_addr, is_punchable, punch_to_peer, NatType};
pub use signaling::{verify_register_signature, SignalClient, SignalEvent};
pub use tls::{pinned_client_config, strict_client_config};

use anyhow::Result;
use miru_common::message::Msg;

pub trait Transport: Send + Sync + 'static {
    fn send(&self, msg: Msg) -> impl std::future::Future<Output = Result<()>> + Send;
    fn recv(&self) -> impl std::future::Future<Output = Result<Option<Msg>>> + Send;
    fn rtt_ms(&self) -> u32;
    fn close(&self);
}

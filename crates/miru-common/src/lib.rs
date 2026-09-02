//! Miru — shared types, protocol messages, crypto primitives.
//!
//! WHY: Single source of truth for wire format and domain types.
//!      Both host and client compile this; no duplication.

pub mod backoff;
pub mod codec;
pub mod crypto;
pub mod display_map;
pub mod error;
pub mod message;
pub mod perf;
pub mod session;
pub mod wol;

pub use error::{MiruError, Result};

#[cfg(test)]
mod tests;

// ─── Version ──────────────────────────────────────────────────────────────────

pub const PROTOCOL_VERSION: u32 = 1;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

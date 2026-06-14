#![no_main]
//! Fuzz target: ensure the MessagePack (wire format) message parser never panics
//! on arbitrary input.
//!
//! The wire protocol uses rmp_serde (MessagePack), not JSON. This target covers
//! the actual deserialization path used in relay.rs::recv_msg().

use libfuzzer_sys::fuzz_target;
use miru_common::message::Msg;

fuzz_target!(|data: &[u8]| {
    let _ = rmp_serde::from_slice::<Msg>(data);
});

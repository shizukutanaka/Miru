#![no_main]
//! Fuzz target: STUN XOR-MAPPED-ADDRESS parser must never panic on arbitrary input.
//!
//! The STUN response is received over UDP from an external server before any
//! authenticated session exists. A crafted packet could previously panic the
//! parser with attr_len < 4 for XOR-MAPPED-ADDRESS (out-of-bounds index).

use libfuzzer_sys::fuzz_target;
use miru_transport::nat::parse_stun_xor_mapped_fuzz;

fuzz_target!(|data: &[u8]| {
    // Must not panic — only Ok(addr) or Err(_) are acceptable outcomes.
    let _ = parse_stun_xor_mapped_fuzz(data);
});

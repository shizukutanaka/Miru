#![no_main]
//! Fuzz target: ensure the JSON message parser never panics on arbitrary input.

use libfuzzer_sys::fuzz_target;
use miru_common::message::Msg;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Msg>(data);
});

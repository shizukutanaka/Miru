#![no_main]
//! Fuzz target: ensure the cipher's decrypt path never panics on bad input.

use libfuzzer_sys::fuzz_target;
use miru_common::crypto::SessionCipher;

fuzz_target!(|data: &[u8]| {
    let cipher = SessionCipher::new([0u8; 32]);
    let _ = cipher.decrypt(data); // expects Err on garbage, must not panic
});

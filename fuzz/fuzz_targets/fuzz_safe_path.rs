#![no_main]
//! Fuzz target: resolve_safe_path() must never panic on arbitrary peer-supplied paths.
//!
//! A file-transfer peer sends the path as a raw string. This target ensures
//! that path traversal attempts (../, symlink tricks, NUL bytes, overlong names,
//! Windows-style separators) either return Err or a path strictly inside the root.

use libfuzzer_sys::fuzz_target;
use miru_host::safe_fs::{resolve_safe_path, FileTransferConfig};
use std::path::PathBuf;

fuzz_target!(|data: &[u8]| {
    // Interpret as a string (lossy) — peers send UTF-8 but may send garbage.
    let requested = String::from_utf8_lossy(data);

    let config = FileTransferConfig {
        allowed_roots: vec![PathBuf::from("/tmp/miru-fuzz")],
        max_file_bytes: 1024 * 1024,
        max_session_bytes: 10 * 1024 * 1024,
    };

    match resolve_safe_path(&config, &requested) {
        Ok(p) => {
            // If resolution succeeds, the path MUST be inside the allowed root.
            assert!(
                p.starts_with("/tmp/miru-fuzz"),
                "resolved path escaped allowed root: {}",
                p.display()
            );
        }
        Err(_) => {} // rejection is always acceptable
    }
});

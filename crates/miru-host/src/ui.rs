//! Host status output — prints device ID and pairing fingerprint to stdout.
//!
//! A future iteration will add a system tray icon (via tray-icon crate)
//! and a small status window. For headless servers the CLI output is enough.

use miru_auth::DeviceIdentity;
use miru_common::session::DeviceId;

pub fn print_banner(device_id: &DeviceId, identity: &DeviceIdentity) {
    let fpr = identity.pubkey_fingerprint();
    println!();
    println!("  ╭──────────────────────────────────────────╮");
    println!("  │                                          │");
    println!("  │        Miru Host  —  Ready               │");
    println!("  │                                          │");
    println!("  │   Device ID:  {:<27}│", device_id.0);
    println!("  │   Fingerprint:                           │");
    println!("  │     {fpr:<37}│");
    println!("  │                                          │");
    println!("  │   Tell the connecting party your         │");
    println!("  │   Device ID. Verify the fingerprint      │");
    println!("  │   matches when they connect.             │");
    println!("  │                                          │");
    println!("  ╰──────────────────────────────────────────╯");
    println!();
    println!("  Waiting for connections...");
    println!("  Press Ctrl+C to stop.");
    println!();
}

//! Real-display integration test — exercises X11Capturer (incl. the XRandR
//! monitor enumeration) against an actual X server.
//!
//! Skips silently when `$DISPLAY` is unset so plain `cargo test` works on
//! headless machines; CI runs it under Xvfb.

#![cfg(target_os = "linux")]

use miru_capture::{create_capturer, ScreenCapturer};

fn have_display() -> bool {
    std::env::var("DISPLAY").is_ok()
}

#[test]
fn enumerates_at_least_one_display() {
    if !have_display() {
        eprintln!("skipped: no $DISPLAY");
        return;
    }
    let cap = create_capturer().expect("create capturer");
    let displays = cap.displays().expect("list displays");
    assert!(!displays.is_empty(), "at least one display expected");
    for d in &displays {
        assert!(d.width > 0 && d.height > 0, "display {d:?} has zero size");
        assert!(!d.name.is_empty());
    }
    // Exactly one primary (or none if the server reports none — but never >1).
    assert!(displays.iter().filter(|d| d.primary).count() <= 1);
}

#[test]
fn captures_frame_matching_selected_display() {
    if !have_display() {
        eprintln!("skipped: no $DISPLAY");
        return;
    }
    let mut cap = create_capturer().expect("create capturer");
    let displays = cap.displays().expect("list displays");
    let first = &displays[0];

    cap.select_display(0).expect("select display 0");
    let frame = cap
        .next_frame()
        .expect("capture frame")
        .expect("frame available");

    assert_eq!(frame.width, first.width, "frame width == display width");
    assert_eq!(frame.height, first.height, "frame height == display height");
    assert_eq!(frame.stride, frame.width * 4, "BGRA stride");
    assert_eq!(
        frame.data.len(),
        (frame.width * frame.height * 4) as usize,
        "buffer holds exactly width*height BGRA pixels"
    );
}

#[test]
fn out_of_range_display_rejected() {
    if !have_display() {
        eprintln!("skipped: no $DISPLAY");
        return;
    }
    let mut cap = create_capturer().expect("create capturer");
    let displays = cap.displays().expect("list displays");
    // Index far past any real monitor must be rejected (when XRandR works).
    if cap.select_display(200).is_ok() {
        // Acceptable only in the no-RandR fallback (single full-root display).
        assert_eq!(
            displays.len(),
            1,
            "select(200) may only succeed in fallback mode"
        );
    }
}

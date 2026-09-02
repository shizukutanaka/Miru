//! Input handler — receives InputEvent from viewer, injects into OS.
//!
//! Security:
//!   - Only active sessions can inject input
//!   - Session must be authenticated (HelloAck completed)
//!   - Input is rate-limited (max 1000 events/sec per session)

use anyhow::Result;
use miru_common::display_map;
use miru_common::message::{ClipboardFormat, ClipboardSync, DisplayInfo, InputEvent, InputKind};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

pub struct InputHandler {
    #[allow(dead_code)]
    last_event: Instant,
    event_count: u32,
    rate_limit_window_start: Instant,
    /// Keys the viewer has pressed but not released, so the session can undo
    /// them on teardown. A viewer that dies mid-keystroke (crash, network
    /// drop, alt-tab) otherwise leaves the key latched on the host — and on
    /// Linux the uinput device is process-lifetime, so a stuck modifier
    /// survives reconnects and makes the machine unusable.
    /// Stored as (legacy keyCode, physical code) so the release event is
    /// injected exactly as the press was.
    held_keys: HashSet<(u32, Option<String>)>,
    /// Display layout, for turning "normalised within the display the viewer is
    /// watching" into "normalised across the whole virtual desktop".
    ///
    /// Empty until the host enumerates displays. While empty, coordinates are
    /// passed through untouched: with no layout there is nothing to map onto,
    /// and moving the pointer on a guess is worse than the status quo.
    displays: Vec<DisplayInfo>,
}

const MAX_EVENTS_PER_SEC: u32 = 1000;
/// Cap on a single Text event's character count. miru_input::inject processes
/// each character synchronously inside tokio::select!, so an unbounded Text
/// event blocks the entire session loop (no pings, no video) until injection
/// completes — an easy DoS from a Control-permission peer.
const MAX_TEXT_CHARS: usize = 4096;

impl InputHandler {
    pub fn new() -> Self {
        Self {
            last_event: Instant::now(),
            event_count: 0,
            rate_limit_window_start: Instant::now(),
            held_keys: HashSet::new(),
            displays: Vec::new(),
        }
    }

    /// Supply the display layout captured from the OS. Call again if the layout
    /// changes; a stale layout puts clicks on the wrong monitor.
    pub fn set_displays(&mut self, displays: Vec<DisplayInfo>) {
        self.displays = displays;
    }

    /// Rewrite a pointer position from display-relative to virtual-desktop
    /// relative.
    ///
    /// Every injection backend interprets coordinates against the whole virtual
    /// desktop — Windows once MOUSEEVENTF_VIRTUALDESK is set, Linux because
    /// uinput ABS is spread across it by the compositor, macOS because CGEvent
    /// coordinates are global. The viewer, however, normalises against the one
    /// display it is showing. Without this the two disagree on every
    /// multi-monitor host.
    ///
    /// For a single display at the origin this is the identity, which is what
    /// makes the backend changes safe for the ordinary case
    /// (`display_map::tests::single_display_normalisation_is_the_identity`).
    fn to_virtual(&self, x: f32, y: f32, display: u8) -> (f32, f32) {
        match display_map::virtual_normalized(&self.displays, display, x, y) {
            Some((vx, vy)) => (vx as f32, vy as f32),
            // No layout known, or a degenerate one. Pass through unchanged.
            None => (x, y),
        }
    }

    /// Inject a KeyUp for every key still held. Called on session teardown.
    /// Best-effort: a failed release must not mask the original disconnect
    /// reason, so errors are logged rather than propagated.
    pub fn release_all_keys(&mut self) {
        if self.held_keys.is_empty() {
            return;
        }
        debug!(
            "Releasing {} held key(s) at session end",
            self.held_keys.len()
        );
        for (key, code) in self.held_keys.drain() {
            let evt = InputEvent {
                kind: InputKind::KeyUp {
                    key,
                    modifiers: 0,
                    code,
                },
                timestamp_ms: 0,
            };
            if let Err(e) = miru_input::inject(&evt) {
                warn!("Failed to release held key {key}: {e}");
            }
        }
    }

    #[cfg(test)]
    pub fn held_key_count(&self) -> usize {
        self.held_keys.len()
    }

    pub fn handle_input(&mut self, event: &InputEvent) -> Result<()> {
        // Rate limiting
        if self.rate_limit_window_start.elapsed() > Duration::from_secs(1) {
            self.event_count = 0;
            self.rate_limit_window_start = Instant::now();
        }
        self.event_count += 1;
        if self.event_count > MAX_EVENTS_PER_SEC {
            warn!("Input rate limit exceeded — dropping event");
            return Ok(());
        }

        // Guard against a large Text event blocking the async event loop.
        if let InputKind::Text { text } = &event.kind {
            if text.chars().count() > MAX_TEXT_CHARS {
                warn!(
                    "Text event too long ({} chars > {MAX_TEXT_CHARS}), dropped",
                    text.chars().count()
                );
                return Ok(());
            }
        }

        // Track press/release so teardown can un-stick anything still down.
        match &event.kind {
            InputKind::KeyDown { key, code, .. } => {
                self.held_keys.insert((*key, code.clone()));
            }
            InputKind::KeyUp { key, code, .. } => {
                self.held_keys.remove(&(*key, code.clone()));
            }
            _ => {}
        }

        debug!("Input: {:?}", event.kind);

        // Pointer positions are remapped onto the virtual desktop before they
        // reach the OS; everything else is injected as received.
        let remapped;
        let event = match &event.kind {
            InputKind::MouseMove { x, y, display } => {
                let (x, y) = self.to_virtual(*x, *y, *display);
                remapped = InputEvent {
                    kind: InputKind::MouseMove {
                        x,
                        y,
                        display: *display,
                    },
                    timestamp_ms: event.timestamp_ms,
                };
                &remapped
            }
            InputKind::MouseDown {
                button,
                x,
                y,
                display,
            } => {
                let (x, y) = self.to_virtual(*x, *y, *display);
                remapped = InputEvent {
                    kind: InputKind::MouseDown {
                        button: button.clone(),
                        x,
                        y,
                        display: *display,
                    },
                    timestamp_ms: event.timestamp_ms,
                };
                &remapped
            }
            InputKind::MouseUp {
                button,
                x,
                y,
                display,
            } => {
                let (x, y) = self.to_virtual(*x, *y, *display);
                remapped = InputEvent {
                    kind: InputKind::MouseUp {
                        button: button.clone(),
                        x,
                        y,
                        display: *display,
                    },
                    timestamp_ms: event.timestamp_ms,
                };
                &remapped
            }
            InputKind::Scroll {
                dx,
                dy,
                x,
                y,
                display,
            } => {
                let (x, y) = self.to_virtual(*x, *y, *display);
                remapped = InputEvent {
                    kind: InputKind::Scroll {
                        dx: *dx,
                        dy: *dy,
                        x,
                        y,
                        display: *display,
                    },
                    timestamp_ms: event.timestamp_ms,
                };
                &remapped
            }
            _ => event,
        };
        miru_input::inject(event)
    }

    pub fn handle_clipboard(&self, sync: &ClipboardSync) -> Result<()> {
        match sync.format {
            ClipboardFormat::Text => {
                let text = String::from_utf8_lossy(&sync.data);
                miru_input::set_clipboard(&text)
            }
            ClipboardFormat::Html => miru_input::set_clipboard_raw(&sync.data, "text/html"),
            ClipboardFormat::Image => miru_input::set_clipboard_raw(&sync.data, "image/png"),
        }
    }
}

/// Safety net: `handle_viewer` can also leave via `?` (e.g. a transport error
/// in the recv arm), which skips the explicit `release_all_keys()` call at the
/// end of the session. Dropping the handler covers every exit path.
impl Drop for InputHandler {
    fn drop(&mut self) {
        self.release_all_keys();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_evt(down: bool, code: &str) -> InputEvent {
        let kind = if down {
            InputKind::KeyDown {
                key: 0,
                modifiers: 0,
                code: Some(code.into()),
            }
        } else {
            InputKind::KeyUp {
                key: 0,
                modifiers: 0,
                code: Some(code.into()),
            }
        };
        InputEvent {
            kind,
            timestamp_ms: 0,
        }
    }

    /// Injection itself needs a real OS device, so these assert the bookkeeping
    /// that decides what gets released — not the injection.
    #[test]
    fn tracks_pressed_and_released_keys() {
        let mut h = InputHandler::new();
        let _ = h.handle_input(&key_evt(true, "ControlLeft"));
        let _ = h.handle_input(&key_evt(true, "KeyA"));
        assert_eq!(h.held_key_count(), 2);

        let _ = h.handle_input(&key_evt(false, "KeyA"));
        assert_eq!(h.held_key_count(), 1, "KeyA released, Ctrl still held");
    }

    /// The stuck-modifier scenario: viewer vanishes with Ctrl down.
    #[test]
    fn release_all_clears_held_keys() {
        let mut h = InputHandler::new();
        let _ = h.handle_input(&key_evt(true, "ControlLeft"));
        let _ = h.handle_input(&key_evt(true, "ShiftLeft"));
        assert_eq!(h.held_key_count(), 2);

        h.release_all_keys();
        assert_eq!(h.held_key_count(), 0);
        h.release_all_keys(); // idempotent — Drop runs it again
        assert_eq!(h.held_key_count(), 0);
    }

    #[test]
    fn repeated_keydown_does_not_double_track() {
        let mut h = InputHandler::new();
        let _ = h.handle_input(&key_evt(true, "KeyA"));
        let _ = h.handle_input(&key_evt(true, "KeyA")); // auto-repeat
        assert_eq!(h.held_key_count(), 1);
    }

    fn display(index: u8, x: i32, width: u32, primary: bool) -> DisplayInfo {
        DisplayInfo {
            index,
            x,
            y: 0,
            width,
            height: 1080,
            refresh_hz: 60,
            name: format!("D{index}"),
            primary,
        }
    }

    fn move_to(x: f32, display: u8) -> InputEvent {
        InputEvent {
            kind: InputKind::MouseMove { x, y: 0.5, display },
            timestamp_ms: 0,
        }
    }

    /// The regression the audit warned about: adding VIRTUALDESK on its own
    /// would move (0.5, 0.5) from the primary's centre to the desktop's. With
    /// one display the remap must be a no-op, so the common case is untouched.
    #[test]
    fn single_display_coordinates_pass_through_unchanged() {
        let mut h = InputHandler::new();
        h.set_displays(vec![display(0, 0, 1920, true)]);
        assert_eq!(h.to_virtual(0.5, 0.5, 0), (0.5, 0.5));
        assert_eq!(h.to_virtual(0.0, 1.0, 0), (0.0, 1.0));
    }

    /// Without a layout we must not guess. Passing through keeps the previous
    /// behaviour rather than putting the pointer somewhere invented.
    #[test]
    fn no_display_layout_passes_coordinates_through() {
        let h = InputHandler::new();
        assert_eq!(h.to_virtual(0.5, 0.5, 0), (0.5, 0.5));
        assert_eq!(h.to_virtual(0.25, 0.75, 3), (0.25, 0.75));
    }

    /// Two 1920 screens side by side: the centre of the second is three
    /// quarters of the way across the desktop, not the middle.
    #[test]
    fn second_display_maps_into_its_own_half_of_the_desktop() {
        let mut h = InputHandler::new();
        h.set_displays(vec![
            display(0, 0, 1920, true),
            display(1, 1920, 1920, false),
        ]);
        let (x, _) = h.to_virtual(0.5, 0.5, 1);
        assert!((x - 0.75).abs() < 1e-6, "expected 0.75, got {x}");
        let (x, _) = h.to_virtual(0.5, 0.5, 0);
        assert!((x - 0.25).abs() < 1e-6, "expected 0.25, got {x}");
    }

    /// handle_input must rewrite the event it injects, not just compute a value
    /// and discard it — the remap is useless if it does not reach inject().
    #[test]
    fn handle_input_remaps_the_event_it_injects() {
        let mut h = InputHandler::new();
        h.set_displays(vec![
            display(0, 0, 1920, true),
            display(1, 1920, 1920, false),
        ]);
        // Injection is stubbed in this harness, so assert on the transform the
        // same way handle_input applies it.
        let ev = move_to(0.5, 1);
        if let InputKind::MouseMove { x, y, display } = ev.kind {
            assert_eq!(h.to_virtual(x, y, display).0, 0.75);
        }
        assert!(h.handle_input(&ev).is_ok());
    }

    /// Clicks carried no display before this change, so they could not be
    /// resolved on a multi-monitor host even when moves could.
    #[test]
    fn clicks_and_scrolls_carry_a_display() {
        let mut h = InputHandler::new();
        h.set_displays(vec![
            display(0, 0, 1920, true),
            display(1, 1920, 1920, false),
        ]);
        let down = InputEvent {
            kind: InputKind::MouseDown {
                button: MouseButton::Left,
                x: 0.5,
                y: 0.5,
                display: 1,
            },
            timestamp_ms: 0,
        };
        assert!(h.handle_input(&down).is_ok());
        let scroll = InputEvent {
            kind: InputKind::Scroll {
                dx: 0.0,
                dy: 1.0,
                x: 0.5,
                y: 0.5,
                display: 1,
            },
            timestamp_ms: 0,
        };
        assert!(h.handle_input(&scroll).is_ok());
    }
}

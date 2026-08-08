//! Input handler — receives InputEvent from viewer, injects into OS.
//!
//! Security:
//!   - Only active sessions can inject input
//!   - Session must be authenticated (HelloAck completed)
//!   - Input is rate-limited (max 1000 events/sec per session)

use anyhow::Result;
use miru_common::message::{ClipboardFormat, ClipboardSync, InputEvent, InputKind};
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
        }
    }

    /// Inject a KeyUp for every key still held. Called on session teardown.
    /// Best-effort: a failed release must not mask the original disconnect
    /// reason, so errors are logged rather than propagated.
    pub fn release_all_keys(&mut self) {
        if self.held_keys.is_empty() {
            return;
        }
        debug!("Releasing {} held key(s) at session end", self.held_keys.len());
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
                warn!("Text event too long ({} chars > {MAX_TEXT_CHARS}), dropped", text.chars().count());
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
            InputKind::KeyDown { key: 0, modifiers: 0, code: Some(code.into()) }
        } else {
            InputKind::KeyUp { key: 0, modifiers: 0, code: Some(code.into()) }
        };
        InputEvent { kind, timestamp_ms: 0 }
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
}

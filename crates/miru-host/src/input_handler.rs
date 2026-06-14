//! Input handler — receives InputEvent from viewer, injects into OS.
//!
//! Security:
//!   - Only active sessions can inject input
//!   - Session must be authenticated (HelloAck completed)
//!   - Input is rate-limited (max 1000 events/sec per session)

use anyhow::Result;
use miru_common::message::{ClipboardFormat, ClipboardSync, InputEvent, InputKind};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

pub struct InputHandler {
    #[allow(dead_code)]
    last_event: Instant,
    event_count: u32,
    rate_limit_window_start: Instant,
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
        }
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

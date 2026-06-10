//! Input handler — receives InputEvent from viewer, injects into OS.
//!
//! Security:
//!   - Only active sessions can inject input
//!   - Session must be authenticated (HelloAck completed)
//!   - Input is rate-limited (max 1000 events/sec per session)

use anyhow::Result;
use miru_common::message::{ClipboardFormat, ClipboardSync, InputEvent};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

pub struct InputHandler {
    #[allow(dead_code)]
    last_event: Instant,
    event_count: u32,
    rate_limit_window_start: Instant,
}

const MAX_EVENTS_PER_SEC: u32 = 1000;

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

        debug!("Input: {:?}", event.kind);
        miru_input::inject(event)
    }

    pub fn handle_clipboard(&self, sync: &ClipboardSync) -> Result<()> {
        match sync.format {
            ClipboardFormat::Text => {
                let text = String::from_utf8_lossy(&sync.data);
                miru_input::set_clipboard(&text)
            }
            _ => Ok(()), // TODO: image/html clipboard
        }
    }
}

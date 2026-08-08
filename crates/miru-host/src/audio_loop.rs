//! System-audio capture loop — runs on a dedicated OS thread.
//!
//! cpal's `Stream` is !Send, so (like the viewer's playback thread) the whole
//! capture+encode pipeline lives on its own `std::thread`. Mirrors
//! `capture_loop.rs`: returns a `flume::Receiver<Msg>` of `Msg::AudioFrame`.
//! When the session drops that receiver (session end), the encode thread's
//! `try_send` reports Disconnected and the thread exits, which drops the cpal
//! stream and stops capture.
//!
//! KNOWN LIMITATION — multiple concurrent viewers: `handle_viewer` is spawned
//! per incoming connection with no concurrency cap, so each session calls
//! `start()` and opens its own capture stream on the same monitor device.
//! System audio is really a singleton resource, so the second and later viewers
//! may fail to open it (device busy) — that path degrades safely (a warning is
//! logged and only that session is silent; nothing crashes), but it is wasteful
//! and inconsistent. The correct design is one capture fanned out to all
//! sessions via a broadcast channel; see docs/FEATURE_AUDIT.md item 1.

use anyhow::Result;
use flume::{Receiver, TrySendError};
use miru_audio::capture::SystemAudioCapture;
use miru_audio::{AudioEncoder, Layout};
use miru_common::message::Msg;
use tracing::{info, warn};

/// Start system-audio capture + Opus encode. Returns a receiver of
/// `Msg::AudioFrame`. The thread spawns immediately but capture only actually
/// begins if a loopback/monitor device exists; if not, the thread logs and
/// exits and the receiver simply never yields (caller degrades to no audio).
pub fn start(bitrate_kbps: u32) -> Result<Receiver<Msg>> {
    // Encoded-frame channel to the session loop — same depth/drop policy as
    // video (capture_loop.rs uses bounded(8)).
    let (out_tx, out_rx) = flume::bounded::<Msg>(8);
    // Raw f32 chunks from cpal's realtime thread → our encode thread.
    let (raw_tx, raw_rx) = flume::bounded::<Vec<f32>>(16);

    std::thread::Builder::new()
        .name("miru-audio-capture".into())
        .spawn(move || {
            // Owns the !Send cpal stream; kept alive until this thread returns.
            let _capture = match SystemAudioCapture::start(move |samples| {
                // Never block cpal's realtime callback — drop on a full queue.
                let _ = raw_tx.try_send(samples.to_vec());
            }) {
                Ok(c) => c,
                Err(e) => {
                    warn!("System audio capture unavailable: {e}");
                    return;
                }
            };

            let mut enc = match AudioEncoder::new(Layout::Stereo, bitrate_kbps) {
                Ok(e) => e,
                Err(e) => {
                    warn!("Audio encoder init failed: {e}");
                    return;
                }
            };
            info!("System audio capture active (stereo 48kHz, {bitrate_kbps}kbps)");

            // Block on raw chunks; AudioEncoder buffers internally and emits
            // zero-or-more 20ms Opus frames per call. Exit when the session
            // drops the output receiver (Disconnected) or capture ends.
            while let Ok(chunk) = raw_rx.recv() {
                match enc.encode(&chunk) {
                    Ok(frames) => {
                        for f in frames {
                            match out_tx.try_send(Msg::AudioFrame(f)) {
                                Ok(()) => {}
                                // Consumer gone — stop capturing.
                                Err(TrySendError::Disconnected(_)) => return,
                                // Transient backlog — drop this frame, keep going.
                                Err(TrySendError::Full(_)) => {}
                            }
                        }
                    }
                    Err(e) => warn!("audio encode: {e}"),
                }
            }
        })?;

    Ok(out_rx)
}

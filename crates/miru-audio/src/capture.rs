//! System-audio capture — feeds interleaved f32 samples from a loopback/monitor
//! device to a sink callback.
//!
//! Only *loopback/monitor* input devices are used (e.g. a PulseAudio/PipeWire
//! "Monitor of <sink>" source on Linux, or a virtual loopback device such as
//! BlackHole / VB-Cable on macOS/Windows). We deliberately never fall back to
//! the default microphone: advertising "system audio" while capturing a mic
//! would be a dishonest surprise. If no loopback device is found, `start`
//! returns Err and the caller keeps audio disabled (degrade, don't crash).
//!
//! Native OS loopback without a virtual device (WASAPI `AUDCLNT_STREAMFLAGS_
//! LOOPBACK` on Windows, a CoreAudio process tap on macOS) is a follow-up; see
//! docs/RESEARCH_NOTES.md §5.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleRate, StreamConfig};
use tracing::{info, warn};

/// Fixed capture format. 48 kHz matches `AudioEncoder`'s expectation and stereo
/// is the most broadly-supported interleaved layout, so no resample/downmix is
/// needed. Devices that can't provide this exact config make `start` fail,
/// which the caller treats as "audio unavailable".
pub const CAPTURE_SAMPLE_RATE: u32 = 48_000;
pub const CAPTURE_CHANNELS: u16 = 2;

pub struct SystemAudioCapture {
    // cpal's Stream is !Send and stops when dropped — keep it alive for the
    // lifetime of capture.
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u8,
}

impl SystemAudioCapture {
    /// Start capturing from a loopback/monitor device. `sink` is called from
    /// cpal's realtime audio thread with interleaved f32 frames (stereo,
    /// 48 kHz) — it must not block. Keep the returned value alive for the
    /// duration of capture; dropping it stops the stream.
    pub fn start(sink: impl Fn(&[f32]) + Send + 'static) -> Result<Self> {
        let host = cpal::default_host();
        let device = find_loopback_device(&host)
            .context("no loopback/monitor audio device found — system-audio capture unavailable")?;
        info!(
            "System audio capture device: {}",
            device.name().unwrap_or_default()
        );

        let config = StreamConfig {
            channels: CAPTURE_CHANNELS,
            sample_rate: SampleRate(CAPTURE_SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _| sink(data),
                |err| warn!("audio capture stream error: {err}"),
                None,
            )
            .context("build input stream (device may not support 48kHz stereo f32)")?;
        stream.play().context("start capture stream")?;

        Ok(Self {
            _stream: stream,
            sample_rate: CAPTURE_SAMPLE_RATE,
            channels: CAPTURE_CHANNELS as u8,
        })
    }
}

/// Whether a loopback/monitor capture device is currently available. Cheap
/// (device enumeration only, no stream) so the host can decide whether to
/// advertise `Features.audio` before the handshake. Never panics: a host with
/// no audio backend (headless CI) simply reports false.
pub fn loopback_available() -> bool {
    find_loopback_device(&cpal::default_host()).is_some()
}

/// Find a loopback/monitor *input* device. Returns None if the host exposes
/// none — we never fall back to a microphone (see module docs).
fn find_loopback_device(host: &cpal::Host) -> Option<cpal::Device> {
    let devices = host.input_devices().ok()?;
    for d in devices {
        if let Ok(name) = d.name() {
            let lname = name.to_lowercase();
            if lname.contains("monitor") || lname.contains("loopback") {
                return Some(d);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_format_matches_encoder_expectation() {
        // AudioEncoder is built for 48kHz; stereo keeps the interleave simple.
        assert_eq!(CAPTURE_SAMPLE_RATE, 48_000);
        assert_eq!(CAPTURE_CHANNELS, 2);
    }

    #[test]
    fn loopback_available_never_panics() {
        // Must be safe to call on a headless host with no audio backend.
        let _ = loopback_available();
    }
}

//! System audio capture via cpal.
//!
//! Captures the default output device (loopback) where available.
//! Windows: WASAPI loopback is native.
//! macOS:   Requires virtual audio driver (e.g. BlackHole) or ScreenAudio via SCKit.
//! Linux:   PulseAudio/PipeWire monitor source.

use anyhow::Result;
#[allow(unused_imports)]
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    StreamConfig,
};
use crossbeam_channel::{bounded, Receiver};
use tracing::{info, warn};

pub struct AudioFrame {
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u8,
    pub timestamp_ms: u64,
}

pub struct AudioCapturer {
    rx: Receiver<AudioFrame>,
    _stream: cpal::Stream,
}

impl AudioCapturer {
    /// Opens the default output device for loopback capture.
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();

        // Prefer loopback device if available (Windows WASAPI)
        let device = Self::find_loopback_device(&host)
            .or_else(|| host.default_output_device())
            .ok_or_else(|| anyhow::anyhow!("no audio output device"))?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as u8;

        info!(
            "Audio capture: {} @ {}Hz {}ch",
            device.name().unwrap_or_default(),
            sample_rate,
            channels
        );

        let (tx, rx) = bounded::<AudioFrame>(64);

        let stream = device.build_input_stream(
            &StreamConfig {
                channels: config.channels(),
                sample_rate: config.sample_rate(),
                buffer_size: cpal::BufferSize::Default,
            },
            {
                let tx = tx.clone();
                move |data: &[f32], _| {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let _ = tx.try_send(AudioFrame {
                        data: data.to_vec(),
                        sample_rate,
                        channels,
                        timestamp_ms: ts,
                    });
                }
            },
            |err| warn!("audio capture error: {err}"),
            None,
        )?;

        stream.play()?;

        Ok(Self {
            rx,
            _stream: stream,
        })
    }

    /// Returns the next audio frame, blocking until available.
    pub fn next_frame(&self) -> Result<AudioFrame> {
        Ok(self.rx.recv()?)
    }

    /// Non-blocking variant.
    pub fn try_next_frame(&self) -> Option<AudioFrame> {
        self.rx.try_recv().ok()
    }

    fn find_loopback_device(host: &cpal::Host) -> Option<cpal::Device> {
        host.input_devices().ok()?.find(|d| {
            d.name()
                .map(|n| n.contains("Monitor") || n.contains("Loopback") || n.contains("CABLE"))
                .unwrap_or(false)
        })
    }
}

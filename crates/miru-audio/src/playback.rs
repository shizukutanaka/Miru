//! Audio playback — feeds decoded f32 samples to the system's default output.

use anyhow::{Context, Result};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleRate, StreamConfig,
};
use crossbeam_channel::{bounded, Sender};
use tracing::{info, trace, warn};

pub struct AudioPlayer {
    tx: Sender<Vec<f32>>,
    _stream: cpal::Stream,
}

impl AudioPlayer {
    pub fn new(channels: u8, sample_rate: u32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("no default audio output")?;

        info!("Audio playback: {}", device.name().unwrap_or_default());

        let config = StreamConfig {
            channels: channels as u16,
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let (tx, rx) = bounded::<Vec<f32>>(8); // ~160ms buffer (8 × 20ms)
        let mut leftover: Vec<f32> = Vec::new();

        let stream = device.build_output_stream(
            &config,
            move |out: &mut [f32], _| {
                let mut filled = 0;
                while filled < out.len() {
                    if leftover.is_empty() {
                        match rx.try_recv() {
                            Ok(samples) => leftover = samples,
                            Err(_) => {
                                trace!("audio underrun — filling silence ({} samples)", out.len() - filled);
                                for s in &mut out[filled..] {
                                    *s = 0.0;
                                }
                                return;
                            }
                        }
                    }
                    let n = (out.len() - filled).min(leftover.len());
                    out[filled..filled + n].copy_from_slice(&leftover[..n]);
                    leftover.drain(..n);
                    filled += n;
                }
            },
            |err| warn!("audio output error: {err}"),
            None,
        )?;

        stream.play()?;
        Ok(Self {
            tx,
            _stream: stream,
        })
    }

    /// Push decoded samples to the output queue.
    /// Drops the oldest if queue is full (avoid latency drift).
    pub fn push(&self, samples: Vec<f32>) {
        if self.tx.try_send(samples).is_err() {
            // Queue full — silently drop (better than blocking the network)
        }
    }
}

//! Audio: Opus codec + playback.
//!
//! Supports mono, stereo, and 5.1/7.1 multi-channel (Sunshine-class).
//! Channels mapping: SMPTE / WAV order (FL, FR, FC, LFE, BL, BR, SL, SR).

use anyhow::{bail, Result};
use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::packet::Packet;
use audiopus::MutSignals;
use audiopus::{Application, Channels, SampleRate};
use miru_common::message::{AudioCodec, AudioFrame};
use std::time::{SystemTime, UNIX_EPOCH};

const FRAME_SIZE: usize = 960; // 20ms @ 48kHz
const SAMPLE_RATE: i32 = 48_000;

/// Channel layout. Multi-channel uses one Opus stream per pair (stereo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Mono,
    Stereo,
    /// 5.1: FL, FR, FC, LFE, BL, BR
    Surround51,
    /// 7.1: FL, FR, FC, LFE, BL, BR, SL, SR
    Surround71,
}

impl Layout {
    pub fn channels(self) -> u8 {
        match self {
            Layout::Mono => 1,
            Layout::Stereo => 2,
            Layout::Surround51 => 6,
            Layout::Surround71 => 8,
        }
    }

    pub fn from_channels(n: u8) -> Result<Self> {
        Ok(match n {
            1 => Layout::Mono,
            2 => Layout::Stereo,
            6 => Layout::Surround51,
            8 => Layout::Surround71,
            _ => bail!("unsupported channel count: {n}"),
        })
    }
}

pub struct AudioEncoder {
    /// One stereo encoder per pair, plus a mono encoder for FC/LFE in surround.
    encoders: Vec<OpusEncoder>,
    layout: Layout,
    sample_buffer: Vec<f32>,
    seq: u64,
}

impl AudioEncoder {
    pub fn new(layout: Layout, bitrate_kbps: u32) -> Result<Self> {
        // For mono/stereo: one encoder. For 5.1/7.1: 3-4 encoders (paired streams).
        let mut encoders = Vec::new();
        let n = layout.channels();
        let mut remaining = n;
        while remaining >= 2 {
            let mut enc =
                OpusEncoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Audio)?;
            enc.set_bitrate(audiopus::Bitrate::BitsPerSecond(
                ((bitrate_kbps as i32) * 1000) / (n as i32 / 2),
            ))?;
            enc.set_inband_fec(true)?;
            enc.set_packet_loss_perc(5)?;
            encoders.push(enc);
            remaining -= 2;
        }
        if remaining == 1 {
            // odd channel count — won't normally happen but handle gracefully
            let mut enc =
                OpusEncoder::new(SampleRate::Hz48000, Channels::Mono, Application::Audio)?;
            enc.set_bitrate(audiopus::Bitrate::BitsPerSecond(
                (bitrate_kbps as i32) * 1000,
            ))?;
            encoders.push(enc);
        }

        Ok(Self {
            encoders,
            layout,
            sample_buffer: Vec::with_capacity(FRAME_SIZE * 8),
            seq: 0,
        })
    }

    /// Encode interleaved samples (channel order: SMPTE/WAV).
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<AudioFrame>> {
        self.sample_buffer.extend_from_slice(samples);
        let n = self.layout.channels() as usize;
        let frame_samples = FRAME_SIZE * n;
        let mut out = Vec::new();

        while self.sample_buffer.len() >= frame_samples {
            let chunk: Vec<f32> = self.sample_buffer.drain(..frame_samples).collect();

            // For mono / stereo: encode the whole chunk
            if matches!(self.layout, Layout::Mono | Layout::Stereo) {
                let mut buf = vec![0u8; 1500];
                let written = self.encoders[0].encode_float(&chunk, &mut buf)?;
                buf.truncate(written);
                self.seq += 1;
                out.push(self.make_frame(buf));
                continue;
            }

            // Multi-channel: split into stereo pairs, encode each, concatenate
            // with a 2-byte length prefix per pair so the decoder can split them.
            let mut combined: Vec<u8> = Vec::with_capacity(2048);
            for (pair_idx, enc) in self.encoders.iter_mut().enumerate() {
                let pair_start = pair_idx * 2;
                if pair_start + 1 >= n {
                    break;
                } // odd channel — handled separately
                let pair: Vec<f32> = (0..FRAME_SIZE)
                    .flat_map(|s| [chunk[s * n + pair_start], chunk[s * n + pair_start + 1]])
                    .collect();
                let mut buf = vec![0u8; 1500];
                let written = enc.encode_float(&pair, &mut buf)?;
                combined.extend_from_slice(&(written as u16).to_le_bytes());
                combined.extend_from_slice(&buf[..written]);
            }
            self.seq += 1;
            out.push(self.make_frame(combined));
        }
        Ok(out)
    }

    fn make_frame(&self, data: Vec<u8>) -> AudioFrame {
        AudioFrame {
            seq: self.seq,
            codec: AudioCodec::Opus,
            data,
            sample_rate: SAMPLE_RATE as u32,
            channels: self.layout.channels(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

pub struct AudioDecoder {
    decoders: Vec<OpusDecoder>,
    layout: Layout,
}

impl AudioDecoder {
    pub fn new(layout: Layout) -> Result<Self> {
        let mut decoders = Vec::new();
        let n = layout.channels();
        let mut remaining = n;
        while remaining >= 2 {
            decoders.push(OpusDecoder::new(SampleRate::Hz48000, Channels::Stereo)?);
            remaining -= 2;
        }
        if remaining == 1 {
            decoders.push(OpusDecoder::new(SampleRate::Hz48000, Channels::Mono)?);
        }
        Ok(Self { decoders, layout })
    }

    pub fn decode(&mut self, frame: &AudioFrame) -> Result<Vec<f32>> {
        let n = self.layout.channels() as usize;

        if matches!(self.layout, Layout::Mono | Layout::Stereo) {
            let mut out = vec![0.0f32; FRAME_SIZE * n];
            let packet = Packet::try_from(&frame.data[..])
                .map_err(|e| anyhow::anyhow!("opus packet: {e:?}"))?;
            let signals = MutSignals::try_from(&mut out[..])
                .map_err(|e| anyhow::anyhow!("opus signals: {e:?}"))?;
            let written = self.decoders[0].decode_float(Some(packet), signals, false)?;
            out.truncate(written * n);
            return Ok(out);
        }

        // Multi-channel: split + decode each pair
        let mut decoded_pairs: Vec<Vec<f32>> = Vec::new();
        let mut cur = 0;
        for dec in self.decoders.iter_mut() {
            if cur + 2 > frame.data.len() {
                break;
            }
            let len = u16::from_le_bytes([frame.data[cur], frame.data[cur + 1]]) as usize;
            cur += 2;
            if cur + len > frame.data.len() {
                bail!("malformed multi-channel audio");
            }
            let pair_data = &frame.data[cur..cur + len];
            cur += len;
            let mut pair_out = vec![0.0f32; FRAME_SIZE * 2];
            let packet =
                Packet::try_from(pair_data).map_err(|e| anyhow::anyhow!("opus packet: {e:?}"))?;
            let signals = MutSignals::try_from(&mut pair_out[..])
                .map_err(|e| anyhow::anyhow!("opus signals: {e:?}"))?;
            let _ = dec.decode_float(Some(packet), signals, false)?;
            decoded_pairs.push(pair_out);
        }

        // Re-interleave pairs back to channel order
        let mut out = vec![0.0f32; FRAME_SIZE * n];
        for s in 0..FRAME_SIZE {
            for (pair_idx, pair) in decoded_pairs.iter().enumerate() {
                let dst_l = s * n + pair_idx * 2;
                let dst_r = dst_l + 1;
                if dst_r < out.len() {
                    out[dst_l] = pair[s * 2];
                    out[dst_r] = pair[s * 2 + 1];
                }
            }
        }
        Ok(out)
    }
}

pub mod capture;
pub mod playback;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_channels() {
        assert_eq!(Layout::Mono.channels(), 1);
        assert_eq!(Layout::Stereo.channels(), 2);
        assert_eq!(Layout::Surround51.channels(), 6);
        assert_eq!(Layout::Surround71.channels(), 8);
    }

    #[test]
    fn from_channels_roundtrip() {
        for &ch in &[1u8, 2, 6, 8] {
            assert_eq!(Layout::from_channels(ch).unwrap().channels(), ch);
        }
        assert!(Layout::from_channels(3).is_err());
    }

    #[test]
    fn stereo_encode_decode_roundtrip() {
        let mut enc = AudioEncoder::new(Layout::Stereo, 64).unwrap();
        let mut dec = AudioDecoder::new(Layout::Stereo).unwrap();
        // Generate 20ms of silence
        let samples = vec![0.0f32; FRAME_SIZE * 2];
        let frames = enc.encode(&samples).unwrap();
        assert_eq!(frames.len(), 1);
        let out = dec.decode(&frames[0]).unwrap();
        assert_eq!(out.len(), FRAME_SIZE * 2);
    }
}

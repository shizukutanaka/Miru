//! Raw captured frame — designed for zero-copy handoff to the encoder.

use bytes::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra32,
    Nv12, // YUV 4:2:0 planar (GPU-native on Windows/DXGI)
    I420, // YUV 4:2:0 planar (common codec input)
    Rgba32,
}

/// A single captured display frame.
pub struct RawFrame {
    pub display_idx: u8,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Bytes, // Arc-counted; cheap clone
    pub timestamp_ms: u64,
    pub dirty_rects: Vec<DirtyRect>, // Changed regions (DXGI provides this)
}

#[derive(Debug, Clone, Copy)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl RawFrame {
    pub fn full_dirty(width: u32, height: u32) -> Vec<DirtyRect> {
        vec![DirtyRect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        }]
    }
}

//! Shared BT.601 (limited-range) fixed-point RGB → YUV conversion math.
//!
//! Both the JPEG decode path (`decoder::rgb_to_i420`) and miru-host's raw
//! capture-frame conversion (`capture_loop::packed32_to_i420`) need the exact
//! same per-pixel formula; factored out here so the two never drift apart.

/// Y (luma), limited range: `((66*R + 129*G + 25*B + 128) >> 8) + 16`, clamped [16, 235].
#[inline]
pub fn bt601_y(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(16, 235) as u8
}

/// (Cb, Cr) chroma, limited range, clamped [16, 240].
#[inline]
pub fn bt601_uv(r: i32, g: i32, b: i32) -> (u8, u8) {
    let cb = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let cr = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (cb.clamp(16, 240) as u8, cr.clamp(16, 240) as u8)
}

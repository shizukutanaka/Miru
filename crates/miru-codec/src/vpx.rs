//! VP9/VP8 encoder and decoder via libvpx-sys.
//! Used as software fallback when AV1 HW is unavailable.

use anyhow::{bail, Result};
use miru_common::message::VideoCodec;
use vpx_sys::*;

use crate::{DecodedFrame, EncodedPacket, EncoderBackend};
use crate::decoder::DecoderBackend;

pub struct VpxEncoder {
    ctx: vpx_codec_ctx_t,
    cfg: vpx_codec_enc_cfg_t,
    pts: i64,
    fps_num: u32,
    fps_den: u32,
    keyframe_requested: bool,
}

unsafe impl Send for VpxEncoder {}

impl VpxEncoder {
    pub fn new(
        codec: &VideoCodec,
        width: u32,
        height: u32,
        fps: u8,
        bitrate_kbps: u32,
    ) -> Result<Self> {
        unsafe {
            let iface = match codec {
                VideoCodec::Vp9 => vpx_codec_vp9_cx(),
                VideoCodec::Vp8 => vpx_codec_vp8_cx(),
                _ => bail!("VpxEncoder: unsupported codec {:?}", codec),
            };

            let mut cfg = std::mem::zeroed::<vpx_codec_enc_cfg_t>();
            let rc = vpx_codec_enc_config_default(iface, &mut cfg, 0);
            if rc != vpx_codec_err_t::VPX_CODEC_OK {
                bail!("vpx_codec_enc_config_default failed: {:?}", rc);
            }

            cfg.g_w = width;
            cfg.g_h = height;
            cfg.g_timebase.num = 1;
            cfg.g_timebase.den = 1_000_000; // microsecond timebase
            cfg.rc_target_bitrate = bitrate_kbps;
            cfg.rc_end_usage = vpx_rc_mode::VPX_CBR;
            cfg.g_threads = num_cpus() as u32;
            cfg.kf_max_dist = (fps as u32) * 10; // keyframe every 10s max
            cfg.kf_mode = vpx_kf_mode::VPX_KF_AUTO;
            cfg.g_lag_in_frames = 0; // real-time (no lookahead)

            // VP9 specific: real-time quality preset
            let mut ctx = std::mem::zeroed::<vpx_codec_ctx_t>();
            let rc =
                vpx_codec_enc_init_ver(&mut ctx, iface, &cfg, 0, VPX_ENCODER_ABI_VERSION as i32);
            if rc != vpx_codec_err_t::VPX_CODEC_OK {
                bail!("vpx_codec_enc_init failed: {:?}", rc);
            }

            // Real-time encoding settings
            if matches!(codec, VideoCodec::Vp9) {
                set_ctrl(&mut ctx, VP9E_SET_LOSSLESS, 0)?;
                set_ctrl(&mut ctx, VP8E_SET_CPUUSED, 8)?; // 8 = fastest
                set_ctrl(&mut ctx, VP9E_SET_TILE_COLUMNS, 4)?;
                set_ctrl(&mut ctx, VP9E_SET_FRAME_PARALLEL_DECODING, 1)?;
                set_ctrl(&mut ctx, VP9E_SET_AQ_MODE, 3)?; // cyclic refresh
                // Tune for screen content (VPX_CONTENT_SCREEN = 1 in the
                // vpx_tune_content enum). Miru is a screen-sharing tool, so
                // frames are dominated by text, sharp edges, and large flat
                // regions — exactly what libvpx's screen-content mode targets
                // (it biases toward palette/intra-block-copy-style decisions).
                // See docs/RESEARCH_NOTES.md §1.
                set_ctrl(&mut ctx, VP9E_SET_TUNE_CONTENT, 1)?;
            }

            Ok(Self {
                ctx,
                cfg,
                pts: 0,
                fps_num: 1,
                fps_den: fps as u32,
                keyframe_requested: false,
            })
        }
    }
}

impl EncoderBackend for VpxEncoder {
    fn encode(
        &mut self,
        i420: &[u8],
        width: u32,
        height: u32,
        ts_ms: u64,
        keyframe: bool,
    ) -> Result<Option<EncodedPacket>> {
        unsafe {
            let pts = ts_ms.saturating_mul(1000) as i64; // ms → microseconds
            let duration = (1_000_000 / self.fps_den.max(1)) as u64;

            let flags = if keyframe || self.keyframe_requested {
                self.keyframe_requested = false;
                VPX_EFLAG_FORCE_KF as u32
            } else {
                0
            };

            // Build vpx_image
            let mut img = std::mem::zeroed::<vpx_image_t>();

            // Guard against u32 overflow in dimension arithmetic and
            // unreasonable allocations before any unsafe pointer work.
            const MAX_FRAME_DIM: u32 = 32768;
            if width > MAX_FRAME_DIM || height > MAX_FRAME_DIM {
                bail!("VpxEncoder: frame dimensions {}×{} exceed limit {}", width, height, MAX_FRAME_DIM);
            }
            let y_size = (width as usize) * (height as usize);
            let uv_size = (width as usize / 2) * (height as usize / 2);

            if i420.len() < y_size + 2 * uv_size {
                bail!(
                    "VpxEncoder: I420 buffer too small ({} < {})",
                    i420.len(),
                    y_size + 2 * uv_size
                );
            }

            img.fmt = vpx_img_fmt_t::VPX_IMG_FMT_I420;
            img.w = width;
            img.h = height;
            img.d_w = width;
            img.d_h = height;

            let y_ptr = i420.as_ptr() as *mut u8;
            let u_ptr = i420[y_size..].as_ptr() as *mut u8;
            let v_ptr = i420[y_size + uv_size..].as_ptr() as *mut u8;

            img.planes[VPX_PLANE_Y as usize] = y_ptr;
            img.planes[VPX_PLANE_U as usize] = u_ptr;
            img.planes[VPX_PLANE_V as usize] = v_ptr;
            img.stride[VPX_PLANE_Y as usize] = width as i32;
            img.stride[VPX_PLANE_U as usize] = (width / 2) as i32;
            img.stride[VPX_PLANE_V as usize] = (width / 2) as i32;

            let rc = vpx_codec_encode(
                &mut self.ctx,
                &img,
                pts,
                duration,
                flags as i64,
                VPX_DL_REALTIME as u64,
            );
            if rc != vpx_codec_err_t::VPX_CODEC_OK {
                bail!("vpx_codec_encode: {:?}", rc);
            }

            // Collect output packets
            let mut iter = std::ptr::null();
            while let Some(pkt) = {
                let p = vpx_codec_get_cx_data(&mut self.ctx, &mut iter);
                if p.is_null() {
                    None
                } else {
                    Some(&*p)
                }
            } {
                if pkt.kind == vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                    let data = std::slice::from_raw_parts(
                        pkt.data.frame.buf as *const u8,
                        pkt.data.frame.sz,
                    );
                    let is_key = pkt.data.frame.flags & VPX_FRAME_IS_KEY != 0;
                    return Ok(Some(EncodedPacket {
                        keyframe: is_key,
                        data: data.to_vec(),
                        timestamp_ms: ts_ms,
                        duration_ms: (1000 / self.fps_den.max(1)) as u32,
                    }));
                }
            }
            Ok(None)
        }
    }

    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    fn update_bitrate(&mut self, kbps: u32) {
        // VP8E_SET_SCREEN_CONTENT_MODE controls screen-content mode (0/1/2) — not
        // the bitrate. The correct path is to update cfg.rc_target_bitrate and
        // re-apply the encoder config via vpx_codec_enc_config_set.
        unsafe {
            self.cfg.rc_target_bitrate = kbps;
            let rc = vpx_codec_enc_config_set(&mut self.ctx, &self.cfg);
            if rc != vpx_codec_err_t::VPX_CODEC_OK {
                tracing::warn!("vpx_codec_enc_config_set failed when updating bitrate to {kbps} kbps: {rc:?}");
            }
        }
    }
}

impl Drop for VpxEncoder {
    fn drop(&mut self) {
        unsafe {
            vpx_codec_destroy(&mut self.ctx);
        }
    }
}

unsafe fn set_ctrl(ctx: *mut vpx_codec_ctx_t, id: i32, val: i32) -> Result<()> {
    let rc = vpx_codec_control_(ctx, id, val);
    if rc != vpx_codec_err_t::VPX_CODEC_OK {
        bail!("vpx_codec_control({}, {}): {:?}", id, val, rc);
    }
    Ok(())
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

// ─── VP9/VP8 Decoder ─────────────────────────────────────────────────────────

pub struct VpxDecoder {
    ctx: vpx_codec_ctx_t,
}

unsafe impl Send for VpxDecoder {}

impl VpxDecoder {
    pub fn new(codec: VideoCodec) -> Result<Self> {
        unsafe {
            let iface = match codec {
                VideoCodec::Vp9 => vpx_codec_vp9_dx(),
                VideoCodec::Vp8 => vpx_codec_vp8_dx(),
                _ => bail!("VpxDecoder: unsupported codec {:?}", codec),
            };

            let mut ctx = std::mem::zeroed::<vpx_codec_ctx_t>();
            let rc = vpx_codec_dec_init_ver(
                &mut ctx,
                iface,
                std::ptr::null(),
                0,
                VPX_DECODER_ABI_VERSION as i32,
            );
            if rc != vpx_codec_err_t::VPX_CODEC_OK {
                bail!("vpx_codec_dec_init failed: {:?}", rc);
            }
            Ok(Self { ctx })
        }
    }
}

impl DecoderBackend for VpxDecoder {
    fn decode(&mut self, data: &[u8], ts_ms: u64) -> Result<Option<DecodedFrame>> {
        unsafe {
            // Decode the compressed frame.
            let rc = vpx_codec_decode(
                &mut self.ctx,
                data.as_ptr(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
            );
            if rc != vpx_codec_err_t::VPX_CODEC_OK {
                bail!("vpx_codec_decode: {:?}", rc);
            }

            // Retrieve the first decoded image.
            let mut iter = std::ptr::null();
            let img_ptr = vpx_codec_get_frame(&mut self.ctx, &mut iter);
            if img_ptr.is_null() {
                return Ok(None);
            }
            let img = &*img_ptr;

            if img.fmt != vpx_img_fmt_t::VPX_IMG_FMT_I420 {
                bail!("VpxDecoder: unexpected pixel format {:?}", img.fmt);
            }

            let w = img.d_w as usize;
            let h = img.d_h as usize;
            if w == 0 || h == 0 {
                bail!("VpxDecoder: zero-dimension frame {}×{}", w, h);
            }

            let y_ptr = img.planes[VPX_PLANE_Y as usize];
            let u_ptr = img.planes[VPX_PLANE_U as usize];
            let v_ptr = img.planes[VPX_PLANE_V as usize];
            if y_ptr.is_null() || u_ptr.is_null() || v_ptr.is_null() {
                bail!("VpxDecoder: null plane pointer in decoded image");
            }

            let y_stride = img.stride[VPX_PLANE_Y as usize] as usize;
            let u_stride = img.stride[VPX_PLANE_U as usize] as usize;
            let v_stride = img.stride[VPX_PLANE_V as usize] as usize;

            // Chroma half-dimensions (rounded down as per I420 spec).
            let cw = w / 2;
            let ch = h / 2;

            // Copy row-by-row to strip stride padding so callers receive
            // tightly-packed I420 planes (w*h, cw*ch, cw*ch bytes).
            let mut y_plane = vec![0u8; w * h];
            let mut u_plane = vec![0u8; cw * ch];
            let mut v_plane = vec![0u8; cw * ch];

            for row in 0..h {
                let src = std::slice::from_raw_parts(y_ptr.add(row * y_stride), w);
                y_plane[row * w..(row + 1) * w].copy_from_slice(src);
            }
            for row in 0..ch {
                let src = std::slice::from_raw_parts(u_ptr.add(row * u_stride), cw);
                u_plane[row * cw..(row + 1) * cw].copy_from_slice(src);
            }
            for row in 0..ch {
                let src = std::slice::from_raw_parts(v_ptr.add(row * v_stride), cw);
                v_plane[row * cw..(row + 1) * cw].copy_from_slice(src);
            }

            Ok(Some(DecodedFrame {
                width: w as u32,
                height: h as u32,
                y_plane,
                u_plane,
                v_plane,
                timestamp_ms: ts_ms,
            }))
        }
    }
}

impl Drop for VpxDecoder {
    fn drop(&mut self) {
        unsafe {
            vpx_codec_destroy(&mut self.ctx);
        }
    }
}

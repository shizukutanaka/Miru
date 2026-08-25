//! Safe RAII wrapper over the libavcodec C shim (`csrc/miru_ffmpeg.c`).
//!
//! This is the only place in miru-codec that contains `unsafe`, per the
//! workspace rule that unsafe lives under `platform/`. Everything above it
//! sees a safe Rust type.
//!
//! There is no Rust dependency behind this: the shim is compiled by build.rs
//! and linked against the system libavcodec. See the shim's header comment for
//! why a hand-written C surface beats a binding crate here.

use std::ffi::{c_char, c_int, c_uint, c_void, CString};

#[link(name = "avcodec")]
#[link(name = "avutil")]
extern "C" {
    fn miru_enc_available(name: *const c_char) -> c_int;
    fn miru_enc_version() -> c_uint;
    fn miru_enc_open(
        name: *const c_char,
        w: c_int,
        h: c_int,
        fps: c_int,
        bitrate_bps: i64,
        gop: c_int,
    ) -> *mut c_void;
    fn miru_enc_send(e: *mut c_void, i420: *const u8, len: usize, keyframe: c_int) -> c_int;
    fn miru_enc_recv_begin(e: *mut c_void, size: *mut c_int, is_key: *mut c_int) -> c_int;
    fn miru_enc_recv_copy(e: *mut c_void, out: *mut u8, cap: c_int) -> c_int;
    fn miru_enc_set_bitrate(e: *mut c_void, bitrate_bps: i64);
    fn miru_enc_close(e: *mut c_void);
}

/// Version of the libavcodec actually linked, as (major, minor, micro).
pub fn avcodec_version() -> (u32, u32, u32) {
    let v = unsafe { miru_enc_version() };
    (v >> 16, (v >> 8) & 0xff, v & 0xff)
}

/// Is this encoder compiled into the linked libavcodec?
///
/// A `true` here does not mean the encoder will open — `h264_nvenc` is present
/// in most distro builds whether or not an NVIDIA card is. Opening it is the
/// only real capability test.
pub fn encoder_compiled_in(name: &str) -> bool {
    let Ok(c) = CString::new(name) else {
        return false;
    };
    unsafe { miru_enc_available(c.as_ptr()) == 1 }
}

/// An open libavcodec encoder. Freed on drop.
pub struct FfmpegEncoder {
    handle: *mut c_void,
    /// Reused across frames so the hot path does not allocate.
    scratch: Vec<u8>,
}

// The handle is owned exclusively by this struct and libavcodec contexts are
// not shared between threads here; the encode loop owns one and moves it.
unsafe impl Send for FfmpegEncoder {}

/// One encoded packet.
pub struct Packet {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

impl FfmpegEncoder {
    /// Open `name` (an ffmpeg encoder name such as `h264_nvenc`). Returns None
    /// if the encoder is absent or refuses to open — which is the normal signal
    /// that the hardware is not there.
    pub fn open(name: &str, width: u32, height: u32, fps: u8, bitrate_kbps: u32) -> Option<Self> {
        // Even dimensions are required for YUV420P chroma subsampling.
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 || fps == 0 {
            return None;
        }
        let c = CString::new(name).ok()?;
        let handle = unsafe {
            miru_enc_open(
                c.as_ptr(),
                width as c_int,
                height as c_int,
                fps as c_int,
                i64::from(bitrate_kbps) * 1000,
                // 10-second GOP: long enough not to waste bitrate, short enough
                // that a viewer joining late is not stuck on a grey screen.
                c_int::from(fps) * 10,
            )
        };
        if handle.is_null() {
            return None;
        }
        Some(Self { handle, scratch: Vec::new() })
    }

    /// Submit one I420 frame (Y, then U, then V, tightly packed).
    pub fn send(&mut self, i420: &[u8], keyframe: bool) -> Result<(), i32> {
        let r = unsafe {
            miru_enc_send(self.handle, i420.as_ptr(), i420.len(), c_int::from(keyframe))
        };
        if r < 0 {
            return Err(r);
        }
        Ok(())
    }

    /// Take the next encoded packet, if one is ready.
    pub fn receive(&mut self) -> Result<Option<Packet>, i32> {
        let (mut size, mut is_key) = (0 as c_int, 0 as c_int);
        let r = unsafe { miru_enc_recv_begin(self.handle, &mut size, &mut is_key) };
        if r == 0 {
            return Ok(None);
        }
        if r < 0 {
            return Err(r);
        }
        // _begin reported the exact size, so this cannot truncate.
        let n = size as usize;
        self.scratch.clear();
        self.scratch.resize(n, 0);
        let copied = unsafe { miru_enc_recv_copy(self.handle, self.scratch.as_mut_ptr(), size) };
        if copied < 0 {
            return Err(copied);
        }
        Ok(Some(Packet {
            data: self.scratch[..copied as usize].to_vec(),
            keyframe: is_key == 1,
        }))
    }

    pub fn set_bitrate(&mut self, bitrate_kbps: u32) {
        unsafe { miru_enc_set_bitrate(self.handle, i64::from(bitrate_kbps) * 1000) };
    }
}

impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        unsafe { miru_enc_close(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize, t: usize) -> Vec<u8> {
        let mut b = vec![0u8; w * h + 2 * (w / 2) * (h / 2)];
        for y in 0..h {
            for x in 0..w {
                b[y * w + x] = ((x + y + t * 7) % 256) as u8;
            }
        }
        b[w * h..].fill(128);
        b
    }

    #[test]
    fn links_against_a_real_libavcodec() {
        let (major, _, _) = avcodec_version();
        assert!(major >= 58, "unexpectedly old libavcodec: {major}");
    }

    #[test]
    fn reports_absent_encoders_as_absent() {
        assert!(!encoder_compiled_in("definitely_not_a_codec"));
        // Interior NUL must not panic.
        assert!(!encoder_compiled_in("bad\0name"));
    }

    #[test]
    fn rejects_odd_and_zero_dimensions() {
        assert!(FfmpegEncoder::open("mpeg4", 321, 240, 30, 800).is_none());
        assert!(FfmpegEncoder::open("mpeg4", 320, 0, 30, 800).is_none());
        assert!(FfmpegEncoder::open("mpeg4", 320, 240, 0, 800).is_none());
    }

    /// The full submit/drain loop against a software encoder. Hardware encoders
    /// take the identical path — only the name differs — so this exercises the
    /// plumbing that would otherwise only ever run on a GPU box.
    #[test]
    fn encodes_frames_end_to_end() {
        let (w, h) = (320usize, 240usize);
        let mut enc = FfmpegEncoder::open("mpeg4", w as u32, h as u32, 30, 800)
            .expect("mpeg4 is built into every libavcodec");

        let (mut packets, mut bytes, mut keyframes) = (0, 0usize, 0);
        for f in 0..10 {
            enc.send(&gradient(w, h, f), f == 0).expect("send");
            while let Some(p) = enc.receive().expect("receive") {
                packets += 1;
                bytes += p.data.len();
                if p.keyframe {
                    keyframes += 1;
                }
            }
        }
        assert!(packets > 0, "encoder produced no packets");
        assert!(bytes > 0, "encoder produced empty packets");
        assert!(keyframes > 0, "no keyframe in a 10-frame GOP");
    }

    #[test]
    fn rejects_a_short_frame_instead_of_reading_past_it() {
        let mut enc = FfmpegEncoder::open("mpeg4", 320, 240, 30, 800).unwrap();
        assert!(enc.send(&[0u8; 16], true).is_err());
    }

    /// The capability rule the codec advertisement depends on: an encoder being
    /// compiled into libavcodec does not mean it can open. Distro builds ship
    /// h264_nvenc whether or not the machine has an NVIDIA card, so presence and
    /// openability must be allowed to disagree — and only openability may be
    /// advertised to a peer.
    #[test]
    fn compiled_in_does_not_imply_openable() {
        for name in ["h264_nvenc", "av1_nvenc", "h264_vaapi", "hevc_vaapi"] {
            if encoder_compiled_in(name) && FfmpegEncoder::open(name, 640, 480, 30, 2000).is_none()
            {
                // Exactly the case that must not be advertised. Reaching here on
                // a GPU-less machine is the expected outcome, not a failure.
                return;
            }
        }
        // On a box with working hardware every candidate opens, which is also
        // fine — the assertion is that the two notions are checked separately,
        // and a software encoder proves openability is really being tested.
        assert!(FfmpegEncoder::open("mpeg4", 640, 480, 30, 2000).is_some());
    }

    #[test]
    fn set_bitrate_is_safe_to_call_on_an_open_encoder() {
        let mut enc = FfmpegEncoder::open("mpeg4", 320, 240, 30, 800).unwrap();
        enc.set_bitrate(2000);
        enc.send(&gradient(320, 240, 0), true).expect("send");
    }
}

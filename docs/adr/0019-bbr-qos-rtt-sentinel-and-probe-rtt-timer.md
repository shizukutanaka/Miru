# ADR 0019 — BbrQos: Phantom FPS Throttle + ProbeRTT Never Entered + VPX Bitrate No-Op

**Date:** 2026-06-13  
**Status:** Fixed  
**Deciders:** Claude Socratic Review

---

## Context

`BbrQos` and `VpxEncoder::update_bitrate` contained three related bugs that
collectively caused the BBR congestion controller to behave incorrectly from
the first frame of every session.

---

## Bug 1 — Phantom FPS throttle at session start

`BbrQos::rtt_min_us` is initialized to `u32::MAX`. The FPS adjustment in
`tick()` branches on `self.rtt_min_us > 100_000` (100 ms threshold). Because
`u32::MAX > 100_000` is always true, every tick before the first real RTT
sample reduced `cur_fps` by 5. At `ADJUST_INTERVAL = 200 ms`, a session
starting at 60 fps silently degraded to the floor (15 fps) within ~1.5
seconds — before any network data had arrived.

**Fix:** Add a guard: `if self.rtt_min_us == u32::MAX { keep cur_fps }` before
applying the high-RTT penalty.

---

## Bug 2 — ProbeRTT phase never entered

BBR enters `ProbeRTT` to periodically drain the queue and refresh `rtt_min`.
The trigger condition in `advance_phase` was:

```rust
let window_stale = match (self.rtt_samples.front(), self.rtt_samples.back()) {
    (Some((first, _)), Some((last, _))) => last.duration_since(*first) > RTT_WINDOW,
    _ => true,
};
```

`on_rtt()` already evicts samples older than `RTT_WINDOW` (10 s). So when
samples exist, the span from front→back is always ≤ `RTT_WINDOW`, meaning
`window_stale` is effectively always `false`. ProbeRTT was never entered and
`rtt_min_us` was therefore anchored to the first few seconds of the session,
causing the bitrate and FPS to be calibrated to stale network conditions for
the entire session.

**Fix:** Replace with a wall-clock timer (`last_probe_rtt: Instant`). ProbeRTT
is triggered when `now - last_probe_rtt > PROBE_RTT_INTERVAL` (10 s), matching
the BBR specification. The timer is reset each time ProbeRTT is exited.

---

## Bug 3 — VpxEncoder::update_bitrate was a no-op

`BbrQos` calls `update_bitrate(kbps)` on the encoder when bandwidth changes.
`VpxEncoder::update_bitrate` was:

```rust
fn update_bitrate(&mut self, kbps: u32) {
    unsafe {
        set_ctrl(&mut self.ctx, VP8E_SET_SCREEN_CONTENT_MODE, kbps as i32).ok();
    }
}
```

`VP8E_SET_SCREEN_CONTENT_MODE` controls screen-content coding mode (0 = off,
1 = on, 2 = auto). Passing an arbitrary `kbps` value (e.g. 5000) set the
encoder to an undefined screen-content mode and did **not** change the target
bitrate. The BBR controller believed it was adapting encoder bandwidth but the
VP9 encoder was running at its initial bitrate for the entire session.

**Fix:** Store the `vpx_codec_enc_cfg_t` in the encoder struct. In
`update_bitrate`, update `cfg.rc_target_bitrate = kbps` and call
`vpx_codec_enc_config_set(&mut self.ctx, &self.cfg)` — the standard libvpx
live reconfiguration path.

---

## Consequences

- Sessions no longer start with a phantom FPS penalty.
- `rtt_min_us` is periodically refreshed; bitrate tracks actual network
  conditions rather than the state at connection establishment.
- VP9 encoder bitrate now responds to BBR bandwidth estimates in real time.
- Three regression tests added to `qos_bbr.rs`.

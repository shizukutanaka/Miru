/**
 * Normalize `WheelEvent` deltas to a consistent "notch" unit across browsers.
 *
 * The viewer used to send `deltaY / 100` unconditionally, assuming pixel-mode
 * deltas. But `WheelEvent.deltaMode` varies by browser and platform — MDN and
 * the W3C UI Events spec both warn "do not assume the values are in pixels; you
 * must check deltaMode". In line mode (DOM_DELTA_LINE, common on Firefox and on
 * WebKitGTK — which is exactly the Tauri webview on Linux) a wheel notch is
 * `deltaY ≈ 3` *lines*, so `/100` produced ~0.03 and scrolling barely moved.
 *
 * We convert to pixels first (the normalize-wheel convention: 40px per line,
 * 800px per page), then to notches at ~100px per notch. Pixel-mode input is
 * divided by 100 exactly as before, so the common Chromium path is unchanged —
 * this only rescues line/page mode.
 *
 * Output is in the same sign convention as the raw deltas; the caller applies
 * the scroll-direction negation, as it always has.
 *
 * The host treats the magnitude loosely (Windows ×120, macOS ×3, Linux uses the
 * sign only), so exact px constants are not load-bearing — what matters is that
 * a notch maps to ~1.0 in every mode instead of collapsing to zero.
 */

/** DOM_DELTA_LINE line height, per the normalize-wheel convention. */
const PIXELS_PER_LINE = 40;
/** DOM_DELTA_PAGE page height (approximate; page mode is rare). */
const PIXELS_PER_PAGE = 800;
/** Pixels in one wheel notch — keeps pixel mode identical to the old `/100`. */
const PIXELS_PER_NOTCH = 100;

function toPixels(delta: number, deltaMode: number): number {
  if (deltaMode === 1) return delta * PIXELS_PER_LINE; // DOM_DELTA_LINE
  if (deltaMode === 2) return delta * PIXELS_PER_PAGE; // DOM_DELTA_PAGE
  return delta; // DOM_DELTA_PIXEL (0) or anything unexpected
}

export interface WheelNotches {
  dx: number;
  dy: number;
}

/** Convert raw wheel deltas + deltaMode to notch units (sign preserved). */
export function normalizeWheel(
  deltaX: number,
  deltaY: number,
  deltaMode: number,
): WheelNotches {
  return {
    dx: toPixels(deltaX, deltaMode) / PIXELS_PER_NOTCH,
    dy: toPixels(deltaY, deltaMode) / PIXELS_PER_NOTCH,
  };
}

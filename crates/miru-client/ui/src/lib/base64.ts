/**
 * Base64 → bytes for the video hot path.
 *
 * Every decoded frame arrives over Tauri IPC as base64 (the event payload is
 * JSON), so this runs 30–60 times a second on the main thread: a 1080p JPEG is
 * ~270 KB, meaning ~270k iterations per frame with the hand-rolled loop, all
 * competing with rendering.
 *
 * `Uint8Array.fromBase64` (TC39 arraybuffer-base64, Stage 4 — in the standard)
 * does the same work in native code. Support varies across the webviews Tauri
 * embeds — WebKitGTK on Linux, WKWebView on macOS, WebView2 on Windows — so it
 * is feature-detected once and the loop stays as the fallback.
 *
 * Both paths are exported so tests can assert they agree byte for byte.
 *
 * The return type pins the backing buffer to `ArrayBuffer` rather than the
 * default `ArrayBufferLike`: a possibly-shared view is not a valid `BlobPart`,
 * and the JPEG path wraps the result in a `Blob`.
 *
 * Note the host always emits canonical padded standard-alphabet base64 (Rust's
 * `base64::engine::general_purpose::STANDARD`), so the two decoders cannot
 * diverge on padding or alphabet edge cases in practice. Neither swallows
 * malformed input: both throw, as `atob` did before, and the callers already
 * treat a throw as "corrupt frame, skip it".
 */

type Decoder = (b64: string) => Uint8Array<ArrayBuffer>;

/** Detected once at module load rather than per frame. */
const nativeDecoder: Decoder | null = (() => {
  const ctor = Uint8Array as unknown as {
    fromBase64?: (s: string) => Uint8Array<ArrayBuffer>;
  };
  return typeof ctor.fromBase64 === "function"
    ? (b64: string) => ctor.fromBase64!(b64)
    : null;
})();

/** True when the runtime provides the standardized native decoder. */
export const hasNativeBase64 = nativeDecoder !== null;

/** Portable fallback: `atob` plus a char-code copy. */
export function decodeBase64Loop(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Decode base64 to bytes, preferring the native decoder when present. */
export function base64ToBytes(b64: string): Uint8Array<ArrayBuffer> {
  return nativeDecoder ? nativeDecoder(b64) : decodeBase64Loop(b64);
}

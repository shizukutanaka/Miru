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
type Encoder = (bytes: Uint8Array) => string;

/** Detected once at module load rather than per frame. */
const nativeDecoder: Decoder | null = (() => {
  const ctor = Uint8Array as unknown as {
    fromBase64?: (s: string) => Uint8Array<ArrayBuffer>;
  };
  return typeof ctor.fromBase64 === "function"
    ? (b64: string) => ctor.fromBase64!(b64)
    : null;
})();

/** Native encoder from the same proposal, if present. */
const nativeEncoder: Encoder | null = (() => {
  const proto = Uint8Array.prototype as unknown as { toBase64?: () => string };
  return typeof proto.toBase64 === "function"
    ? (bytes: Uint8Array) => (bytes as unknown as { toBase64: () => string }).toBase64()
    : null;
})();

/** True when the runtime provides the standardized native decoder. */
export const hasNativeBase64 = nativeDecoder !== null;

/** True when the runtime provides the standardized native encoder. */
export const hasNativeBase64Encode = nativeEncoder !== null;

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

/**
 * Chunk size for the fallback encoder. `String.fromCharCode` is variadic, and
 * spreading a whole multi-megabyte array would blow the argument limit, so the
 * bytes are walked in blocks.
 */
const ENCODE_CHUNK = 8192;

/**
 * Portable fallback encoder.
 *
 * The previous inline version appended one character at a time
 * (`binary += String.fromCharCode(bytes[i])`), which for the 100 MB the file
 * picker allows meant 100 million iterations synchronously on the main thread —
 * the window simply froze. Chunking cuts that to a few thousand calls.
 */
export function encodeBase64Loop(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += ENCODE_CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + ENCODE_CHUNK));
  }
  return btoa(binary);
}

/**
 * Encode bytes to base64, preferring the native encoder.
 *
 * Note this still materialises the whole base64 string, because the
 * `send_file` command takes the payload in one shot. Removing that peak
 * memory needs a chunked IPC command on the Rust side — out of scope here.
 */
export function bytesToBase64(bytes: Uint8Array): string {
  return nativeEncoder ? nativeEncoder(bytes) : encodeBase64Loop(bytes);
}

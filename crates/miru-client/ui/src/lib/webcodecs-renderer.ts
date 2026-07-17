/**
 * WebCodecs VP9/VP8 renderer.
 *
 * The correct fix for the double-codec path described in ADR 0013: instead of
 * decoding VP9 → I420 → JPEG in Rust and shipping JPEG over IPC, the host
 * forwards the raw VP9/VP8 bitstream (`video-packet` event) and this renderer
 * decodes it in the WebView with a hardware-accelerated `VideoDecoder`, drawing
 * straight to the session canvas. IPC then carries only the compressed stream.
 *
 * On any decoder failure the `onError` callback fires; the caller falls back to
 * the Rust JPEG path (`api.setDecodeMode(false)`), which keeps working.
 */
export interface DecodePacket {
  codec: string;
  keyframe: boolean;
  timestamp_ms: number;
  data_b64: string;
}

/** True if this WebView can construct a VideoDecoder and decode VP9. */
export async function webcodecsVp9Supported(): Promise<boolean> {
  if (typeof VideoDecoder === "undefined") return false;
  try {
    const res = await VideoDecoder.isConfigSupported({ codec: VP9_CODEC });
    return res.supported === true;
  } catch {
    return false;
  }
}

const VP9_CODEC = "vp09.00.10.08"; // profile 0, level 1.0, 8-bit
const VP8_CODEC = "vp8";

export class WebCodecsRenderer {
  private decoder: VideoDecoder | null = null;
  private configuredCodec: string | null = null;
  private gotKey = false;

  constructor(
    private canvas: HTMLCanvasElement,
    private onError: () => void,
    private onResolution: (w: number, h: number) => void,
  ) {}

  /** Feed one encoded packet; decoded frames are drawn to the canvas. */
  push(pkt: DecodePacket): void {
    try {
      this.ensureDecoder(pkt.codec);
      const decoder = this.decoder;
      if (!decoder) return;
      // A decoder can only start from a keyframe — drop deltas until one arrives.
      if (!pkt.keyframe && !this.gotKey) return;
      if (pkt.keyframe) this.gotKey = true;
      // Latency guard: if the decode queue is backing up, drop delta frames
      // (keyframes are always kept so the picture recovers).
      if (!pkt.keyframe && decoder.decodeQueueSize > 4) return;

      const data = b64ToBytes(pkt.data_b64);
      const chunk = new EncodedVideoChunk({
        type: pkt.keyframe ? "key" : "delta",
        timestamp: pkt.timestamp_ms * 1000, // WebCodecs timestamps are µs
        data,
      });
      decoder.decode(chunk);
    } catch {
      this.onError();
    }
  }

  private ensureDecoder(codec: string): void {
    if (this.decoder && this.configuredCodec === codec) return;
    this.reset();

    const decoder = new VideoDecoder({
      output: (frame) => {
        try {
          const c = this.canvas;
          if (c.width !== frame.displayWidth || c.height !== frame.displayHeight) {
            c.width = frame.displayWidth;
            c.height = frame.displayHeight;
            this.onResolution(frame.displayWidth, frame.displayHeight);
          }
          c.getContext("2d")?.drawImage(frame, 0, 0, c.width, c.height);
        } finally {
          frame.close();
        }
      },
      error: () => this.onError(),
    });
    decoder.configure({
      codec: codec === "vp8" ? VP8_CODEC : VP9_CODEC,
      optimizeForLatency: true,
    });
    this.decoder = decoder;
    this.configuredCodec = codec;
    this.gotKey = false;
  }

  /** Tear down the decoder (e.g. before reconfiguring or on fallback). */
  reset(): void {
    if (this.decoder) {
      try {
        if (this.decoder.state !== "closed") this.decoder.close();
      } catch {
        /* already closed / errored */
      }
    }
    this.decoder = null;
    this.configuredCodec = null;
    this.gotKey = false;
  }

  destroy(): void {
    this.reset();
  }
}

/** Decode base64 → Uint8Array (BufferSource for EncodedVideoChunk). */
function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

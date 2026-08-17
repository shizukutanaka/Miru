import { describe, it, expect } from "vitest";
import { base64ToBytes, decodeBase64Loop, hasNativeBase64 } from "./base64";

/** Encode bytes to base64 without depending on the decoder under test. */
function encode(bytes: number[]): string {
  return btoa(String.fromCharCode(...bytes));
}

describe("base64ToBytes", () => {
  it("decodes base64 to the original bytes", () => {
    // "Miru" → TWlydQ==
    expect(Array.from(base64ToBytes("TWlydQ=="))).toEqual([0x4d, 0x69, 0x72, 0x75]);
  });

  it("handles empty input", () => {
    expect(base64ToBytes("").length).toBe(0);
  });

  it("round-trips arbitrary byte values including 0 and 255", () => {
    const bytes = [0, 1, 127, 128, 254, 255];
    expect(Array.from(base64ToBytes(encode(bytes)))).toEqual(bytes);
  });

  it("handles every input length mod 4 (both padding forms)", () => {
    for (const len of [1, 2, 3, 4, 5, 6, 7]) {
      const bytes = Array.from({ length: len }, (_, i) => (i * 37) & 0xff);
      expect(Array.from(base64ToBytes(encode(bytes)))).toEqual(bytes);
    }
  });

  it("decodes a frame-sized payload correctly", () => {
    const bytes = Array.from({ length: 4096 }, (_, i) => i & 0xff);
    const out = base64ToBytes(encode(bytes));
    expect(out.length).toBe(4096);
    expect(Array.from(out)).toEqual(bytes);
  });

  it("throws on malformed input rather than returning garbage", () => {
    // Callers treat a throw as "corrupt frame, skip"; silently returning
    // partial bytes would feed junk to the decoder instead.
    expect(() => base64ToBytes("!!!!")).toThrow();
  });
});

describe("decodeBase64Loop (fallback)", () => {
  it("produces the same bytes as the selected decoder", () => {
    const inputs = ["", "TWlydQ==", encode([0, 255, 128]), encode([1, 2, 3, 4, 5])];
    for (const b64 of inputs) {
      expect(Array.from(decodeBase64Loop(b64))).toEqual(Array.from(base64ToBytes(b64)));
    }
  });

  /**
   * When the runtime has the native decoder, prove the two implementations
   * agree — otherwise a browser with native support could behave differently
   * from one without. Skipped where the API is absent (e.g. Node 22).
   */
  it.skipIf(!hasNativeBase64)("agrees with the native decoder on random data", () => {
    for (let trial = 0; trial < 50; trial++) {
      const len = trial * 3;
      const bytes = Array.from({ length: len }, (_, i) => (i * 91 + trial) & 0xff);
      const b64 = encode(bytes);
      expect(Array.from(base64ToBytes(b64))).toEqual(Array.from(decodeBase64Loop(b64)));
    }
  });
});

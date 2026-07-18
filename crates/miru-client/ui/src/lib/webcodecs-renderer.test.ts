import { describe, it, expect } from "vitest";
import {
  codecStringFor,
  shouldDecode,
  b64ToBytes,
  MAX_QUEUE_BEFORE_DROP,
} from "./webcodecs-renderer";

describe("codecStringFor", () => {
  it("maps vp8 to the WebCodecs vp8 string", () => {
    expect(codecStringFor("vp8")).toBe("vp8");
  });
  it("maps vp9 to a full vp09 codec string", () => {
    expect(codecStringFor("vp9")).toBe("vp09.00.10.08");
  });
  it("defaults unknown tags to vp9 (the primary codec)", () => {
    expect(codecStringFor("something-else")).toBe("vp09.00.10.08");
  });
});

describe("shouldDecode", () => {
  it("always decodes keyframes, even before any keyframe was seen", () => {
    expect(shouldDecode(true, false, 0)).toBe(true);
    expect(shouldDecode(true, false, 999)).toBe(true);
  });

  it("drops delta frames until the first keyframe has arrived", () => {
    expect(shouldDecode(false, false, 0)).toBe(false);
  });

  it("decodes delta frames once a keyframe was seen and the queue is shallow", () => {
    expect(shouldDecode(false, true, 0)).toBe(true);
    expect(shouldDecode(false, true, MAX_QUEUE_BEFORE_DROP)).toBe(true);
  });

  it("drops delta frames when the decode queue is backing up", () => {
    expect(shouldDecode(false, true, MAX_QUEUE_BEFORE_DROP + 1)).toBe(false);
  });
});

describe("b64ToBytes", () => {
  it("decodes base64 to the original bytes", () => {
    // "Miru" → TWlydQ==
    expect(Array.from(b64ToBytes("TWlydQ=="))).toEqual([0x4d, 0x69, 0x72, 0x75]);
  });
  it("handles empty input", () => {
    expect(b64ToBytes("").length).toBe(0);
  });
  it("round-trips arbitrary bytes", () => {
    const bytes = new Uint8Array([0, 1, 127, 128, 255]);
    const b64 = btoa(String.fromCharCode(...bytes));
    expect(Array.from(b64ToBytes(b64))).toEqual(Array.from(bytes));
  });
});

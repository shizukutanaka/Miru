import { describe, it, expect } from "vitest";
import {
  codecStringFor,
  shouldDecode,
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

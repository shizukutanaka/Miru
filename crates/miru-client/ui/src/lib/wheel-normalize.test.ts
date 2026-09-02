import { describe, it, expect } from "vitest";
import { normalizeWheel } from "./wheel-normalize";

const PIXEL = 0;
const LINE = 1;
const PAGE = 2;

describe("normalizeWheel", () => {
  it("keeps pixel mode identical to the previous /100 behavior", () => {
    // A Chromium wheel notch is ~100px → 1.0 notch, exactly as before.
    expect(normalizeWheel(0, 100, PIXEL)).toEqual({ dx: 0, dy: 1 });
    expect(normalizeWheel(50, 0, PIXEL)).toEqual({ dx: 0.5, dy: 0 });
  });

  /**
   * Regression: this is the whole point. A Firefox/WebKitGTK wheel notch is
   * deltaY ≈ 3 lines; the old `/100` gave 0.03 (no scroll). It must now be a
   * usable notch magnitude near 1.
   */
  it("rescues line mode instead of collapsing it to near-zero", () => {
    const { dy } = normalizeWheel(0, 3, LINE);
    expect(dy).toBeCloseTo(1.2, 5); // 3 * 40 / 100
    expect(dy).toBeGreaterThan(0.5);
  });

  it("scales page mode to a large-but-finite scroll", () => {
    expect(normalizeWheel(0, 1, PAGE)).toEqual({ dx: 0, dy: 8 }); // 800 / 100
  });

  it("preserves sign in both axes", () => {
    expect(normalizeWheel(-100, -100, PIXEL)).toEqual({ dx: -1, dy: -1 });
    const up = normalizeWheel(0, -3, LINE);
    expect(up.dy).toBeLessThan(0);
  });

  it("treats an unknown deltaMode as pixels rather than throwing", () => {
    expect(normalizeWheel(0, 100, 99)).toEqual({ dx: 0, dy: 1 });
  });

  it("passes zero through unchanged", () => {
    expect(normalizeWheel(0, 0, LINE)).toEqual({ dx: 0, dy: 0 });
  });
});

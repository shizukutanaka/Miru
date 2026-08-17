import { describe, it, expect } from "vitest";
import { TouchGestures, type TouchAction } from "./touch-gestures";

const at = (x: number, y: number) => ({ x, y });
const types = (a: TouchAction[]) => a.map((x) => x.type);

describe("TouchGestures — single finger", () => {
  it("presses on touchstart and releases on touchend", () => {
    const g = new TouchGestures();
    expect(g.start(1, at(0.2, 0.3))).toEqual([
      { type: "press_left", x: 0.2, y: 0.3 },
    ]);
    expect(g.isLeftDown()).toBe(true);
    expect(g.end()).toEqual([{ type: "release_left", x: 0.2, y: 0.3 }]);
    expect(g.isLeftDown()).toBe(false);
  });

  it("releases at the position the finger last moved to", () => {
    const g = new TouchGestures();
    g.start(1, at(0.1, 0.1));
    g.move(1, at(0.8, 0.9));
    expect(g.end()).toEqual([{ type: "release_left", x: 0.8, y: 0.9 }]);
  });
});

describe("TouchGestures — two fingers", () => {
  /**
   * Regression: a two-finger scroll never presses the left button, but the old
   * handler released on every touchend — and touchend fires per finger, so the
   * host received two releases with no press at all.
   */
  it("emits no release when a two-finger gesture ends", () => {
    const g = new TouchGestures();
    // Both fingers land "simultaneously": the browser reports count 2.
    expect(g.start(2, at(0.5, 0.5))).toEqual([]);
    g.move(2, at(0.5, 0.4));
    expect(g.end()).toEqual([]); // first finger up
    expect(g.end()).toEqual([]); // second finger up
  });

  /**
   * Regression: adding a second finger used to leave the left button pressed
   * for the whole scroll, so the host scrolled with a left-drag active.
   */
  it("releases the left button when a second finger joins", () => {
    const g = new TouchGestures();
    g.start(1, at(0.3, 0.3));
    expect(g.start(2, at(0.4, 0.4))).toEqual([
      { type: "release_left", x: 0.3, y: 0.3 },
    ]);
    expect(g.isLeftDown()).toBe(false);
  });

  it("emits exactly one release across the whole sequence", () => {
    const g = new TouchGestures();
    const all: TouchAction[] = [
      ...g.start(1, at(0.3, 0.3)),
      ...g.start(2, at(0.4, 0.4)),
      ...g.end(),
      ...g.end(),
    ];
    expect(types(all)).toEqual(["press_left", "release_left"]);
  });

  it("ignores movement bookkeeping while two fingers are down", () => {
    const g = new TouchGestures();
    g.start(1, at(0.2, 0.2));
    g.start(2, at(0.9, 0.9));
    g.move(2, at(0.1, 0.1)); // must not become the release position
    g.start(1, at(0.5, 0.5)); // back to one finger → new press
    expect(g.end()).toEqual([{ type: "release_left", x: 0.5, y: 0.5 }]);
  });
});

describe("TouchGestures — robustness", () => {
  it("never releases without a matching press", () => {
    const g = new TouchGestures();
    expect(g.end()).toEqual([]);
    expect(g.end()).toEqual([]);
  });

  it("never sends two presses in a row", () => {
    const g = new TouchGestures();
    g.start(1, at(0.1, 0.1));
    // A stray second touchstart with count 1 (shouldn't happen, but must not
    // leave the host with two downs and one up).
    const second = g.start(1, at(0.6, 0.6));
    expect(types(second)).toEqual(["release_left", "press_left"]);
    expect(types(g.end())).toEqual(["release_left"]);
  });

  it("cancel releases an outstanding press and is idempotent", () => {
    const g = new TouchGestures();
    g.start(1, at(0.4, 0.4));
    expect(types(g.cancel())).toEqual(["release_left"]);
    expect(g.cancel()).toEqual([]);
  });

  it("leaves nothing outstanding after a three-finger sequence", () => {
    const g = new TouchGestures();
    g.start(1, at(0.5, 0.5));
    g.start(2, at(0.5, 0.5));
    g.start(3, at(0.5, 0.5));
    g.end();
    g.end();
    g.end();
    expect(g.isLeftDown()).toBe(false);
  });
});

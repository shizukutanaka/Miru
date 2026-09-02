import { describe, it, expect } from "vitest";
import {
  TouchGestures,
  LONG_PRESS_MS,
  MOVE_SLOP,
  type TouchAction,
} from "./touch-gestures";

const at = (x: number, y: number) => ({ x, y });
const types = (a: TouchAction[]) => a.map((x) => x.type);

describe("TouchGestures — tap", () => {
  it("emits nothing on touchstart; press and release arrive together on lift", () => {
    const g = new TouchGestures();
    expect(g.start(1, at(0.2, 0.3), 0)).toEqual([]);
    expect(g.isPending()).toBe(true);
    expect(g.isLeftDown()).toBe(false);

    expect(g.end(100)).toEqual([
      { type: "press_left", x: 0.2, y: 0.3 },
      { type: "release_left", x: 0.2, y: 0.3 },
    ]);
    expect(g.isPending()).toBe(false);
  });

  it("a jitter smaller than the slop is still a tap", () => {
    const g = new TouchGestures();
    g.start(1, at(0.5, 0.5), 0);
    expect(g.move(1, at(0.5 + MOVE_SLOP / 2, 0.5), 10)).toEqual([]);
    expect(g.isLeftDown()).toBe(false);
    expect(types(g.end(50))).toEqual(["press_left", "release_left"]);
  });
});

describe("TouchGestures — long press", () => {
  it("held still then lifted produces a right click and no left events", () => {
    const g = new TouchGestures();
    g.start(1, at(0.4, 0.6), 1000);
    const out = g.end(1000 + LONG_PRESS_MS);
    expect(out).toEqual([
      { type: "press_right", x: 0.4, y: 0.6 },
      { type: "release_right", x: 0.4, y: 0.6 },
    ]);
    expect(types(out).some((t) => t.endsWith("_left"))).toBe(false);
  });

  it("one millisecond short of the threshold is still a left click", () => {
    const g = new TouchGestures();
    g.start(1, at(0.4, 0.6), 0);
    expect(types(g.end(LONG_PRESS_MS - 1))).toEqual([
      "press_left",
      "release_left",
    ]);
  });

  it("moving cancels the long press even if the finger is down long enough", () => {
    const g = new TouchGestures();
    g.start(1, at(0.1, 0.1), 0);
    g.move(1, at(0.9, 0.9), 10);
    expect(types(g.end(10_000))).toEqual(["release_left"]);
  });
});

describe("TouchGestures — drag", () => {
  it("commits on crossing the slop, stamping the press at the ORIGINAL position", () => {
    const g = new TouchGestures();
    g.start(1, at(0.1, 0.1), 0);
    // The press must land where the finger touched down, not where it moved to,
    // or the drag starts from the wrong place.
    expect(g.move(1, at(0.3, 0.1), 10)).toEqual([
      { type: "press_left", x: 0.1, y: 0.1 },
    ]);
    expect(g.isLeftDown()).toBe(true);
  });

  it("presses exactly once no matter how many moves follow", () => {
    const g = new TouchGestures();
    g.start(1, at(0.1, 0.1), 0);
    const all = [
      ...g.move(1, at(0.3, 0.1), 10),
      ...g.move(1, at(0.5, 0.1), 20),
      ...g.move(1, at(0.7, 0.1), 30),
    ];
    expect(types(all)).toEqual(["press_left"]);
  });

  it("releases at the position the finger last moved to", () => {
    const g = new TouchGestures();
    g.start(1, at(0.1, 0.1), 0);
    g.move(1, at(0.8, 0.9), 10);
    expect(g.end(20)).toEqual([{ type: "release_left", x: 0.8, y: 0.9 }]);
  });
});

describe("TouchGestures — two fingers", () => {
  it("a two-finger scroll emits no button events at all", () => {
    // Regression: the first finger used to press the left button and the second
    // finger's arrival released it, so every scroll began with a stray click.
    const g = new TouchGestures();
    const all = [
      ...g.start(1, at(0.5, 0.5), 0),
      ...g.start(2, at(0.5, 0.5), 5),
      ...g.end(200),
      ...g.end(205),
    ];
    expect(all).toEqual([]);
  });

  it("a second finger during a committed drag releases exactly once", () => {
    const g = new TouchGestures();
    g.start(1, at(0.2, 0.2), 0);
    g.move(1, at(0.6, 0.2), 10);
    expect(g.start(2, at(0.6, 0.2), 20)).toEqual([
      { type: "release_left", x: 0.6, y: 0.2 },
    ]);
    expect(g.end(30)).toEqual([]);
    expect(g.end(35)).toEqual([]);
  });

  it("does not resolve a discarded gesture into a click on lift", () => {
    const g = new TouchGestures();
    g.start(1, at(0.5, 0.5), 0);
    g.start(2, at(0.5, 0.5), 5);
    // Long enough to have been a right click had it not become a scroll.
    expect(g.end(5 + LONG_PRESS_MS * 2)).toEqual([]);
  });
});

describe("TouchGestures — teardown", () => {
  it("cancel drops an unresolved gesture rather than clicking", () => {
    const g = new TouchGestures();
    g.start(1, at(0.5, 0.5), 0);
    expect(g.cancel()).toEqual([]);
    expect(g.isPending()).toBe(false);
  });

  it("cancel releases a committed press, and is idempotent", () => {
    const g = new TouchGestures();
    g.start(1, at(0.2, 0.2), 0);
    g.move(1, at(0.7, 0.2), 10);
    expect(g.cancel()).toEqual([{ type: "release_left", x: 0.7, y: 0.2 }]);
    expect(g.cancel()).toEqual([]);
  });

  it("a new touch never leaves two presses outstanding", () => {
    const g = new TouchGestures();
    g.start(1, at(0.2, 0.2), 0);
    g.move(1, at(0.7, 0.2), 10);
    // A touchstart arriving with a press still held must release it first.
    expect(types(g.start(1, at(0.4, 0.4), 20))).toEqual(["release_left"]);
    expect(g.isLeftDown()).toBe(false);
    expect(g.isPending()).toBe(true);
  });
});

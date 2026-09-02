import { describe, it, expect } from "vitest";
import {
  BLOCKED_CHORDS,
  KeyTracker,
  isBlockedChord,
  modifierBits,
  type KeyEventLike,
  type KeyInputMsg,
} from "./key-tracker";

/** Build a KeyboardEvent-shaped object; defaults to a plain "a" press. */
function kev(over: Partial<KeyEventLike> = {}): KeyEventLike {
  return {
    key: "a",
    code: "KeyA",
    keyCode: 65,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    ...over,
  };
}

function mkTracker() {
  const sent: KeyInputMsg[] = [];
  const tracker = new KeyTracker((m) => sent.push(m));
  return { tracker, sent };
}

describe("isBlockedChord", () => {
  it("blocks the four window-management chords with ctrl", () => {
    for (const letter of ["w", "q", "h", "m"]) {
      expect(isBlockedChord({ key: letter, ctrlKey: true, metaKey: false })).toBe(true);
    }
  });

  it("blocks them with meta instead of ctrl", () => {
    expect(isBlockedChord({ key: "w", ctrlKey: false, metaKey: true })).toBe(true);
  });

  it("is case-insensitive, so Ctrl+Shift+W is still blocked", () => {
    expect(isBlockedChord({ key: "W", ctrlKey: true, metaKey: false })).toBe(true);
  });

  it("does not block the same letters without a modifier", () => {
    expect(isBlockedChord({ key: "w", ctrlKey: false, metaKey: false })).toBe(false);
  });

  it("does not block an unlisted letter like Ctrl+C", () => {
    expect(isBlockedChord({ key: "c", ctrlKey: true, metaKey: false })).toBe(false);
  });

  /** Anti-drift: the filter used to be a hand-written list separate from the picker. */
  it("blocks every chord the toolbar picker offers", () => {
    for (const c of BLOCKED_CHORDS) {
      const letter = c.code.replace(/^Key/, "").toLowerCase();
      expect(isBlockedChord({ key: letter, ctrlKey: true, metaKey: false })).toBe(true);
    }
  });
});

describe("modifierBits", () => {
  it("maps each modifier to its own bit", () => {
    const base = { shiftKey: false, ctrlKey: false, altKey: false, metaKey: false };
    expect(modifierBits({ ...base, shiftKey: true })).toBe(0x01);
    expect(modifierBits({ ...base, ctrlKey: true })).toBe(0x02);
    expect(modifierBits({ ...base, altKey: true })).toBe(0x04);
    expect(modifierBits({ ...base, metaKey: true })).toBe(0x08);
    expect(modifierBits(base)).toBe(0);
    expect(
      modifierBits({ shiftKey: true, ctrlKey: true, altKey: true, metaKey: true }),
    ).toBe(0x0f);
  });
});

describe("KeyTracker.handleKey", () => {
  it("forwards a keydown with code, keyCode and modifiers", () => {
    const { tracker, sent } = mkTracker();
    expect(tracker.handleKey(true, kev({ shiftKey: true }))).toBe(true);
    expect(sent).toEqual([
      { kind: "key_down", code: "KeyA", key: 65, modifiers: 0x01 },
    ]);
  });

  it("forwards the keyup of a held key", () => {
    const { tracker, sent } = mkTracker();
    tracker.handleKey(true, kev());
    tracker.handleKey(false, kev());
    expect(sent.map((m) => m.kind)).toEqual(["key_down", "key_up"]);
    expect(tracker.heldCount()).toBe(0);
  });

  it("sends nothing for a blocked chord and tells the caller not to preventDefault", () => {
    const { tracker, sent } = mkTracker();
    expect(tracker.handleKey(true, kev({ key: "w", code: "KeyW", keyCode: 87, ctrlKey: true })))
      .toBe(false);
    expect(sent).toEqual([]);
  });

  /**
   * Regression: releasing Ctrl before W clears e.ctrlKey, so W's keyup no
   * longer looks like a blocked chord and used to be forwarded — a key_up the
   * host never saw a key_down for.
   */
  it("never emits a key_up for a blocked chord whose modifier was released first", () => {
    const { tracker, sent } = mkTracker();
    const ctrl = { key: "Control", code: "ControlLeft", keyCode: 17 };
    tracker.handleKey(true, kev({ ...ctrl, ctrlKey: true }));
    tracker.handleKey(true, kev({ key: "w", code: "KeyW", keyCode: 87, ctrlKey: true }));
    tracker.handleKey(false, kev({ ...ctrl, ctrlKey: false }));
    // Ctrl is up, so this no longer matches the chord filter.
    const consumed = tracker.handleKey(false, kev({ key: "w", code: "KeyW", keyCode: 87 }));

    expect(consumed).toBe(true);
    expect(sent).toEqual([
      { kind: "key_down", code: "ControlLeft", key: 17, modifiers: 0x02 },
      { kind: "key_up", code: "ControlLeft", key: 17, modifiers: 0 },
    ]);
  });

  it("sends nothing for the keyup of a key that was never pressed", () => {
    const { tracker, sent } = mkTracker();
    expect(tracker.handleKey(false, kev())).toBe(true);
    expect(sent).toEqual([]);
  });

  it("forwards auto-repeat keydowns but only one keyup", () => {
    const { tracker, sent } = mkTracker();
    tracker.handleKey(true, kev());
    tracker.handleKey(true, kev());
    tracker.handleKey(true, kev());
    expect(tracker.heldCount()).toBe(1);
    tracker.handleKey(false, kev());
    expect(sent.map((m) => m.kind)).toEqual([
      "key_down",
      "key_down",
      "key_down",
      "key_up",
    ]);
  });
});

describe("KeyTracker.releaseAll", () => {
  /** Regression: releases used to be sent with key: 0 rather than the pressed keyCode. */
  it("releases each held key with the keyCode it was pressed with", () => {
    const { tracker, sent } = mkTracker();
    tracker.handleKey(true, kev());
    tracker.handleKey(true, kev({ key: "Shift", code: "ShiftLeft", keyCode: 16, shiftKey: true }));
    sent.length = 0;

    tracker.releaseAll();

    expect(sent).toHaveLength(2);
    expect(sent.every((m) => m.kind === "key_up")).toBe(true);
    expect(sent.every((m) => m.modifiers === 0)).toBe(true);
    expect(sent.map((m) => [m.code, m.key]).sort()).toEqual(
      [["KeyA", 65], ["ShiftLeft", 16]].sort(),
    );
    expect(sent.some((m) => m.key === 0)).toBe(false);
  });

  it("clears held state so a second call sends nothing", () => {
    const { tracker, sent } = mkTracker();
    tracker.handleKey(true, kev());
    tracker.releaseAll();
    sent.length = 0;
    tracker.releaseAll();
    expect(sent).toEqual([]);
  });

  it("does not double-release when the real keyup arrives after a blur", () => {
    const { tracker, sent } = mkTracker();
    tracker.handleKey(true, kev());
    tracker.releaseAll();
    sent.length = 0;
    tracker.handleKey(false, kev());
    expect(sent).toEqual([]);
  });

  it("sends nothing when no keys are held", () => {
    const { tracker, sent } = mkTracker();
    tracker.releaseAll();
    expect(sent).toEqual([]);
  });
});

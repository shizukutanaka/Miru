/**
 * Button-state tracking for touch input.
 *
 * The touch handlers pressed the left button on a one-finger touchstart but
 * released it on *every* touchend, unconditionally. Two real bugs followed:
 *
 * 1. A two-finger gesture never presses the left button (it scrolls), yet
 *    lifting the fingers sent a left `mouse_up` anyway — and `touchend` fires
 *    once per finger, so the host got **two** releases with no press.
 * 2. Putting a second finger down while the first is held left the button
 *    pressed for the whole scroll, so the host scrolled with left-drag active.
 *
 * This tracks whether a press is actually outstanding and releases exactly
 * once. Movement and pinch/scroll maths stay in the component — this owns only
 * button semantics, so it is pure and testable under Node.
 *
 * Not handled: right-click. See `docs/FEATURE_AUDIT.md` item 15g — a two-finger
 * tap cannot be mapped to right-click while the first finger's touchstart
 * presses the left button immediately, because the left click has already been
 * sent by the time the second finger lands. Doing it properly means deferring
 * the initial press until the gesture is disambiguated, which changes drag
 * latency and needs testing on real hardware.
 */

export interface TouchPos {
  x: number;
  y: number;
}

export type TouchAction =
  | { type: "press_left"; x: number; y: number }
  | { type: "release_left"; x: number; y: number };

export class TouchGestures {
  /** True while a left press has been sent and not yet released. */
  private leftDown = false;
  /** Position the press was sent at, so the release matches it. */
  private pressPos: TouchPos = { x: 0.5, y: 0.5 };

  /**
   * A touch began. `activeTouches` is the number of touches after the event.
   *
   * One finger presses the left button. A second finger means the gesture is a
   * scroll, so any outstanding press is released first.
   */
  start(activeTouches: number, pos: TouchPos): TouchAction[] {
    if (activeTouches === 1) {
      // Defensive: if a press somehow survived, release before re-pressing so
      // the host never sees two downs in a row.
      const out: TouchAction[] = [];
      if (this.leftDown) {
        out.push({ type: "release_left", ...this.pressPos });
      }
      this.leftDown = true;
      this.pressPos = pos;
      out.push({ type: "press_left", ...pos });
      return out;
    }

    if (this.leftDown) {
      this.leftDown = false;
      return [{ type: "release_left", ...this.pressPos }];
    }
    return [];
  }

  /** Track the position so a later release lands where the finger last was. */
  move(activeTouches: number, pos: TouchPos): void {
    if (activeTouches === 1 && this.leftDown) this.pressPos = pos;
  }

  /**
   * A touch ended. `remainingTouches` is the number still down afterwards.
   * Releases only if a press is actually outstanding, and only once.
   */
  end(): TouchAction[] {
    if (!this.leftDown) return [];
    this.leftDown = false;
    return [{ type: "release_left", ...this.pressPos }];
  }

  /** Release anything outstanding — for teardown / cancelled touches. */
  cancel(): TouchAction[] {
    return this.end();
  }

  /** Whether a left press is currently outstanding (for assertions). */
  isLeftDown(): boolean {
    return this.leftDown;
  }
}

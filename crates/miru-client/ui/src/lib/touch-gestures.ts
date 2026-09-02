/**
 * Touch → mouse button semantics.
 *
 * A remote desktop needs a right-click, and touch had no way to produce one.
 * Mapping it to a long press requires **deferring the initial press** until the
 * gesture is known, because a press sent on `touchstart` has already reached the
 * host by the time we could tell a long press from a tap.
 *
 * The cost of deferring is smaller than it looks — only a *stationary* finger
 * waits:
 *
 *   tap        press+release are emitted together on touchend. No perceptible
 *              difference from before.
 *   drag       the press is emitted the moment movement exceeds the slop
 *              threshold, and is stamped with the **original** touch position,
 *              so the drag still starts where the finger landed.
 *   long press held still for LONG_PRESS_MS and then lifted → right click.
 *
 * Firing on lift rather than on a timer keeps this a pure state machine: no
 * timers, no async, `now` is a parameter. It is therefore fully testable under
 * Node. The trade-off is that press-and-hold-without-moving is no longer
 * expressible from touch — the same trade-off Guacamole and RustDesk mobile
 * make, and the price of having a right click at all.
 *
 * It also fixes a real defect. Previously a two-finger scroll pressed the left
 * button on the first finger and released it when the second landed, so the
 * host received a spurious click at the start of every scroll. A deferred press
 * is simply discarded, so a scroll now emits no button events at all.
 *
 * Movement and pinch maths stay in the component; this owns only button
 * semantics.
 */

/** Held still at least this long, then lifted → right click. */
export const LONG_PRESS_MS = 500;
/**
 * Movement past this (in normalised 0–1 screen coords) means "drag", not "tap".
 * ~0.01 is ~19px on a 1920px-wide screen, close to Android's ~8dp touch slop.
 */
export const MOVE_SLOP = 0.01;

export interface TouchPos {
  x: number;
  y: number;
}

export type TouchAction =
  | { type: "press_left"; x: number; y: number }
  | { type: "release_left"; x: number; y: number }
  | { type: "press_right"; x: number; y: number }
  | { type: "release_right"; x: number; y: number };

export class TouchGestures {
  /** A touch is down but we have not decided what it is yet. */
  private pending = false;
  /** True while a left press has been sent and not yet released. */
  private leftDown = false;
  /** Where the touch began — presses are stamped here, not at the latest move. */
  private pressPos: TouchPos = { x: 0.5, y: 0.5 };
  private startedAt = 0;

  /**
   * A touch began. `activeTouches` is the number of touches after the event.
   *
   * One finger starts an undecided gesture and emits nothing. A second finger
   * means a scroll: an undecided press is discarded outright, and an
   * already-committed one is released so the host does not scroll with the
   * button held.
   */
  start(activeTouches: number, pos: TouchPos, now: number): TouchAction[] {
    if (activeTouches === 1) {
      // Defensive: a committed press should not survive into a new touch.
      const out: TouchAction[] = [];
      if (this.leftDown) {
        out.push({ type: "release_left", ...this.pressPos });
        this.leftDown = false;
      }
      this.pending = true;
      this.pressPos = pos;
      this.startedAt = now;
      return out;
    }

    this.pending = false;
    if (this.leftDown) {
      this.leftDown = false;
      return [{ type: "release_left", ...this.pressPos }];
    }
    return [];
  }

  /**
   * Moving far enough commits the gesture to a drag: the press is emitted now,
   * but positioned where the finger first landed.
   */
  move(activeTouches: number, pos: TouchPos, _now?: number): TouchAction[] {
    if (activeTouches !== 1) return [];

    if (this.pending) {
      const dx = pos.x - this.pressPos.x;
      const dy = pos.y - this.pressPos.y;
      if (Math.hypot(dx, dy) <= MOVE_SLOP) return [];
      this.pending = false;
      this.leftDown = true;
      const press: TouchAction = { type: "press_left", ...this.pressPos };
      this.pressPos = pos;
      return [press];
    }

    if (this.leftDown) this.pressPos = pos;
    return [];
  }

  /**
   * A touch ended. An undecided gesture resolves here — long enough is a right
   * click, otherwise a left click — and both halves are emitted together.
   */
  end(now: number): TouchAction[] {
    if (this.pending) {
      this.pending = false;
      const held = now - this.startedAt;
      const side = held >= LONG_PRESS_MS ? "right" : "left";
      return [
        { type: `press_${side}`, ...this.pressPos } as TouchAction,
        { type: `release_${side}`, ...this.pressPos } as TouchAction,
      ];
    }
    if (!this.leftDown) return [];
    this.leftDown = false;
    return [{ type: "release_left", ...this.pressPos }];
  }

  /**
   * Teardown or a cancelled touch. An undecided gesture is dropped rather than
   * resolved — a cancelled touch is not a click.
   */
  cancel(): TouchAction[] {
    this.pending = false;
    if (!this.leftDown) return [];
    this.leftDown = false;
    return [{ type: "release_left", ...this.pressPos }];
  }

  /** Whether a left press is currently outstanding (for assertions). */
  isLeftDown(): boolean {
    return this.leftDown;
  }

  /** Whether a touch is down but not yet resolved (for assertions). */
  isPending(): boolean {
    return this.pending;
  }
}

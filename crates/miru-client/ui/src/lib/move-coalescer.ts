/**
 * Latest-wins coalescing for pointer movement.
 *
 * Every DOM mousemove used to become its own Tauri IPC invoke. Mice report at
 * 125–1000 Hz, so dragging flooded the IPC channel — and the host's input
 * handler rate-limits to 1000 events/sec and drops indiscriminately once over,
 * meaning a burst of movement could starve keystrokes.
 *
 * Coalescing to one send per animation frame is the platform's own granularity,
 * not an arbitrary throttle: the existence of `PointerEvent.getCoalescedEvents()`
 * in Pointer Events Level 3 is precisely because browsers already batch pointer
 * movement to the frame and expose the raw samples only for apps that need them
 * (drawing tools). A remote desktop wants the batched side. `mousemove` is a
 * Mouse Events API and is not guaranteed to be frame-aligned, so the batching
 * has to happen here.
 *
 * The same latest-wins discipline is already used for video frames (ADR 0013);
 * this brings the input path in line.
 */

export interface MoveMsg {
  kind: "mouse_move";
  x: number;
  y: number;
}

export type MoveSend = (msg: MoveMsg) => void;

/** Injectable so tests can drive frames deterministically under node. */
export interface Scheduler {
  schedule(cb: () => void): number;
  cancel(handle: number): void;
}

/**
 * Browser scheduler. Safe to *import* under node — `requestAnimationFrame` is
 * only dereferenced when `schedule()` actually runs, and tests inject a fake.
 */
export const rafScheduler: Scheduler = {
  schedule: (cb) => requestAnimationFrame(cb),
  cancel: (h) => cancelAnimationFrame(h),
};

export class MoveCoalescer {
  private pending: { x: number; y: number } | null = null;
  private handle: number | null = null;
  private disposed = false;

  constructor(
    private send: MoveSend,
    private scheduler: Scheduler = rafScheduler,
  ) {}

  /** Record a position. Only the newest survives until the frame fires. */
  push(x: number, y: number): void {
    if (this.disposed) return;
    this.pending = { x, y };
    if (this.handle === null) {
      this.handle = this.scheduler.schedule(() => this.onFrame());
    }
  }

  private onFrame(): void {
    this.handle = null;
    const p = this.pending;
    this.pending = null;
    if (p && !this.disposed) {
      this.send({ kind: "mouse_move", x: p.x, y: p.y });
    }
  }

  /**
   * Send any pending move right now.
   *
   * Callers MUST flush before sending a click, wheel, or touch-end: those must
   * never overtake a queued move, or the button lands at a stale position. They
   * are sent directly rather than routed through here — a click is never
   * coalescible, so queueing it would add a kind-dispatch and an interleaving
   * problem for no benefit. Forgetting a flush merely degrades to the old
   * behaviour (click up to one frame ahead of the last move); a bug in a
   * unified queue could reorder or drop the click itself.
   */
  flush(): void {
    if (this.disposed || !this.pending) return;
    if (this.handle !== null) {
      this.scheduler.cancel(this.handle);
      this.handle = null;
    }
    const p = this.pending;
    this.pending = null;
    this.send({ kind: "mouse_move", x: p.x, y: p.y });
  }

  /**
   * Terminal teardown for unmount. Later push/flush become no-ops, and a frame
   * that still fires (a real rAF already committed, or a sloppy fake) sends
   * nothing.
   */
  cancel(): void {
    if (this.handle !== null) {
      this.scheduler.cancel(this.handle);
      this.handle = null;
    }
    this.pending = null;
    this.disposed = true;
  }
}

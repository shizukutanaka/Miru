import { describe, it, expect } from "vitest";
import { MoveCoalescer, type MoveMsg, type Scheduler } from "./move-coalescer";

/** Deterministic stand-in for requestAnimationFrame. */
class FakeScheduler implements Scheduler {
  private cbs = new Map<number, () => void>();
  private next = 1;
  scheduled = 0;
  cancelled = 0;

  schedule(cb: () => void): number {
    const h = this.next++;
    this.cbs.set(h, cb);
    this.scheduled++;
    return h;
  }
  cancel(handle: number): void {
    if (this.cbs.delete(handle)) this.cancelled++;
  }
  /** Run every still-scheduled callback, as a display frame would. */
  fire(): void {
    const due = [...this.cbs.values()];
    this.cbs.clear();
    for (const cb of due) cb();
  }
}

/** `sent` doubles as the ordering log — tests append their own markers to it. */
function mk() {
  const sent: Array<MoveMsg | { kind: string }> = [];
  const sched = new FakeScheduler();
  const c = new MoveCoalescer((m) => sent.push(m), sched);
  return { c, sent, sched };
}

describe("MoveCoalescer.push", () => {
  it("schedules a single frame no matter how many moves arrive", () => {
    const { c, sent, sched } = mk();
    c.push(0.1, 0.1);
    c.push(0.2, 0.2);
    c.push(0.3, 0.3);
    expect(sched.scheduled).toBe(1);
    expect(sent).toEqual([]); // nothing sent until the frame runs
  });

  it("sends only the newest position when the frame fires", () => {
    const { c, sent, sched } = mk();
    c.push(0.1, 0.1);
    c.push(0.9, 0.8);
    sched.fire();
    expect(sent).toEqual([{ kind: "mouse_move", x: 0.9, y: 0.8 }]);
  });

  it("schedules a fresh frame for the next move after one fires", () => {
    const { c, sent, sched } = mk();
    c.push(0.1, 0.1);
    sched.fire();
    c.push(0.2, 0.2);
    expect(sched.scheduled).toBe(2);
    sched.fire();
    expect(sent).toHaveLength(2);
  });
});

describe("MoveCoalescer.flush", () => {
  it("sends the pending move immediately and cancels the frame", () => {
    const { c, sent, sched } = mk();
    c.push(0.4, 0.5);
    c.flush();
    expect(sent).toEqual([{ kind: "mouse_move", x: 0.4, y: 0.5 }]);
    expect(sched.cancelled).toBe(1);
    sched.fire(); // the cancelled frame must not duplicate the move
    expect(sent).toHaveLength(1);
  });

  it("does nothing when no move is pending", () => {
    const { c, sent } = mk();
    expect(() => c.flush()).not.toThrow();
    expect(sent).toEqual([]);
  });

  /**
   * The invariant the whole design exists for: a click must land at the last
   * position the user moved to, never be overtaken by a stale queued move.
   */
  it("keeps a click after the move it follows, and no move after it", () => {
    const { c, sent, sched } = mk();
    c.push(0.1, 0.1);
    c.push(0.7, 0.7);
    c.flush();
    sent.push({ kind: "mouse_down" });
    sched.fire();
    expect(sent).toEqual([
      { kind: "mouse_move", x: 0.7, y: 0.7 },
      { kind: "mouse_down" },
    ]);
  });

  it("is idempotent — a second flush sends nothing", () => {
    const { c, sent } = mk();
    c.push(0.2, 0.2);
    c.flush();
    c.flush();
    expect(sent).toHaveLength(1);
  });
});

describe("MoveCoalescer.cancel", () => {
  it("drops a pending move and stays silent if a frame still fires", () => {
    const { c, sent, sched } = mk();
    c.push(0.5, 0.5);
    c.cancel();
    expect(sched.cancelled).toBe(1);
    sched.fire();
    expect(sent).toEqual([]);
  });

  it("makes later pushes no-ops", () => {
    const { c, sent, sched } = mk();
    c.cancel();
    c.push(0.5, 0.5);
    expect(sched.scheduled).toBe(0);
    sched.fire();
    expect(sent).toEqual([]);
  });

  it("makes a later flush a no-op", () => {
    const { c, sent } = mk();
    c.push(0.5, 0.5);
    c.cancel();
    expect(() => c.flush()).not.toThrow();
    expect(sent).toEqual([]);
  });

  it("is safe with nothing pending and safe to call twice", () => {
    const { c } = mk();
    expect(() => {
      c.cancel();
      c.cancel();
    }).not.toThrow();
  });
});

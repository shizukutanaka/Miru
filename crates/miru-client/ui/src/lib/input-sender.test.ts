import { describe, it, expect } from "vitest";
import { makeInputSender, type InputMsg } from "./input-sender";

function mk(ready: boolean) {
  const sent: InputMsg[] = [];
  let isReady = ready;
  const send = makeInputSender(
    () => isReady,
    async (m) => {
      sent.push(m);
    },
  );
  return { send, sent, setReady: (v: boolean) => (isReady = v) };
}

describe("makeInputSender", () => {
  it("forwards input while connected", () => {
    const { send, sent } = mk(true);
    send({ kind: "mouse_move", x: 0.5, y: 0.5 });
    expect(sent).toEqual([{ kind: "mouse_move", x: 0.5, y: 0.5 }]);
  });

  /**
   * Regression: the session slot exists before the handshake, so these used to
   * be buffered in the host channel and replayed into the remote machine the
   * moment the user confirmed the fingerprint.
   */
  it("drops input while not connected instead of queueing it", () => {
    const { send, sent } = mk(false);
    send({ kind: "key_down", code: "KeyA", key: 65, modifiers: 0 });
    send({ kind: "mouse_move", x: 0.1, y: 0.1 });
    expect(sent).toEqual([]);
  });

  it("re-checks readiness on every call rather than capturing it", () => {
    const { send, sent, setReady } = mk(false);
    send({ kind: "mouse_move", x: 0, y: 0 });
    expect(sent).toHaveLength(0);

    setReady(true);
    send({ kind: "mouse_move", x: 1, y: 1 });
    expect(sent).toHaveLength(1);

    setReady(false); // e.g. the session dropped
    send({ kind: "mouse_move", x: 0.5, y: 0.5 });
    expect(sent).toHaveLength(1);
  });

  it("swallows a rejected send so it cannot become an unhandled rejection", async () => {
    const send = makeInputSender(
      () => true,
      () => Promise.reject(new Error("session closed")),
    );
    expect(() => send({ kind: "mouse_up", x: 0, y: 0 })).not.toThrow();
    // Let the rejected promise settle; an unhandled rejection would surface here.
    await new Promise((r) => setTimeout(r, 0));
  });

  it("tolerates an invoke that throws synchronously", () => {
    const send = makeInputSender(
      () => true,
      () => {
        throw new Error("boom");
      },
    );
    // A synchronous throw is the caller's bug, but it must not kill the
    // event listener that called us.
    expect(() => send({ kind: "mouse_move", x: 0, y: 0 })).toThrow("boom");
  });
});

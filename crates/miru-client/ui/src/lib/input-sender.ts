/**
 * Gated, rejection-safe wrapper around the input IPC call.
 *
 * Two problems it solves, both traced through the Rust side:
 *
 * 1. **Pre-connection input is queued and replayed.** `AppState::connect`
 *    installs the session slot immediately, so `send_input` accepts events and
 *    buffers them into a 64-slot mpsc channel right away. That channel is only
 *    drained by the session loop, which starts *after* the handshake, the TOFU
 *    fingerprint confirmation, and cipher install. So anything the user does
 *    while the pairing dialog is open — mouse movement, and keystrokes, since
 *    the key listeners live on `window` — sits in the buffer and is flushed
 *    into the remote machine the instant they press "接続". Input made before
 *    the peer was confirmed must simply be dropped.
 *
 * 2. **Unhandled promise rejections.** Sends are fire-and-forget. If the
 *    session's channel closes mid-teardown, `send_input` returns Err and the
 *    unawaited promise rejects with nothing to catch it.
 *
 * `isReady` is evaluated per call (not captured), so a sender created once in
 * an effect still sees the current connection state.
 */

/** Structurally matches the parameter of `api.sendInput`. */
export interface InputMsg {
  kind: string;
  x?: number;
  y?: number;
  button?: string;
  key?: number;
  modifiers?: number;
  code?: string;
  dx?: number;
  dy?: number;
  text?: string;
}

export type InputInvoke = (msg: InputMsg) => Promise<unknown>;
export type InputSender = (msg: InputMsg) => void;

export function makeInputSender(
  isReady: () => boolean,
  invoke: InputInvoke,
): InputSender {
  return (msg) => {
    if (!isReady()) return;
    // Swallow: a send failing during teardown is expected and there is nothing
    // useful to show the user for a single dropped input event.
    void Promise.resolve(invoke(msg)).catch(() => {});
  };
}

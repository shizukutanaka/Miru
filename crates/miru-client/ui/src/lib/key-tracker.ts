/**
 * Keyboard state tracking for the session view.
 *
 * Extracted from SessionScreen so it can be unit-tested: this logic has already
 * shipped two real bugs (an unmatched key_up when Ctrl was released before the
 * letter in a blocked chord, and releaseAll sending `key: 0` instead of the
 * keyCode that was pressed), and there was no test to catch either.
 *
 * Deliberately DOM-free — it takes plain event-shaped objects rather than
 * `KeyboardEvent`, so it runs under Vitest's default node environment with no
 * jsdom. The component keeps the `addEventListener` calls and forwards events
 * here; a real `KeyboardEvent` satisfies `KeyEventLike` structurally.
 */

/**
 * Ctrl chords the keyboard handler blocks so they act on the viewer's own
 * window instead of being forwarded (Ctrl+W would otherwise close the viewer).
 * `key` is the legacy browser keyCode, kept so hosts predating the `code` field
 * still resolve the key.
 *
 * This is the single source of truth: the toolbar's "send this chord" picker
 * and the keyboard filter below both derive from it. They used to be two
 * hand-maintained lists that could silently drift apart.
 */
export const BLOCKED_CHORDS = [
  { label: "Ctrl+W", code: "KeyW", key: 87 },
  { label: "Ctrl+Q", code: "KeyQ", key: 81 },
  { label: "Ctrl+H", code: "KeyH", key: 72 },
  { label: "Ctrl+M", code: "KeyM", key: 77 },
] as const;

/** Letters of BLOCKED_CHORDS, derived so the filter can never drift from the picker. */
const BLOCKED_LETTERS: ReadonlySet<string> = new Set(
  BLOCKED_CHORDS.map((c) => c.code.replace(/^Key/, "").toLowerCase()),
);

/** Structurally satisfied by DOM KeyboardEvent; plain objects work in tests. */
export interface KeyEventLike {
  key: string;
  code: string;
  keyCode: number;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

export interface KeyInputMsg {
  kind: "key_down" | "key_up";
  code: string;
  key: number;
  modifiers: number;
}

export type KeySend = (msg: KeyInputMsg) => void;

/**
 * True when this event is a window-management chord the viewer must keep for
 * itself. Matched on `key` (the character the layout produced) rather than
 * `code`, because the OS shortcuts these collide with are defined on characters.
 */
export function isBlockedChord(
  e: Pick<KeyEventLike, "key" | "ctrlKey" | "metaKey">,
): boolean {
  return (e.ctrlKey || e.metaKey) && BLOCKED_LETTERS.has(e.key.toLowerCase());
}

/** Modifier bitmask matching the host's InputEvent encoding. */
export function modifierBits(
  e: Pick<KeyEventLike, "shiftKey" | "ctrlKey" | "altKey" | "metaKey">,
): number {
  return (
    (e.shiftKey ? 0x01 : 0) |
    (e.ctrlKey ? 0x02 : 0) |
    (e.altKey ? 0x04 : 0) |
    (e.metaKey ? 0x08 : 0)
  );
}

export class KeyTracker {
  /**
   * Physical keys currently held, mapped to the legacy keyCode sent with the
   * press so the matching release can carry the same value. Needed to release
   * everything if the viewer loses focus: otherwise alt-tabbing away with a
   * modifier down leaves it latched on the host, and on Linux the uinput device
   * outlives the session so it stays stuck across reconnects.
   */
  private held = new Map<string, number>();

  constructor(private send: KeySend) {}

  /**
   * Process one key event.
   *
   * Returns whether the caller should `preventDefault()` — false means the
   * event is a blocked chord that must reach the viewer's own window.
   */
  handleKey(down: boolean, e: KeyEventLike): boolean {
    if (isBlockedChord(e)) return false;

    // Never release a key we never pressed. Releasing Ctrl before W in a Ctrl+W
    // chord clears e.ctrlKey, so W's keyup slips past the check above even
    // though its keydown was blocked. Still "consumed" (the old code called
    // preventDefault before this guard), just not forwarded.
    if (!down && !this.held.has(e.code)) return true;

    const modifiers = modifierBits(e);
    if (down) this.held.set(e.code, e.keyCode);
    else this.held.delete(e.code);

    this.send({
      kind: down ? "key_down" : "key_up",
      // `code` is the physical key and is layout-independent; `key` is the
      // legacy keyCode, sent only so older hosts keep working.
      code: e.code,
      key: e.keyCode,
      modifiers,
    });
    return true;
  }

  /** Release every held key. Called on blur, tab hide, and unmount. */
  releaseAll(): void {
    for (const [code, keyCode] of this.held) {
      this.send({ kind: "key_up", code, key: keyCode, modifiers: 0 });
    }
    this.held.clear();
  }

  /** Number of keys currently held — for assertions. */
  heldCount(): number {
    return this.held.size;
  }
}

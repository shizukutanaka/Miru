//! Physical-key mapping: W3C UI Events `code` → per-OS key codes.
//!
//! # Why this exists
//!
//! The viewer used to send the browser's `KeyboardEvent.keyCode` and each host
//! backend cast that number straight into its OS API. That is wrong on two of
//! three platforms, because `keyCode` (historically derived from Windows
//! virtual-key codes) and the OS key codes live in *different numbering
//! spaces*:
//!
//! | key | browser keyCode | evdev | CGKeyCode | Windows VK |
//! |-----|-----------------|-------|-----------|------------|
//! | `A` | 65              | 30    | 0         | 65         |
//! | `Enter` | 13          | 28    | 36        | 13         |
//!
//! So a raw cast made `A` arrive as evdev 65 (`KEY_F7`) on Linux and as
//! CGKeyCode 65 (keypad `.`) on macOS. Windows happened to work only because
//! `keyCode` and VK largely coincide.
//!
//! # Why `code` and not `keyCode`
//!
//! `KeyboardEvent.code` identifies the *physical* key ("KeyA", "Semicolon"),
//! independent of the viewer's keyboard layout, whereas `keyCode`/`key` reflect
//! the character that layout produced. A remote desktop must send the physical
//! key and let the *host* apply its own layout — otherwise a Dvorak or AZERTY
//! viewer types mojibake on a US host. This is the same approach VNC/RDP-style
//! clients take, and `keyCode` is deprecated in the UI Events spec.
//!
//! # Source of the numbers
//!
//! The evdev and macOS columns are transcribed from Chromium's canonical
//! cross-platform table, `ui/events/keycodes/dom/keycode_converter_data.inc`
//! (a.k.a. `dom_code_data.inc`), which is itself derived from the USB HID
//! Usage Tables and the W3C "UI Events KeyboardEvent code Values" spec.
//! Windows uses virtual-key codes (`WinUser.h` `VK_*`) rather than Chromium's
//! scancode column, because `platform/windows.rs` injects via `VIRTUAL_KEY`.
//!
//! Unmapped keys return `None`; callers fall back to the legacy raw cast so
//! older viewers that send only `keyCode` keep working.

/// Sentinel for "this platform has no code for this key", matching the
/// `0xffff` convention in Chromium's table. Some keys genuinely do not exist
/// on a platform — macOS has no Convert/NonConvert/KanaMode/PrintScreen/
/// ScrollLock/Pause keycode, for instance. The accessors return `None` for
/// these so the caller falls back rather than injecting key 65535.
const NO_CODE: u16 = 0xFFFF;

/// Per-OS codes for one physical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCodes {
    /// Linux evdev `KEY_*` code (what uinput `EV_KEY` expects).
    pub evdev: u16,
    /// macOS `CGKeyCode` (Carbon `kVK_*` virtual key).
    pub mac: u16,
    /// Windows virtual-key code (`VK_*`).
    pub vk: u16,
}

/// Look up a W3C `KeyboardEvent.code` string. Returns `None` for keys this
/// table does not cover (caller should fall back to legacy behaviour).
pub fn lookup(code: &str) -> Option<KeyCodes> {
    // (evdev, mac, vk)
    let (evdev, mac, vk) = match code {
        // ── Letters ──────────────────────────────────────────────────────
        "KeyA" => (30, 0, 0x41),
        "KeyB" => (48, 11, 0x42),
        "KeyC" => (46, 8, 0x43),
        "KeyD" => (32, 2, 0x44),
        "KeyE" => (18, 14, 0x45),
        "KeyF" => (33, 3, 0x46),
        "KeyG" => (34, 5, 0x47),
        "KeyH" => (35, 4, 0x48),
        "KeyI" => (23, 34, 0x49),
        "KeyJ" => (36, 38, 0x4A),
        "KeyK" => (37, 40, 0x4B),
        "KeyL" => (38, 37, 0x4C),
        "KeyM" => (50, 46, 0x4D),
        "KeyN" => (49, 45, 0x4E),
        "KeyO" => (24, 31, 0x4F),
        "KeyP" => (25, 35, 0x50),
        "KeyQ" => (16, 12, 0x51),
        "KeyR" => (19, 15, 0x52),
        "KeyS" => (31, 1, 0x53),
        "KeyT" => (20, 17, 0x54),
        "KeyU" => (22, 32, 0x55),
        "KeyV" => (47, 9, 0x56),
        "KeyW" => (17, 13, 0x57),
        "KeyX" => (45, 7, 0x58),
        "KeyY" => (21, 16, 0x59),
        "KeyZ" => (44, 6, 0x5A),

        // ── Digit row (note: evdev/mac order is 1-9 then 0) ──────────────
        "Digit1" => (2, 18, 0x31),
        "Digit2" => (3, 19, 0x32),
        "Digit3" => (4, 20, 0x33),
        "Digit4" => (5, 21, 0x34),
        "Digit5" => (6, 23, 0x35),
        "Digit6" => (7, 22, 0x36),
        "Digit7" => (8, 26, 0x37),
        "Digit8" => (9, 28, 0x38),
        "Digit9" => (10, 25, 0x39),
        "Digit0" => (11, 29, 0x30),

        // ── Editing / whitespace ─────────────────────────────────────────
        "Enter" => (28, 36, 0x0D),
        "Escape" => (1, 53, 0x1B),
        "Backspace" => (14, 51, 0x08),
        "Tab" => (15, 48, 0x09),
        "Space" => (57, 49, 0x20),
        "CapsLock" => (58, 57, 0x14),

        // ── Punctuation (Windows uses OEM VKs) ───────────────────────────
        "Minus" => (12, 27, 0xBD),        // VK_OEM_MINUS
        "Equal" => (13, 24, 0xBB),        // VK_OEM_PLUS
        "BracketLeft" => (26, 33, 0xDB),  // VK_OEM_4
        "BracketRight" => (27, 30, 0xDD), // VK_OEM_6
        "Backslash" => (43, 42, 0xDC),    // VK_OEM_5
        "Semicolon" => (39, 41, 0xBA),    // VK_OEM_1
        "Quote" => (40, 39, 0xDE),        // VK_OEM_7
        "Backquote" => (41, 50, 0xC0),    // VK_OEM_3
        "Comma" => (51, 43, 0xBC),        // VK_OEM_COMMA
        "Period" => (52, 47, 0xBE),       // VK_OEM_PERIOD
        "Slash" => (53, 44, 0xBF),        // VK_OEM_2

        // ── Function row ─────────────────────────────────────────────────
        "F1" => (59, 122, 0x70),
        "F2" => (60, 120, 0x71),
        "F3" => (61, 99, 0x72),
        "F4" => (62, 118, 0x73),
        "F5" => (63, 96, 0x74),
        "F6" => (64, 97, 0x75),
        "F7" => (65, 98, 0x76),
        "F8" => (66, 100, 0x77),
        "F9" => (67, 101, 0x78),
        "F10" => (68, 109, 0x79),
        "F11" => (87, 103, 0x7A),
        "F12" => (88, 111, 0x7B),

        // ── Navigation ───────────────────────────────────────────────────
        "Insert" => (110, 114, 0x2D),
        "Delete" => (111, 117, 0x2E),
        "Home" => (102, 115, 0x24),
        "End" => (107, 119, 0x23),
        "PageUp" => (104, 116, 0x21),
        "PageDown" => (109, 121, 0x22),
        "ArrowUp" => (103, 126, 0x26),
        "ArrowDown" => (108, 125, 0x28),
        "ArrowLeft" => (105, 123, 0x25),
        "ArrowRight" => (106, 124, 0x27),

        // ── Modifiers (left/right distinguished) ─────────────────────────
        "ControlLeft" => (29, 59, 0xA2),  // VK_LCONTROL
        "ControlRight" => (97, 62, 0xA3), // VK_RCONTROL
        "ShiftLeft" => (42, 56, 0xA0),    // VK_LSHIFT
        "ShiftRight" => (54, 60, 0xA1),   // VK_RSHIFT
        "AltLeft" => (56, 58, 0xA4),      // VK_LMENU
        "AltRight" => (100, 61, 0xA5),    // VK_RMENU
        "MetaLeft" => (125, 55, 0x5B),    // VK_LWIN
        "MetaRight" => (126, 54, 0x5C),   // VK_RWIN

        // ── Japanese (JIS) / international keys ──────────────────────────
        // Without these a JIS keyboard cannot type ¥ or ろ, and cannot switch
        // IME state at all (変換 / 無変換 / かな). macOS has no keycode for
        // the three IME keys, so they resolve to None there.
        "IntlYen" => (124, 93, 0xDC),          // ¥      VK_OEM_5
        "IntlRo" => (89, 94, 0xE2),            // ろ/_   VK_OEM_102
        "IntlBackslash" => (86, 10, 0xE2),     // ISO <> VK_OEM_102
        "Convert" => (92, NO_CODE, 0x1C),      // 変換   VK_CONVERT
        "NonConvert" => (94, NO_CODE, 0x1D),   // 無変換 VK_NONCONVERT
        "KanaMode" => (93, NO_CODE, 0x15),     // かな   VK_KANA
        // Lang1/Lang2 are Hangul/Hanja on Korean layouts and share Windows VKs
        // with Kana/Kanji — that collision is Windows' own numbering, not a
        // transcription error.
        "Lang1" => (122, 104, 0x15),
        "Lang2" => (123, 102, 0x19),

        // ── Numeric keypad ───────────────────────────────────────────────
        "Numpad0" => (82, 82, 0x60),
        "Numpad1" => (79, 83, 0x61),
        "Numpad2" => (80, 84, 0x62),
        "Numpad3" => (81, 85, 0x63),
        "Numpad4" => (75, 86, 0x64),
        "Numpad5" => (76, 87, 0x65),
        "Numpad6" => (77, 88, 0x66),
        "Numpad7" => (71, 89, 0x67),
        "Numpad8" => (72, 91, 0x68),
        "Numpad9" => (73, 92, 0x69),
        "NumpadDivide" => (98, 75, 0x6F),   // VK_DIVIDE
        "NumpadMultiply" => (55, 67, 0x6A), // VK_MULTIPLY
        "NumpadSubtract" => (74, 78, 0x6D), // VK_SUBTRACT
        "NumpadAdd" => (78, 69, 0x6B),      // VK_ADD
        "NumpadDecimal" => (83, 65, 0x6E),  // VK_DECIMAL
        // Windows does not have a separate VK for the keypad Enter; it is
        // VK_RETURN distinguished by the extended-key flag.
        "NumpadEnter" => (96, 76, 0x0D),
        "NumLock" => (69, 71, 0x90),

        // ── Remaining system keys ────────────────────────────────────────
        "PrintScreen" => (99, NO_CODE, 0x2C), // VK_SNAPSHOT
        "ScrollLock" => (70, NO_CODE, 0x91),  // VK_SCROLL
        "Pause" => (119, NO_CODE, 0x13),      // VK_PAUSE
        "ContextMenu" => (127, 110, 0x5D),    // VK_APPS

        "F13" => (183, 105, 0x7C),
        "F14" => (184, 107, 0x7D),
        "F15" => (185, 113, 0x7E),
        "F16" => (186, 106, 0x7F),
        "F17" => (187, 64, 0x80),
        "F18" => (188, 79, 0x81),
        "F19" => (189, 80, 0x82),
        "F20" => (190, 90, 0x83),

        _ => return None,
    };
    Some(KeyCodes { evdev, mac, vk })
}

/// Linux evdev `KEY_*` code for a W3C `code`, if this platform has one.
pub fn code_to_evdev(code: &str) -> Option<u16> {
    lookup(code).map(|k| k.evdev).filter(|&c| c != NO_CODE)
}

/// macOS `CGKeyCode` for a W3C `code`, if this platform has one. Returns
/// `None` for keys macOS lacks entirely (変換 / 無変換 / かな, PrintScreen,
/// ScrollLock, Pause).
pub fn code_to_cgkeycode(code: &str) -> Option<u16> {
    lookup(code).map(|k| k.mac).filter(|&c| c != NO_CODE)
}

/// Windows virtual-key code for a W3C `code`, if this platform has one.
pub fn code_to_vk(code: &str) -> Option<u16> {
    lookup(code).map(|k| k.vk).filter(|&c| c != NO_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact regression this module exists to prevent: before the mapping
    /// table, "KeyA" arrived as the browser keyCode 65, which evdev reads as
    /// KEY_F7 and CoreGraphics reads as the keypad decimal key.
    #[test]
    fn letter_a_is_not_the_raw_browser_keycode() {
        let a = lookup("KeyA").expect("KeyA must be mapped");
        assert_eq!(a.evdev, 30, "evdev KEY_A");
        assert_eq!(a.mac, 0, "kVK_ANSI_A");
        assert_eq!(a.vk, 0x41, "VK 'A' — matches legacy keyCode by coincidence");
        assert_ne!(a.evdev, 65, "must not pass browser keyCode through (KEY_F7)");
        assert_ne!(a.mac, 65, "must not pass browser keyCode through");
    }

    #[test]
    fn enter_differs_across_all_three_platforms() {
        let e = lookup("Enter").unwrap();
        assert_eq!((e.evdev, e.mac, e.vk), (28, 36, 0x0D));
    }

    /// evdev constants that platform/linux.rs previously hardcoded for its
    /// clipboard-paste path — proof the table agrees with known-good values.
    #[test]
    fn matches_known_good_evdev_constants() {
        assert_eq!(code_to_evdev("ControlLeft"), Some(29)); // KEY_LEFTCTRL
        assert_eq!(code_to_evdev("KeyV"), Some(47)); // KEY_V
    }

    #[test]
    fn left_and_right_modifiers_are_distinct() {
        for (l, r) in [
            ("ControlLeft", "ControlRight"),
            ("ShiftLeft", "ShiftRight"),
            ("AltLeft", "AltRight"),
            ("MetaLeft", "MetaRight"),
        ] {
            assert_ne!(lookup(l).unwrap(), lookup(r).unwrap(), "{l} vs {r}");
        }
    }

    /// Digit row is ordered 1..9 then 0 in both evdev and macOS numbering —
    /// an easy place to introduce an off-by-one.
    #[test]
    fn digit_zero_follows_nine() {
        assert_eq!(code_to_evdev("Digit9"), Some(10));
        assert_eq!(code_to_evdev("Digit0"), Some(11));
        assert_eq!(code_to_vk("Digit0"), Some(0x30));
        assert_eq!(code_to_vk("Digit9"), Some(0x39));
    }

    /// JIS keyboards: ¥ and ろ are ordinary keys a Japanese user hits daily,
    /// and 変換/無変換/かな are how the IME is driven at all.
    #[test]
    fn jis_keys_are_mapped_on_linux() {
        assert_eq!(code_to_evdev("IntlYen"), Some(124));
        assert_eq!(code_to_evdev("IntlRo"), Some(89));
        assert_eq!(code_to_evdev("Convert"), Some(92));
        assert_eq!(code_to_evdev("NonConvert"), Some(94));
        assert_eq!(code_to_evdev("KanaMode"), Some(93));
    }

    /// macOS has no keycode for the IME keys. They must resolve to None rather
    /// than injecting the 0xFFFF sentinel as a real key.
    #[test]
    fn keys_absent_on_macos_resolve_to_none() {
        for code in ["Convert", "NonConvert", "KanaMode", "PrintScreen", "ScrollLock", "Pause"] {
            assert_eq!(code_to_cgkeycode(code), None, "{code} should be absent on macOS");
            // ...but they still exist on Linux and Windows.
            assert!(code_to_evdev(code).is_some(), "{code} evdev");
            assert!(code_to_vk(code).is_some(), "{code} vk");
        }
    }

    /// The keypad is physically distinct from the digit row: Numpad1 and
    /// Digit1 must never collide, or numeric entry lands on the wrong key.
    #[test]
    fn numpad_is_distinct_from_digit_row() {
        for d in 0..=9u8 {
            let pad = lookup(&format!("Numpad{d}")).unwrap();
            let row = lookup(&format!("Digit{d}")).unwrap();
            assert_ne!(pad.evdev, row.evdev, "Numpad{d}/Digit{d} evdev");
            assert_ne!(pad.mac, row.mac, "Numpad{d}/Digit{d} mac");
            assert_ne!(pad.vk, row.vk, "Numpad{d}/Digit{d} vk");
        }
        assert_eq!(code_to_vk("Numpad0"), Some(0x60)); // VK_NUMPAD0
        assert_eq!(code_to_vk("Digit0"), Some(0x30));
    }

    /// Four pairs share a Windows VK. These are Windows' own numbering, not
    /// transcription slips, and are pinned here so a future edit that creates
    /// an *unintended* collision is noticed. evdev and CGKeyCode — the two
    /// spaces that were actually broken — have no collisions at all.
    #[test]
    fn known_windows_vk_collisions_are_intentional() {
        // No separate VK for keypad Enter; the extended-key flag distinguishes it.
        assert_eq!(code_to_vk("NumpadEnter"), code_to_vk("Enter"));
        // On JIS the ¥ key sits where US has backslash; both report VK_OEM_5.
        assert_eq!(code_to_vk("IntlYen"), code_to_vk("Backslash"));
        // VK_OEM_102 is "the extra key": ろ on JIS, <> on ISO. Never both.
        assert_eq!(code_to_vk("IntlRo"), code_to_vk("IntlBackslash"));
        // VK_KANA and VK_HANGUL are both 0x15 in WinUser.h.
        assert_eq!(code_to_vk("KanaMode"), code_to_vk("Lang1"));

        // ...but these must stay distinct where it matters.
        assert_ne!(code_to_evdev("NumpadEnter"), code_to_evdev("Enter"));
        assert_ne!(code_to_evdev("IntlYen"), code_to_evdev("Backslash"));
        assert_ne!(code_to_cgkeycode("IntlYen"), code_to_cgkeycode("Backslash"));
    }

    #[test]
    fn unknown_code_returns_none_so_caller_can_fall_back() {
        assert_eq!(lookup("NoSuchKey"), None);
        assert_eq!(lookup(""), None);
        assert_eq!(code_to_evdev("F21"), None); // beyond F20, not in the table
    }

    /// Letters and digits must be unique per platform — a duplicated row would
    /// silently make two keys type the same character.
    #[test]
    fn alphanumeric_mappings_are_unique() {
        let codes: Vec<String> = ('A'..='Z')
            .map(|c| format!("Key{c}"))
            .chain((0..=9).map(|d| format!("Digit{d}")))
            .collect();
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                let (ka, kb) = (lookup(a).unwrap(), lookup(b).unwrap());
                assert_ne!(ka.evdev, kb.evdev, "{a}/{b} evdev collision");
                assert_ne!(ka.mac, kb.mac, "{a}/{b} mac collision");
                assert_ne!(ka.vk, kb.vk, "{a}/{b} vk collision");
            }
        }
    }
}

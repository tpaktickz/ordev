//! Linux input-code translation.
//!
//! The shortcut model is defined in terms of Win32 virtual-key codes
//! (`ParsedKeyCombo::vk_code`), because the config file, the UI and every
//! existing binding speak that vocabulary. On Linux the kernel reports
//! `linux/input-event-codes.h` `KEY_*` values instead, so this module is the one
//! place that translates between the two: config stays untouched, and a binding
//! written as `Ctrl+Alt+F` keeps working.
//!
//! Only keys that `map_key_to_vk` can produce are mapped; anything else returns
//! `None` and is ignored by the dispatcher.

/// `KEY_*` codes that act as modifiers. Tracked as state, never dispatched.
pub const MODIFIER_CODES: &[u16] = &[
    29,  // KEY_LEFTCTRL
    97,  // KEY_RIGHTCTRL
    42,  // KEY_LEFTSHIFT
    54,  // KEY_RIGHTSHIFT
    56,  // KEY_LEFTALT
    100, // KEY_RIGHTALT
    125, // KEY_LEFTMETA
    126, // KEY_RIGHTMETA
];

/// Which modifier a `KEY_*` code sets, if any.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Win,
}

pub fn modifier_of(code: u16) -> Option<Modifier> {
    match code {
        29 | 97 => Some(Modifier::Ctrl),
        42 | 54 => Some(Modifier::Shift),
        56 | 100 => Some(Modifier::Alt),
        125 | 126 => Some(Modifier::Win),
        _ => None,
    }
}

/// Translate an evdev `KEY_*` code into the Win32 virtual-key code used by the
/// shortcut model. Returns `None` for keys outside the supported vocabulary.
pub fn vk_from_evdev(code: u16) -> Option<u16> {
    Some(match code {
        // Letters. The VK code is just the ASCII of the uppercase letter, and
        // the evdev codes are not in alphabetical order (KEY_A..KEY_L = 30..=38,
        // then grave/shift/backslash at 41..43, then KEY_Z..KEY_M = 44..=50), so
        // this is spelled out instead of computed.
        30 => 0x41, // A
        48 => 0x42, // B
        46 => 0x43, // C
        32 => 0x44, // D
        18 => 0x45, // E
        33 => 0x46, // F
        34 => 0x47, // G
        35 => 0x48, // H
        23 => 0x49, // I
        36 => 0x4A, // J
        37 => 0x4B, // K
        38 => 0x4C, // L
        50 => 0x4D, // M
        49 => 0x4E, // N
        24 => 0x4F, // O
        25 => 0x50, // P
        16 => 0x51, // Q
        19 => 0x52, // R
        31 => 0x53, // S
        20 => 0x54, // T
        22 => 0x55, // U
        47 => 0x56, // V
        17 => 0x57, // W
        45 => 0x58, // X
        21 => 0x59, // Y
        44 => 0x5A, // Z
        // Digits: KEY_1..KEY_9 = 2..10, KEY_0 = 11 -> '1'..'9', '0'.
        2..=10 => 0x31 + (code - 2),
        11 => 0x30,
        // Function keys are NOT a contiguous block in evdev: F1..F10 are
        // 59..=68, F11/F12 are 87/88, and F13..F24 are 183..=194.
        59..=68 => 0x70 + (code - 59),
        87 => 0x7A,  // KEY_F11
        88 => 0x7B,  // KEY_F12
        183..=194 => 0x7C + (code - 183), // KEY_F13..KEY_F24

        57 => 0x20,   // KEY_SPACE
        15 => 0x09,   // KEY_TAB
        28 => 0x0D,   // KEY_ENTER
        1 => 0x1B,    // KEY_ESC
        14 => 0x08,   // KEY_BACKSPACE
        111 => 0x2E,  // KEY_DELETE
        110 => 0x2D,  // KEY_INSERT
        102 => 0x24,  // KEY_HOME
        107 => 0x23,  // KEY_END
        104 => 0x21,  // KEY_PAGEUP
        109 => 0x22,  // KEY_PAGEDOWN
        105 => 0x25,  // KEY_LEFT
        103 => 0x26,  // KEY_UP
        106 => 0x27,  // KEY_RIGHT
        108 => 0x28,  // KEY_DOWN
        58 => 0x14,   // KEY_CAPSLOCK
        69 => 0x90,   // KEY_NUMLOCK
        70 => 0x91,   // KEY_SCROLLLOCK
        99 => 0x2C,   // KEY_SYSRQ -> VK_SNAPSHOT
        119 => 0x13,  // KEY_PAUSE

        // OEM punctuation, matching the US layout codes in `map_key_to_vk`.
        12 => 0xBD,  // KEY_MINUS
        13 => 0xBB,  // KEY_EQUAL
        26 => 0xDB,  // KEY_LEFTBRACE
        27 => 0xDD,  // KEY_RIGHTBRACE
        39 => 0xBA,  // KEY_SEMICOLON
        40 => 0xDE,  // KEY_APOSTROPHE
        51 => 0xBC,  // KEY_COMMA
        52 => 0xBE,  // KEY_DOT
        53 => 0xBF,  // KEY_SLASH
        43 => 0xDC,  // KEY_BACKSLASH
        41 => 0xC0,  // KEY_GRAVE

        // Media keys, driven by the chassis firmware on most laptops.
        113 => 0xAD, // KEY_MUTE
        114 => 0xAE, // KEY_VOLUMEDOWN
        115 => 0xAF, // KEY_VOLUMEUP
        163 => 0xB0, // KEY_NEXTSONG
        165 => 0xB1, // KEY_PREVIOUSSONG
        166 => 0xB2, // KEY_STOPCD
        164 => 0xB3, // KEY_PLAYPAUSE

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_fkeys_line_up() {
        assert_eq!(vk_from_evdev(30), Some(0x41)); // A
        assert_eq!(vk_from_evdev(44), Some(0x5A)); // Z
        assert_eq!(vk_from_evdev(50), Some(0x4D)); // M
        assert_eq!(vk_from_evdev(51), Some(0xBC)); // comma, not a letter
        assert_eq!(vk_from_evdev(2), Some(0x31)); // 1
        assert_eq!(vk_from_evdev(11), Some(0x30)); // 0
        assert_eq!(vk_from_evdev(59), Some(0x70)); // F1
        assert_eq!(vk_from_evdev(68), Some(0x79)); // F10
    }

    #[test]
    fn modifiers_are_state_not_primary_keys() {
        assert_eq!(modifier_of(29), Some(Modifier::Ctrl));
        assert_eq!(modifier_of(125), Some(Modifier::Win));
        assert_eq!(modifier_of(30), None);
        assert_eq!(vk_from_evdev(29), None);
    }
}

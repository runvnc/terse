//! Browser keys to USB HID keyboard usage IDs (SPEC §events), as
//! terse-host's `keymap::hid_usage` maps winit's physical keys. The browser
//! names physical keys by `KeyboardEvent.code` (W3C UI Events), the names
//! winit's `KeyCode` copies, so the table is the same key for key.

/// The HID usage of the physical key `code` (`KeyboardEvent.code`).
pub fn hid_usage(code: &str) -> Option<u8> {
    if let Some(l) = code.strip_prefix("Key").filter(|l| l.len() == 1) {
        let c = l.as_bytes()[0];
        return c.is_ascii_uppercase().then(|| 0x04 + (c - b'A'));
    }
    if let Some(d) = code.strip_prefix("Digit").filter(|d| d.len() == 1) {
        let c = d.as_bytes()[0];
        return match c {
            b'1'..=b'9' => Some(0x1E + (c - b'1')),
            b'0' => Some(0x27),
            _ => None,
        };
    }
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&n).then(|| 0x3A + n - 1);
    }
    if let Some(d) = code.strip_prefix("Numpad").filter(|d| d.len() == 1) {
        let c = d.as_bytes()[0];
        return match c {
            b'1'..=b'9' => Some(0x59 + (c - b'1')),
            b'0' => Some(0x62),
            _ => None,
        };
    }
    Some(match code {
        "Enter" => 0x28,
        "Escape" => 0x29,
        "Backspace" => 0x2A,
        "Tab" => 0x2B,
        "Space" => 0x2C,
        "Minus" => 0x2D,
        "Equal" => 0x2E,
        "BracketLeft" => 0x2F,
        "BracketRight" => 0x30,
        "Backslash" => 0x31,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "CapsLock" => 0x39,
        "PrintScreen" => 0x46,
        "ScrollLock" => 0x47,
        "Pause" => 0x48,
        "Insert" => 0x49,
        "Home" => 0x4A,
        "PageUp" => 0x4B,
        "Delete" => 0x4C,
        "End" => 0x4D,
        "PageDown" => 0x4E,
        "ArrowRight" => 0x4F,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "NumLock" => 0x53,
        "NumpadDivide" => 0x54,
        "NumpadMultiply" => 0x55,
        "NumpadSubtract" => 0x56,
        "NumpadAdd" => 0x57,
        "NumpadEnter" => 0x58,
        "NumpadDecimal" => 0x63,
        "IntlBackslash" => 0x64,
        "ContextMenu" => 0x65,
        "ControlLeft" => 0xE0,
        "ShiftLeft" => 0xE1,
        "AltLeft" => 0xE2,
        "MetaLeft" | "OSLeft" => 0xE3,
        "ControlRight" => 0xE4,
        "ShiftRight" => 0xE5,
        "AltRight" => 0xE6,
        "MetaRight" | "OSRight" => 0xE7,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC §events' examples, as terse-host's keymap test has them.
    #[test]
    fn spec_examples() {
        assert_eq!(hid_usage("KeyA"), Some(0x04));
        assert_eq!(hid_usage("KeyZ"), Some(0x1D));
        assert_eq!(hid_usage("Digit1"), Some(0x1E));
        assert_eq!(hid_usage("Digit0"), Some(0x27));
        assert_eq!(hid_usage("Enter"), Some(0x28));
        assert_eq!(hid_usage("Escape"), Some(0x29));
        assert_eq!(hid_usage("ArrowRight"), Some(0x4F));
        assert_eq!(hid_usage("ArrowUp"), Some(0x52));
        assert_eq!(hid_usage("F1"), Some(0x3A));
        assert_eq!(hid_usage("F12"), Some(0x45));
        assert_eq!(hid_usage("Numpad1"), Some(0x59));
        assert_eq!(hid_usage("Numpad0"), Some(0x62));
        assert_eq!(hid_usage("NumpadEnter"), Some(0x58));
        assert_eq!(hid_usage("MetaLeft"), Some(0xE3));
        for none in ["", "Key", "Keya", "F13", "F0", "Digit", "AudioVolumeUp", "Fn"] {
            assert_eq!(hid_usage(none), None, "{none}");
        }
    }
}

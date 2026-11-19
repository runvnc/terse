//! winit physical keys to USB HID keyboard usage IDs (SPEC 5).

use winit::keyboard::{KeyCode, ModifiersState};

pub fn hid_usage(key: KeyCode) -> Option<u8> {
    use KeyCode::*;
    let letters = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, //
        KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    if let Some(i) = letters.iter().position(|&k| k == key) {
        return Some(0x04 + i as u8);
    }
    let digits = [Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0];
    if let Some(i) = digits.iter().position(|&k| k == key) {
        return Some(0x1E + i as u8);
    }
    let f_keys = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    if let Some(i) = f_keys.iter().position(|&k| k == key) {
        return Some(0x3A + i as u8);
    }
    let keypad = [Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, Numpad0];
    if let Some(i) = keypad.iter().position(|&k| k == key) {
        return Some(0x59 + i as u8);
    }
    Some(match key {
        Enter => 0x28,
        Escape => 0x29,
        Backspace => 0x2A,
        Tab => 0x2B,
        Space => 0x2C,
        Minus => 0x2D,
        Equal => 0x2E,
        BracketLeft => 0x2F,
        BracketRight => 0x30,
        Backslash => 0x31,
        Semicolon => 0x33,
        Quote => 0x34,
        Backquote => 0x35,
        Comma => 0x36,
        Period => 0x37,
        Slash => 0x38,
        CapsLock => 0x39,
        PrintScreen => 0x46,
        ScrollLock => 0x47,
        Pause => 0x48,
        Insert => 0x49,
        Home => 0x4A,
        PageUp => 0x4B,
        Delete => 0x4C,
        End => 0x4D,
        PageDown => 0x4E,
        ArrowRight => 0x4F,
        ArrowLeft => 0x50,
        ArrowDown => 0x51,
        ArrowUp => 0x52,
        NumLock => 0x53,
        NumpadDivide => 0x54,
        NumpadMultiply => 0x55,
        NumpadSubtract => 0x56,
        NumpadAdd => 0x57,
        NumpadEnter => 0x58,
        NumpadDecimal => 0x63,
        IntlBackslash => 0x64,
        ContextMenu => 0x65,
        ControlLeft => 0xE0,
        ShiftLeft => 0xE1,
        AltLeft => 0xE2,
        SuperLeft => 0xE3,
        ControlRight => 0xE4,
        ShiftRight => 0xE5,
        AltRight => 0xE6,
        SuperRight => 0xE7,
        _ => return None,
    })
}

/// Modifier bits: 0 Shift, 1 Control, 2 Alt, 3 Meta.
pub fn modifier_bits(m: ModifiersState) -> u8 {
    use terse_core::modifiers::*;
    let mut bits = 0;
    for (on, bit) in [(m.shift_key(), SHIFT), (m.control_key(), CONTROL), (m.alt_key(), ALT), (m.super_key(), META)] {
        if on {
            bits |= bit;
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_examples() {
        assert_eq!(hid_usage(KeyCode::KeyA), Some(0x04));
        assert_eq!(hid_usage(KeyCode::KeyZ), Some(0x1D));
        assert_eq!(hid_usage(KeyCode::Digit0), Some(0x27));
        assert_eq!(hid_usage(KeyCode::Enter), Some(0x28));
        assert_eq!(hid_usage(KeyCode::Escape), Some(0x29));
        assert_eq!(hid_usage(KeyCode::ArrowRight), Some(0x4F));
        assert_eq!(hid_usage(KeyCode::ArrowUp), Some(0x52));
        assert_eq!(hid_usage(KeyCode::F12), Some(0x45));
        assert_eq!(hid_usage(KeyCode::Numpad0), Some(0x62));
    }

    #[test]
    fn modifiers() {
        assert_eq!(modifier_bits(ModifiersState::SHIFT | ModifiersState::SUPER), 0b1001);
    }
}

//! winit physical keys to USB HID keyboard usage IDs (SPEC §events), and the
//! window input the hosts share: the home key, paste, the consent
//! prompt's answer.

use winit::event::KeyEvent;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

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

/// The key with HID usage `usage` (the inverse of [`hid_usage`]), for
/// `--drive`, which presses keys as the window system would.
pub fn key_code(usage: u8) -> Option<KeyCode> {
    use KeyCode::*;
    const ALL: &[KeyCode] = &[
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX,
        KeyY, KeyZ, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
        Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, Numpad0, Enter, Escape, Backspace, Tab, Space, Minus,
        Equal, BracketLeft, BracketRight, Backslash, Semicolon, Quote, Backquote, Comma, Period, Slash, CapsLock, Insert, Home, PageUp, Delete,
        End, PageDown, ArrowRight, ArrowLeft, ArrowDown, ArrowUp, NumpadDivide, NumpadMultiply, NumpadSubtract, NumpadAdd, NumpadEnter,
        NumpadDecimal, ControlLeft, ShiftLeft, AltLeft, SuperLeft, ControlRight, ShiftRight, AltRight, SuperRight,
    ];
    ALL.iter().copied().find(|&k| hid_usage(k) == Some(usage))
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

/// The most pasted characters held for delivery.
pub const MAX_PASTE: usize = 1 << 20;

/// The host's home key (SPEC §machines.input) when it runs a shell.
pub const HOME_KEY: KeyCode = KeyCode::F12;

/// The volume keys the host keeps (SPEC §events "Keys a host keeps"):
/// Ctrl+Alt+minus and Ctrl+Alt+equals (plus) a tenth down and up,
/// Ctrl+Alt+M mute, and the keyboard's own volume keys when the desktop
/// lets them reach the window. Physical keys, so every layout has them in
/// the same place.
pub fn volume_key(key: KeyCode, mods: ModifiersState) -> Option<crate::audio::VolumeKey> {
    use crate::audio::VolumeKey::*;
    match key {
        KeyCode::AudioVolumeDown => return Some(Down),
        KeyCode::AudioVolumeUp => return Some(Up),
        KeyCode::AudioVolumeMute => return Some(Mute),
        _ => {}
    }
    if !(mods.control_key() && mods.alt_key()) || mods.super_key() {
        return None;
    }
    match key {
        KeyCode::Minus | KeyCode::NumpadSubtract => Some(Down),
        KeyCode::Equal | KeyCode::NumpadAdd => Some(Up),
        KeyCode::KeyM => Some(Mute),
        _ => None,
    }
}

/// The text events a key press gives the program (SPEC §events): none for
/// a chord with Control (unless Alt is held too: Control+Alt is AltGr on
/// some systems) or with the logo key, whatever text the window system
/// reports for it (Wayland reports "c" for Ctrl+C, others U+0003), and
/// never a control character.
pub fn typed_text(text: Option<&str>, mods: ModifiersState) -> Vec<char> {
    if (mods.control_key() && !mods.alt_key()) || mods.super_key() {
        return Vec::new();
    }
    text.map(|t| t.chars().filter(|c| !c.is_control()).collect()).unwrap_or_default()
}

/// The characters of pasted text the program receives: tabs, newlines (a
/// CR LF pair or a lone CR counts as one) and no other control characters.
pub fn paste_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    let mut prev = '\0';
    text.chars().filter_map(move |c| {
        let out = match c {
            '\r' => Some('\n'),
            '\n' if prev == '\r' => None,
            '\n' => Some('\n'),
            '\t' => Some('\t'),
            c if c.is_control() => None,
            c => Some(c),
        };
        prev = c;
        out
    })
}

/// The answer a key press gives the consent prompt: Y yes; N or Esc no;
/// any other key (Enter included) nothing.
/// Notes that the release of the key `usage` is the host's, as its press
/// was: once, however often the press auto-repeats, so that one release
/// clears it and the key's next release reaches the program.
pub fn swallow(swallowed: &mut Vec<u8>, usage: Option<u8>) {
    if let Some(u) = usage.filter(|u| !swallowed.contains(u)) {
        swallowed.push(u);
    }
}

pub fn prompt_answer(event: &KeyEvent) -> Option<bool> {
    match &event.logical_key {
        Key::Named(NamedKey::Escape) => Some(false),
        Key::Character(c) if c.eq_ignore_ascii_case("y") => Some(true),
        Key::Character(c) if c.eq_ignore_ascii_case("n") => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The volume keys need Control and Alt together; a program keeps
    /// minus, equals and M with any other modifiers.
    #[test]
    fn volume_keys() {
        use crate::audio::VolumeKey::*;
        let ca = ModifiersState::CONTROL | ModifiersState::ALT;
        assert_eq!(volume_key(KeyCode::Minus, ca), Some(Down));
        assert_eq!(volume_key(KeyCode::Equal, ca | ModifiersState::SHIFT), Some(Up));
        assert_eq!(volume_key(KeyCode::KeyM, ca), Some(Mute));
        assert_eq!(volume_key(KeyCode::AudioVolumeMute, ModifiersState::empty()), Some(Mute));
        for m in [ModifiersState::empty(), ModifiersState::CONTROL, ModifiersState::ALT, ca | ModifiersState::SUPER] {
            for k in [KeyCode::Minus, KeyCode::Equal, KeyCode::KeyM] {
                assert_eq!(volume_key(k, m), None, "{k:?} {m:?}");
            }
        }
        assert_eq!(volume_key(KeyCode::KeyA, ca), None);
    }

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

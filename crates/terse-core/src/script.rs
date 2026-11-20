//! Scripted input for headless runs and tests (`--events <file>`).
//!
//! One event per line: a frame number, a command and its arguments. Blank
//! lines and text after `#` are ignored (except in `text`).
//!
//! ```text
//! 10 down <key> [shift] [ctrl] [alt] [meta]   key down (SPEC §events type 01h)
//! 10 up <key> [modifiers]                     key up (02h)
//! 10 press <key> [modifiers]                  key down and key up in the same frame
//! 10 text <rest of the line>                  one text event (03h) per character
//! 10 pointer <x> <y> [<buttons>]              pointer (04h) in screen units; buttons 0-7
//! 10 click <x> <y>                            pointer with the primary button, then released
//! 10 wheel <vertical> [<horizontal>]          wheel steps (06h); positive is down and right
//! 10 feeds-updated                            feeds updated (07h)
//! 10 home                                     the host's home key (SPEC §machines.input; a system only)
//! 10 pad <n> <buttons> <lx> <ly> [<rx> <ry>]    controller n (0-3) state (SPEC §controllers)
//! 10 pad <n> off                               controller n disconnected
//! ```
//!
//! A `pad` line connects controller `n` and sets its state from that frame
//! on, until the next `pad` line for it. `<buttons>` is hexadecimal (with or
//! without `0x`), byte 1 of the controller state in bits 0-7 and byte 2 in
//! bits 8-15 (`10` is A, `2000` is Start); the sticks are -127 to 127 and the
//! right stick defaults to 0 0. Controllers are disconnected until their
//! first `pad` line.
//!
//! `<key>` is a USB HID usage ID (decimal or `0x28`) or a name: `a`-`z`,
//! `0`-`9`, `f1`-`f12`, `enter`, `esc`, `backspace`, `tab`, `space`, `up`,
//! `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `insert`,
//! `delete`. Events of one frame are delivered in the order written.

use std::collections::BTreeMap;

use crate::system::{CONTROLLERS, Controller, Event, buttons, modifiers};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    frames: BTreeMap<u32, Vec<Event>>,
    /// Controller changes: from this frame on, controller n has this state.
    pads: BTreeMap<u32, Vec<(usize, Controller)>>,
}

/// The HID usage ID for a key name or number.
pub fn key_code(name: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase();
    if let Some(hex) = name.strip_prefix("0x") {
        return u8::from_str_radix(hex, 16).ok();
    }
    if name.len() > 1
        && let Ok(n) = name.parse::<u8>()
    {
        return Some(n);
    }
    let b = name.as_bytes();
    match b {
        [c @ b'a'..=b'z'] => return Some(0x04 + (c - b'a')),
        [b'0'] => return Some(0x27),
        [c @ b'1'..=b'9'] => return Some(0x1E + (c - b'1')),
        [b'f', ..] => {
            if let Ok(n @ 1..=12) = name[1..].parse::<u8>() {
                return Some(0x3A + n - 1);
            }
        }
        _ => {}
    }
    Some(match name.as_str() {
        "enter" => 0x28,
        "esc" | "escape" => 0x29,
        "backspace" => 0x2A,
        "tab" => 0x2B,
        "space" => 0x2C,
        "insert" => 0x49,
        "home" => 0x4A,
        "pageup" => 0x4B,
        "delete" => 0x4C,
        "end" => 0x4D,
        "pagedown" => 0x4E,
        "right" => 0x4F,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        _ => return None,
    })
}

fn modifier_bits(words: &[&str]) -> Result<u8, String> {
    words.iter().try_fold(0, |bits, w| {
        Ok(bits
            | match *w {
                "shift" => modifiers::SHIFT,
                "ctrl" | "control" => modifiers::CONTROL,
                "alt" => modifiers::ALT,
                "meta" => modifiers::META,
                _ => return Err(format!("unknown modifier {w}")),
            })
    })
}

fn number<T: std::str::FromStr>(word: Option<&&str>, what: &str) -> Result<T, String> {
    let w = word.ok_or(format!("missing {what}"))?;
    w.parse().map_err(|_| format!("bad {what} {w}"))
}

/// `<n> <buttons> <lx> <ly> [<rx> <ry>]` or `<n> off`.
fn parse_pad(words: &[&str]) -> Result<(usize, Controller), String> {
    let n: usize = number(words.first(), "controller")?;
    if n >= CONTROLLERS {
        return Err(format!("bad controller {n}"));
    }
    if words.get(1) == Some(&"off") && words.len() == 2 {
        return Ok((n, Controller::default()));
    }
    let b = words.get(1).ok_or("missing buttons")?;
    let buttons = u16::from_str_radix(b.trim_start_matches("0x"), 16).map_err(|_| format!("bad buttons {b}"))?;
    let axis = |i: usize, what: &str| -> Result<i8, String> {
        let v: i8 = number(words.get(i), what)?;
        if v == -128 { Err(format!("bad {what} -128")) } else { Ok(v) }
    };
    let left = (axis(2, "lx")?, axis(3, "ly")?);
    let right = match words.len() {
        4 => (0, 0),
        6 => (axis(4, "rx")?, axis(5, "ry")?),
        _ => return Err("pad wants <n> <buttons> <lx> <ly> [<rx> <ry>]".into()),
    };
    Ok((n, Controller { connected: true, buttons, left, right }))
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut script = Script::default();
        for (i, line) in text.lines().enumerate() {
            script.parse_line(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        }
        Ok(script)
    }

    fn parse_line(&mut self, line: &str) -> Result<(), String> {
        let trimmed = line.trim_start();
        let (frame, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
        if frame.is_empty() || frame.starts_with('#') {
            return Ok(());
        }
        let frame: u32 = frame.parse().map_err(|_| format!("bad frame number {frame}"))?;
        let rest = rest.trim_start();
        let (command, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if command == "pad" {
            let words: Vec<&str> = args.split('#').next().unwrap_or("").split_whitespace().collect();
            let pad = parse_pad(&words)?;
            self.pads.entry(frame).or_default().push(pad);
            return Ok(());
        }
        let events = self.frames.entry(frame).or_default();
        if command == "text" {
            // The rest of the line, exactly (after one separating space).
            events.extend(args.chars().map(Event::text));
            return Ok(());
        }
        let words: Vec<&str> = args.split('#').next().unwrap_or("").split_whitespace().collect();
        let key = || -> Result<u8, String> {
            let name = words.first().ok_or("missing key")?;
            key_code(name).ok_or(format!("unknown key {name}"))
        };
        match command {
            "down" => events.push(Event::key_down(key()?, modifier_bits(&words[1..])?)),
            "up" => events.push(Event::key_up(key()?, modifier_bits(&words[1..])?)),
            "press" => {
                let (k, m) = (key()?, modifier_bits(&words[1..])?);
                events.extend([Event::key_down(k, m), Event::key_up(k, m)]);
            }
            "pointer" => {
                let b = if words.len() > 2 { number(words.get(2), "buttons")? } else { 0 };
                events.push(Event::pointer(number(words.first(), "x")?, number(words.get(1), "y")?, b));
            }
            "click" => {
                let (x, y) = (number(words.first(), "x")?, number(words.get(1), "y")?);
                events.extend([Event::pointer(x, y, buttons::PRIMARY), Event::pointer(x, y, 0)]);
            }
            "wheel" => {
                let h = if words.len() > 1 { number(words.get(1), "horizontal steps")? } else { 0 };
                events.push(Event::wheel(number(words.first(), "vertical steps")?, h));
            }
            "feeds-updated" => events.push(Event::feeds_updated()),
            "home" => events.push(Event::home()),
            _ => return Err(format!("unknown command {command:?}")),
        }
        Ok(())
    }

    /// The controller state for `frame`: each controller as set by its last
    /// `pad` line at or before `frame`.
    pub fn controllers(&self, frame: u32) -> [Controller; CONTROLLERS] {
        let mut out = [Controller::default(); CONTROLLERS];
        for changes in self.pads.range(..=frame).map(|(_, v)| v) {
            for &(n, c) in changes {
                out[n] = c;
            }
        }
        out
    }

    /// The events for `frame`, in the order written.
    pub fn events(&self, frame: u32) -> &[Event] {
        self.frames.get(&frame).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        let s = Script::parse(
            "# a comment\n\
             \n\
             3 press enter\n\
             3 down 0x52 shift ctrl  # trailing comment\n\
             4 up a meta\n\
             5 text 你好 #x\n\
             6 pointer 639 479 5\n\
             6 click 8 16\n\
             7 wheel -2\n\
             7 wheel 1 -1\n\
             8 feeds-updated\n\
             8 home\n\
             2 press f12\n",
        )
        .unwrap();
        assert_eq!(s.events(3), [Event::key_down(0x28, 0), Event::key_up(0x28, 0), Event::key_down(0x52, 3)]);
        assert_eq!(s.events(4), [Event::key_up(0x04, 8)]);
        let text: Vec<Event> = "你好 #x".chars().map(Event::text).collect();
        assert_eq!(s.events(5), text);
        assert_eq!(s.events(6), [Event::pointer(639, 479, 5), Event::pointer(8, 16, 1), Event::pointer(8, 16, 0)]);
        assert_eq!(s.events(7), [Event::wheel(-2, 0), Event::wheel(1, -1)]);
        assert_eq!(s.events(8), [Event::feeds_updated(), Event::home()]);
        assert_eq!(s.events(2), [Event::key_down(0x45, 0), Event::key_up(0x45, 0)]);
        assert_eq!(s.events(9), []);
    }

    #[test]
    fn pad_state_persists_until_changed() {
        let s = Script::parse("5 pad 0 10 -127 0\n9 pad 0 0x2001 5 -6 7 127 # start, up\n9 pad 3 0 0 0\n12 pad 0 off\n").unwrap();
        assert_eq!(s.controllers(4), [Controller::default(); 4]);
        let a = Controller { connected: true, buttons: 0x10, left: (-127, 0), right: (0, 0) };
        assert_eq!(s.controllers(5)[0], a);
        assert_eq!(s.controllers(8)[0], a);
        let b = Controller { connected: true, buttons: 0x2001, left: (5, -6), right: (7, 127) };
        assert_eq!(s.controllers(9)[0], b);
        assert_eq!(s.controllers(11)[3], Controller { connected: true, ..Default::default() });
        assert_eq!(s.controllers(12)[0], Controller::default());
        assert!(s.events(5).is_empty());
        for bad in ["1 pad 4 0 0 0", "1 pad 0 xyz 0 0", "1 pad 0 0 -128 0", "1 pad 0 0 0", "1 pad 0 0 0 0 1"] {
            assert!(Script::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn key_names() {
        for (name, code) in [("a", 4), ("Z", 0x1D), ("1", 0x1E), ("0", 0x27), ("esc", 0x29), ("pagedown", 0x4E), ("40", 40)] {
            assert_eq!(key_code(name), Some(code), "{name}");
        }
        assert_eq!(key_code("f13"), None);
        assert_eq!(key_code("nope"), None);
    }

    #[test]
    fn errors_name_the_line() {
        assert_eq!(Script::parse("1 press enter\nx press a").unwrap_err(), "line 2: bad frame number x");
        assert!(Script::parse("1 jump").unwrap_err().contains("unknown command"));
        assert!(Script::parse("1 press nokey").unwrap_err().contains("unknown key"));
        assert!(Script::parse("1 wheel 200").unwrap_err().contains("bad vertical steps"));
        assert!(Script::parse("1 down a hyper").unwrap_err().contains("unknown modifier"));
    }
}

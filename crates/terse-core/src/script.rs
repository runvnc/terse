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
//! 10 pointer <x> <y> [<buttons>]              pointer (04h) in screen units, clamped; buttons 0-7
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
//! A line may also leave out its frame number: it then belongs to the
//! current frame, which is the frame of the line before it (0 at the start),
//! moved on by `wait <n>` lines; `type <text>` types its text one character
//! a frame as a keyboard does (each character's key down, its text, its key
//! up) and moves the current frame past it. Commands of `terse-host --drive`
//! (pictures, dumps, window size; STATE.md 6.5) are accepted and kept apart,
//! in [`Script::host_commands`], for the hosts that carry them out.
//!
//! The format is specified in STATE.md 6.2. `<key>` is a USB HID usage ID
//! (two or more decimal digits, or `0x28`) or a name: `a`-`z`,
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
    /// Lines for `terse-host --drive` (STATE.md 6.5), with their frames.
    host: Vec<(u32, String)>,
}

/// The commands of the events file proper (STATE.md 6.2).
pub const INPUT_COMMANDS: [&str; 10] = ["down", "up", "press", "text", "pointer", "click", "wheel", "feeds-updated", "home", "pad"];

/// The commands of `terse-host --drive` that are not input (STATE.md 6.5):
/// a headless `terse run` skips them.
pub const HOST_COMMANDS: [&str; 18] = [
    "shot", "dump", "settle", "watch", "expect", "resize", "pointer-px", "click-px", "drag", "drag-px", "perf", "budget", "quit", "say",
    "sleep", "renderer", "repeat", "hold",
];

/// The frame a line belongs to (STATE.md 6.2 and its extension): numbered
/// lines name it, others take the current one; `wait` moves it on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timeline {
    pub frame: u32,
}

/// A line of an events file split up: its frame (`None`: none written),
/// command and the rest of the line. `None` for a blank line or a comment.
pub fn split_line(line: &str) -> Result<Option<(Option<u32>, &str, &str)>, String> {
    let trimmed = line.trim_start();
    let (first, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
    if first.is_empty() || first.starts_with('#') {
        return Ok(None);
    }
    let (frame, rest) = match first.parse::<u32>() {
        Ok(f) => (Some(f), rest.trim_start()),
        Err(_) if first == "wait" || first == "type" || INPUT_COMMANDS.contains(&first) || HOST_COMMANDS.contains(&first) => {
            (None, trimmed)
        }
        Err(_) => return Err(format!("bad frame number {first}")),
    };
    let (command, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if command.is_empty() {
        return Err("missing command".into());
    }
    Ok(Some((frame, command, args)))
}

/// The words of a line's arguments, before any `#` comment.
pub fn words(args: &str) -> Vec<&str> {
    args.split('#').next().unwrap_or("").split_whitespace().collect()
}

/// The key a US keyboard types `c` with, and whether with Shift: for
/// `type`, which gives each character its key as a keyboard would.
pub fn char_key(c: char) -> Option<(u8, bool)> {
    const PLAIN: &str = "-=[]\\;'`,./";
    const SHIFTED: &str = "_+{}|:\"~<>?";
    const USAGES: [u8; 11] = [0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38];
    match c {
        'a'..='z' => Some((0x04 + (c as u8 - b'a'), false)),
        'A'..='Z' => Some((0x04 + (c as u8 - b'A'), true)),
        '1'..='9' => Some((0x1E + (c as u8 - b'1'), false)),
        '0' => Some((0x27, false)),
        ' ' => Some((0x2C, false)),
        '\n' => Some((0x28, false)),
        '\t' => Some((0x2B, false)),
        _ => {
            if let Some(i) = "!@#$%^&*()".find(c) {
                return Some((if i == 9 { 0x27 } else { 0x1E + i as u8 }, true));
            }
            if let Some(i) = PLAIN.chars().position(|p| p == c) {
                return Some((USAGES[i], false));
            }
            SHIFTED.chars().position(|p| p == c).map(|i| (USAGES[i], true))
        }
    }
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
        // The modifier keys themselves (a window sends them too).
        "lctrl" => 0xE0,
        "lshift" => 0xE1,
        "lalt" => 0xE2,
        "lmeta" => 0xE3,
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
        let mut at = Timeline::default();
        for (i, line) in text.lines().enumerate() {
            script.parse_line(line, &mut at).map_err(|e| format!("line {}: {e}", i + 1))?;
        }
        Ok(script)
    }

    /// The `terse-host --drive` lines (STATE.md 6.5), each with its frame,
    /// in the order written; a headless run leaves them out.
    pub fn host_commands(&self) -> &[(u32, String)] {
        &self.host
    }

    fn parse_line(&mut self, line: &str, at: &mut Timeline) -> Result<(), String> {
        let Some((frame, command, args)) = split_line(line)? else { return Ok(()) };
        if let Some(f) = frame {
            at.frame = f;
        }
        let frame = at.frame;
        if command == "wait" {
            let n: u32 = number(words(args).first(), "frame count")?;
            at.frame = at.frame.checked_add(n).ok_or("frame count too large")?;
            return Ok(());
        }
        if command == "type" {
            // As a keyboard types it: a frame a character, each its key
            // down (with Shift where a US keyboard needs it), its text and
            // its key up; a character no key gives is its text alone.
            for c in args.chars() {
                let events = self.frames.entry(at.frame).or_default();
                match char_key(c) {
                    Some((k, shift)) => {
                        let m = if shift { modifiers::SHIFT } else { 0 };
                        events.push(Event::key_down(k, m));
                        if !c.is_control() {
                            events.push(Event::text(c));
                        }
                        events.push(Event::key_up(k, m));
                    }
                    None => events.push(Event::text(c)),
                }
                at.frame += 1;
            }
            return Ok(());
        }
        if HOST_COMMANDS.contains(&command) {
            self.host.push((frame, line.trim().to_string()));
            return Ok(());
        }
        if command == "pad" {
            let pad = parse_pad(&words(args))?;
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
                let b: u8 = if words.len() > 2 { number(words.get(2), "buttons")? } else { 0 };
                if b > 7 {
                    return Err(format!("bad buttons {b} (0-7)"));
                }
                events.push(Event::pointer(number(words.first(), "x")?, number(words.get(1), "y")?, b));
            }
            "click" => {
                let (x, y) = (number(words.first(), "x")?, number(words.get(1), "y")?);
                events.extend([Event::pointer(x, y, buttons::PRIMARY), Event::pointer(x, y, 0)]);
            }
            "wheel" => {
                // SPEC §events: each -127 to 127.
                let steps = |w: Option<&&str>, what: &str| -> Result<i8, String> {
                    let v: i8 = number(w, what)?;
                    if v == -128 { Err(format!("bad {what} -128")) } else { Ok(v) }
                };
                let h = if words.len() > 1 { steps(words.get(1), "horizontal steps")? } else { 0 };
                events.push(Event::wheel(steps(words.first(), "vertical steps")?, h));
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

    /// STATE.md 6.2's extension: lines without a frame number take the
    /// current frame, `wait` moves it on, `type` types a character a frame,
    /// and the drive's lines are kept apart.
    #[test]
    fn sequential_lines_wait_and_type() {
        let s = Script::parse("press a\nwait 2\npress b\n10 press c\nwait 1\ntype aB!\nshot x.png\npress d\n").unwrap();
        assert_eq!(s.events(0), [Event::key_down(4, 0), Event::key_up(4, 0)]);
        assert_eq!(s.events(2), [Event::key_down(5, 0), Event::key_up(5, 0)]);
        assert_eq!(s.events(10), [Event::key_down(6, 0), Event::key_up(6, 0)]);
        assert_eq!(s.events(11), [Event::key_down(4, 0), Event::text('a'), Event::key_up(4, 0)]);
        assert_eq!(s.events(12), [Event::key_down(5, 1), Event::text('B'), Event::key_up(5, 1)]);
        assert_eq!(s.events(13), [Event::key_down(0x1E, 1), Event::text('!'), Event::key_up(0x1E, 1)]);
        assert_eq!(s.events(14), [Event::key_down(7, 0), Event::key_up(7, 0)]);
        assert_eq!(s.host_commands(), [(14, "shot x.png".to_string())]);
        assert_eq!(char_key('='), Some((0x2E, false)));
        assert_eq!(char_key('"'), Some((0x34, true)));
        assert_eq!(char_key('é'), None);
        assert_eq!(key_code("lshift"), Some(0xE1));
    }

    #[test]
    fn key_names() {
        for (name, code) in [("a", 4), ("Z", 0x1D), ("1", 0x1E), ("0", 0x27), ("5", 0x22), ("05", 5), ("esc", 0x29), ("Escape", 0x29), ("pagedown", 0x4E), ("40", 40), ("0x28", 0x28), ("F12", 0x45)] {
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
        // STATE.md 6.2: the ranges of SPEC §events.
        assert!(Script::parse("1 wheel -128").unwrap_err().contains("bad vertical steps"));
        assert!(Script::parse("1 wheel 1 -128").unwrap_err().contains("bad horizontal steps"));
        assert!(Script::parse("1 pointer 1 2 8").unwrap_err().contains("bad buttons"));
        assert!(Script::parse("1 pointer -1 2").unwrap_err().contains("bad x"));
        assert!(Script::parse("1 wheel -127 127\n1 pointer 1 2 7").is_ok());
        assert!(Script::parse("1 down a hyper").unwrap_err().contains("unknown modifier"));
    }
}

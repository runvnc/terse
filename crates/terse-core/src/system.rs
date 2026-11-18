//! Device 1, System (SPEC 5).

use std::collections::VecDeque;

pub const QUEUE_CAPACITY: usize = 64;
/// Screen size in screen units (SPEC 5.4).
pub const SCREEN_WIDTH: u16 = 640;
pub const SCREEN_HEIGHT: u16 = 480;
/// Number of game controllers (SPEC 5.3).
pub const CONTROLLERS: usize = 4;

/// One input event (SPEC 5.1): a type byte and three parameter bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: u8,
    pub a: u8,
    pub b: u8,
    pub c: u8,
}

pub mod modifiers {
    pub const SHIFT: u8 = 1 << 0;
    pub const CONTROL: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;
    pub const META: u8 = 1 << 3;
}

pub mod buttons {
    pub const PRIMARY: u8 = 1 << 0;
    pub const SECONDARY: u8 = 1 << 1;
    pub const MIDDLE: u8 = 1 << 2;
}

impl Event {
    pub const KEY_DOWN: u8 = 0x01;
    pub const KEY_UP: u8 = 0x02;
    pub const TEXT: u8 = 0x03;
    pub const POINTER: u8 = 0x04;
    pub const CONTENT_DONE: u8 = 0x05;
    pub const WHEEL: u8 = 0x06;
    pub const FEEDS_UPDATED: u8 = 0x07;

    /// `key` is a USB HID keyboard usage ID.
    pub fn key_down(key: u8, modifiers: u8) -> Self {
        Event { kind: Self::KEY_DOWN, a: key, b: modifiers, c: 0 }
    }

    pub fn key_up(key: u8, modifiers: u8) -> Self {
        Event { kind: Self::KEY_UP, a: key, b: modifiers, c: 0 }
    }

    pub fn text(ch: char) -> Self {
        let v = ch as u32;
        Event { kind: Self::TEXT, a: v as u8, b: (v >> 8) as u8, c: (v >> 16) as u8 }
    }

    /// A pointer position in screen units (SPEC 5.4), clamped to 0-639 and
    /// 0-479, and the button bits (`buttons::*`).
    pub fn pointer(x: u16, y: u16, buttons: u8) -> Self {
        let (x, y) = (x.min(SCREEN_WIDTH - 1), y.min(SCREEN_HEIGHT - 1));
        let c = (x >> 8) as u8 & 0x03 | ((y >> 8) as u8 & 0x01) << 2 | (buttons & 0x07) << 3;
        Event { kind: Self::POINTER, a: x as u8, b: y as u8, c }
    }

    /// The position and buttons of a pointer event (the inverse of `pointer`).
    pub fn pointer_parts(&self) -> (u16, u16, u8) {
        let x = self.a as u16 | ((self.c & 0x03) as u16) << 8;
        let y = self.b as u16 | ((self.c >> 2 & 0x01) as u16) << 8;
        (x, y, self.c >> 3 & 0x07)
    }

    /// Wheel steps; positive is down (towards later content) and right.
    pub fn wheel(vertical: i8, horizontal: i8) -> Self {
        Event { kind: Self::WHEEL, a: vertical as u8, b: horizontal as u8, c: 0 }
    }

    /// The host stored new feed entries since the previous frame (SPEC 7.7).
    pub fn feeds_updated() -> Self {
        Event { kind: Self::FEEDS_UPDATED, a: 0, b: 0, c: 0 }
    }

    /// A content request finished (SPEC 7.3).
    pub fn content_done(handle: u8, status: u8) -> Self {
        Event { kind: Self::CONTENT_DONE, a: handle, b: status, c: 0 }
    }
}

/// One game controller's state for a frame (SPEC 5.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controller {
    pub connected: bool,
    /// Byte 1 in bits 0-7 (`pad::UP` …) and byte 2 in bits 8-15
    /// (`pad::L1` …).
    pub buttons: u16,
    /// Sticks, x then y; −127 is left or up, 127 right or down (−128 reads as −127).
    pub left: (i8, i8),
    pub right: (i8, i8),
}

/// Controller button bits for `Controller::buttons` (SPEC 5.3 bytes 1 and 2).
pub mod pad {
    pub const UP: u16 = 1 << 0;
    pub const DOWN: u16 = 1 << 1;
    pub const LEFT: u16 = 1 << 2;
    pub const RIGHT: u16 = 1 << 3;
    pub const A: u16 = 1 << 4;
    pub const B: u16 = 1 << 5;
    pub const X: u16 = 1 << 6;
    pub const Y: u16 = 1 << 7;
    pub const L1: u16 = 1 << 8;
    pub const R1: u16 = 1 << 9;
    pub const L2: u16 = 1 << 10;
    pub const R2: u16 = 1 << 11;
    pub const SELECT: u16 = 1 << 12;
    pub const START: u16 = 1 << 13;
    pub const L3: u16 = 1 << 14;
    pub const R3: u16 = 1 << 15;
}

impl Controller {
    /// The eight bytes ports 1Dh/1Eh read; all zero when not connected.
    pub fn bytes(&self) -> [u8; 8] {
        if !self.connected {
            return [0; 8];
        }
        let axis = |v: i8| v.max(-127) as u8;
        let [lo, hi] = self.buttons.to_le_bytes();
        [1, lo, hi, axis(self.left.0), axis(self.left.1), axis(self.right.0), axis(self.right.1), 0]
    }
}

pub struct System {
    queue: VecDeque<Event>,
    frame: u32,
    time: u32,
    random: Vec<u8>,
    random_used: usize,
    exit: Option<u8>,
    /// Controller bytes latched for this frame (SPEC 5.3).
    controllers: [[u8; 8]; CONTROLLERS],
    /// The selected controller, if 0-3 was last written to port 1Dh (reset: 0).
    selected: Option<u8>,
    /// Next byte read through port 1Eh.
    pad_byte: u8,
}

impl Default for System {
    fn default() -> Self {
        System {
            queue: VecDeque::new(),
            frame: 0,
            time: 0,
            random: Vec::new(),
            random_used: 0,
            exit: None,
            controllers: [[0; 8]; CONTROLLERS],
            // SPEC 5.3: after reset, controller 0 is selected.
            selected: Some(0),
            pad_byte: 0,
        }
    }
}

impl System {
    /// Appends an event; returns false if it was dropped because the queue is full.
    pub fn push_event(&mut self, event: Event) -> bool {
        if self.queue.len() >= QUEUE_CAPACITY {
            return false;
        }
        self.queue.push_back(event);
        true
    }

    /// Latches the clock, this frame's random bytes and the controllers
    /// (SPEC 1.3 step 1, 5.3).
    pub fn latch(&mut self, unix_time: u32, random: &[u8], controllers: &[Controller; CONTROLLERS]) {
        self.controllers = controllers.map(|c| c.bytes());
        self.time = unix_time;
        self.random.clear();
        self.random.extend_from_slice(random);
        self.random_used = 0;
    }

    /// Random bytes consumed this frame (never more than were supplied).
    pub fn random_used(&self) -> usize {
        self.random_used.min(self.random.len())
    }

    pub fn end_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    pub fn port_in(&mut self, reg: u8) -> u8 {
        let head = self.queue.front().copied();
        match reg {
            0x0 => head.map_or(0, |e| e.kind),
            0x1 => head.map_or(0, |e| e.a),
            0x2 => head.map_or(0, |e| e.b),
            0x3 => head.map_or(0, |e| e.c),
            0x4..=0x7 => self.frame.to_le_bytes()[reg as usize - 0x4],
            0x8..=0xB => self.time.to_le_bytes()[reg as usize - 0x8],
            0xC => {
                // Reads beyond the supplied bytes return 00h.
                let b = self.random.get(self.random_used).copied().unwrap_or(0);
                self.random_used += 1;
                b
            }
            0xE => {
                let b = self.selected.map_or(0, |c| self.controllers[c as usize][self.pad_byte as usize]);
                self.pad_byte = (self.pad_byte + 1) % 8;
                b
            }
            _ => 0xFF,
        }
    }

    pub fn port_out(&mut self, reg: u8, value: u8) {
        match reg {
            0x0 => {
                self.queue.pop_front();
            }
            0xD => {
                self.selected = (value < CONTROLLERS as u8).then_some(value);
                self.pad_byte = 0;
            }
            0xF => self.exit = Some(value),
            _ => {}
        }
    }

    pub fn queue(&self) -> impl ExactSizeIterator<Item = &Event> {
        self.queue.iter()
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    pub fn exit_code(&self) -> Option<u8> {
        self.exit
    }

    /// The frame counter and exit state (STATE.md, machine section).
    pub(crate) fn write_frame_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.frame.to_le_bytes());
        out.extend_from_slice(&[self.exit.is_some() as u8, self.exit.unwrap_or(0)]);
    }

    /// The latches and the event queue (STATE.md, system section). This
    /// frame's random bytes are an input consumed within the frame and are
    /// not included.
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.time.to_le_bytes());
        out.push(self.queue.len() as u8);
        for e in &self.queue {
            out.extend_from_slice(&[e.kind, e.a, e.b, e.c]);
        }
        for c in &self.controllers {
            out.extend_from_slice(c);
        }
        out.push(self.selected.unwrap_or(0xFF));
        // With no controller selected the read position cannot be observed
        // (selecting one restarts it), so it is written as 0.
        out.push(if self.selected.is_some() { self.pad_byte } else { 0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_head_and_remove() {
        let mut s = System::default();
        assert_eq!(s.port_in(0x0), 0);
        assert_eq!(s.port_in(0x1), 0);
        s.push_event(Event::key_down(0x29, modifiers::SHIFT));
        s.push_event(Event::text('你'));
        assert_eq!((s.port_in(0), s.port_in(1), s.port_in(2)), (1, 0x29, 1));
        s.port_out(0, 0);
        assert_eq!(s.port_in(0), 3);
        let cp = '你' as u32;
        assert_eq!(s.port_in(1), cp as u8);
        assert_eq!(s.port_in(2), (cp >> 8) as u8);
        assert_eq!(s.port_in(3), 0);
        s.port_out(0, 0);
        s.port_out(0, 0); // removing from an empty queue is harmless
        assert_eq!(s.port_in(0), 0);
    }

    #[test]
    fn text_event_above_bmp() {
        let e = Event::text('😀'); // U+1F600
        assert_eq!((e.a, e.b, e.c), (0x00, 0xF6, 0x01));
    }

    #[test]
    fn queue_overflow_drops_new_events() {
        let mut s = System::default();
        for i in 0..QUEUE_CAPACITY {
            assert!(s.push_event(Event::key_down(i as u8, 0)));
        }
        assert!(!s.push_event(Event::key_down(0xEE, 0)));
        assert_eq!(s.queue().len(), QUEUE_CAPACITY);
        assert_eq!(s.queue().last().unwrap().a, (QUEUE_CAPACITY - 1) as u8);
        assert_eq!(s.port_in(1), 0, "head is the oldest event");
    }

    #[test]
    fn counters_time_random_exit() {
        let mut s = System::default();
        s.latch(0x1234_5678, &[0xAB], &Default::default());
        s.end_frame();
        s.end_frame();
        assert_eq!(s.port_in(0x4), 2);
        assert_eq!(s.port_in(0x7), 0);
        assert_eq!(s.port_in(0x8), 0x78);
        assert_eq!(s.port_in(0xB), 0x12);
        assert_eq!(s.port_in(0xC), 0xAB);
        assert_eq!(s.port_in(0xD), 0xFF);
        assert_eq!(s.exit_code(), None);
        s.port_out(0xF, 3);
        assert_eq!(s.exit_code(), Some(3));
    }

    #[test]
    fn pointer_packing_at_the_extremes() {
        let cases = [
            ((0, 0, 0), (0x00, 0x00, 0x00)),
            ((639, 479, 0b111), (0x7F, 0xDF, 0x02 | 0x04 | 0x38)),
            ((256, 256, buttons::PRIMARY), (0x00, 0x00, 0x01 | 0x04 | 0x08)),
            ((255, 255, buttons::MIDDLE), (0xFF, 0xFF, 0x20)),
            ((512, 0, buttons::SECONDARY), (0x00, 0x00, 0x02 | 0x10)),
            ((767, 300, 0), (0x7F, 0x2C, 0x02 | 0x04)), // x clamped to 639
            ((u16::MAX, u16::MAX, 0xFF), (0x7F, 0xDF, 0x02 | 0x04 | 0x38)),
        ];
        for ((x, y, b), (a, bb, c)) in cases {
            let e = Event::pointer(x, y, b);
            assert_eq!((e.kind, e.a, e.b, e.c), (Event::POINTER, a, bb, c), "({x}, {y}, {b})");
            assert_eq!(e.pointer_parts(), (x.min(639), y.min(479), b & 7));
        }
    }

    #[test]
    fn wheel_and_feeds_updated() {
        let e = Event::wheel(-3, 1);
        assert_eq!((e.kind, e.a, e.b, e.c), (0x06, 0xFD, 0x01, 0));
        let e = Event::wheel(127, -128);
        assert_eq!((e.a, e.b), (0x7F, 0x80));
        let e = Event::feeds_updated();
        assert_eq!((e.kind, e.a, e.b, e.c), (0x07, 0, 0, 0));
    }

    fn read_pad(s: &mut System, n: usize) -> Vec<u8> {
        (0..n).map(|_| s.port_in(0xE)).collect()
    }

    #[test]
    fn controller_port_sequence_and_wraparound() {
        let mut s = System::default();
        let mut pads = [Controller::default(); CONTROLLERS];
        pads[0] = Controller { connected: true, buttons: pad::UP | pad::A | pad::START, left: (-128, 127), right: (5, -5) };
        pads[2] = Controller { connected: true, buttons: 0xFFFF, left: (0, 0), right: (0, 0) };
        pads[3] = Controller { connected: false, buttons: 0xFFFF, left: (9, 9), right: (9, 9) };
        s.latch(0, &[], &pads);
        // Controller 0 is selected after reset; byte 7 wraps to byte 0.
        let want0 = [1, 0x11, 0x20, 0x81, 0x7F, 5, 0xFB, 0];
        assert_eq!(read_pad(&mut s, 10), [&want0[..], &want0[..2]].concat());
        // Selecting restarts at byte 0, even mid-sequence.
        s.port_out(0xD, 2);
        assert_eq!(read_pad(&mut s, 3), [1, 0xFF, 0xFF]);
        s.port_out(0xD, 2);
        assert_eq!(read_pad(&mut s, 1), [1]);
        // Not connected (whatever the rest of its state), and no controller.
        s.port_out(0xD, 1);
        assert_eq!(read_pad(&mut s, 8), [0; 8]);
        s.port_out(0xD, 3);
        assert_eq!(read_pad(&mut s, 8), [0; 8]);
        for none in [4, 0x80, 0xFF] {
            s.port_out(0xD, none);
            assert_eq!(read_pad(&mut s, 9), [0; 9], "select {none}");
        }
        // Port 1Dh is write-only and 1Eh read-only.
        assert_eq!(s.port_in(0xD), 0xFF);
        s.port_out(0xE, 3);
        s.port_out(0xD, 0);
        assert_eq!(read_pad(&mut s, 1), [1]);
    }

    #[test]
    fn controller_state_is_latched_per_frame() {
        let mut s = System::default();
        let mut pads = [Controller::default(); CONTROLLERS];
        pads[0] = Controller { connected: true, buttons: pad::B, ..Default::default() };
        s.latch(0, &[], &pads);
        assert_eq!(read_pad(&mut s, 2), [1, 0x20]);
        // The host's state changes within the frame: nothing changes until
        // the next latch, and the read position carries across frames.
        pads[0].buttons = pad::DOWN;
        assert_eq!(read_pad(&mut s, 7), [0, 0, 0, 0, 0, 0, 1]);
        s.end_frame();
        s.latch(0, &[], &pads);
        assert_eq!(read_pad(&mut s, 1), [0x02]);
        pads[0].connected = false;
        s.latch(0, &[], &pads);
        s.port_out(0xD, 0);
        assert_eq!(read_pad(&mut s, 8), [0; 8]);
    }
}

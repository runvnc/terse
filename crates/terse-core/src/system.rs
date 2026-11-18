//! Device 1, System (SPEC 5).

use std::collections::VecDeque;

pub const QUEUE_CAPACITY: usize = 64;

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

    pub fn pointer(column: u8, row: u8, buttons: u8) -> Self {
        Event { kind: Self::POINTER, a: column, b: row, c: buttons }
    }

    /// A content request finished (SPEC 7.3).
    pub fn content_done(handle: u8, status: u8) -> Self {
        Event { kind: Self::CONTENT_DONE, a: handle, b: status, c: 0 }
    }
}

#[derive(Default)]
pub struct System {
    queue: VecDeque<Event>,
    frame: u32,
    time: u32,
    random: Vec<u8>,
    random_used: usize,
    exit: Option<u8>,
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

    /// Latches the clock and this frame's random bytes (SPEC 1.3 step 1).
    pub fn latch(&mut self, unix_time: u32, random: &[u8]) {
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
            _ => 0xFF,
        }
    }

    pub fn port_out(&mut self, reg: u8, value: u8) {
        match reg {
            0x0 => {
                self.queue.pop_front();
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
        s.latch(0x1234_5678, &[0xAB]);
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
}

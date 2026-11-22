use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

const COLUMNS: usize = 80;
const ROWS: usize = 30;
const NONE: u8 = 0xff;
const DEFERRED: u8 = 2;
const BUFFERED: u8 = 4;
const READS: u8 = 1;
const IN_ROM: u8 = 4;
const FAILED: u32 = 0xffff_fffe;

struct Entry {
    function: u8,
    flags: u8,
    buffer: u8,
    length: u8,
    scale: u32,
    most: u32,
    access: u8,
    checked: u8,
    checked_most: u32,
    rule: u8,
    rule_arg: u8,
    per_four: u8,
    charge_arg: u8,
    per_byte: u16,
    low: u32,
    high: u32,
    add: u32,
}

const PLAIN: Entry = Entry {
    function: 0,
    flags: DEFERRED,
    buffer: 0,
    length: NONE,
    scale: 0,
    most: 0,
    access: 0,
    checked: NONE,
    checked_most: 0,
    rule: 1,
    rule_arg: 0,
    per_four: 0,
    charge_arg: NONE,
    per_byte: 0,
    low: 0,
    high: 0,
    add: 0,
};

const TABLE: [Entry; 5] = [
    Entry {
        function: 0,
        flags: DEFERRED | BUFFERED,
        length: 1,
        scale: 1,
        most: 4096,
        access: READS | IN_ROM,
        rule: 2,
        rule_arg: 1,
        per_byte: 2,
        ..PLAIN
    },
    Entry {
        function: 1,
        charge_arg: 0,
        low: 1,
        high: 3,
        add: 150,
        ..PLAIN
    },
    Entry { function: 2, ..PLAIN },
    Entry { function: 4, ..PLAIN },
    Entry {
        function: 7,
        flags: DEFERRED | BUFFERED,
        buffer: 1,
        length: 2,
        scale: 4,
        most: 9600,
        access: READS | IN_ROM,
        checked: 0,
        checked_most: 2399,
        rule: 2,
        rule_arg: 2,
        per_four: 1,
        ..PLAIN
    },
];

impl Entry {
    fn encode(&self) -> Vec<u8> {
        let e = self;
        let mut b =
            vec![e.function, e.flags, e.buffer, e.length];
        words(&mut b, &[e.scale, 0, e.most]);
        b.extend([e.access, e.checked, 0, 0]);
        words(&mut b, &[e.checked_most, FAILED, FAILED]);
        b.extend([e.rule, e.rule_arg, 0, 0]);
        words(&mut b, &[0, 0, 0]);
        b.extend([e.per_four, e.charge_arg]);
        b.extend(e.per_byte.to_le_bytes());
        words(&mut b, &[e.low, e.high, e.add]);
        b
    }
}

fn words(bytes: &mut Vec<u8>, values: &[u32]) {
    for value in values {
        bytes.extend(value.to_le_bytes());
    }
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

const XTERM: [u8; 48] = [
    0, 0, 0, 205, 0, 0, 0, 205, 0, 205, 205, 0, 0, 0, 238, 205,
    0, 205, 0, 205, 205, 229, 229, 229, 127, 127, 127, 255, 0,
    0, 0, 255, 0, 255, 255, 0, 92, 92, 255, 255, 0, 255, 0, 255,
    255, 255, 255, 255,
];

struct Terminal {
    grid: Vec<u8>,
    col: usize,
    row: usize,
    fg: u8,
    bg: u8,
    shown: u8,
    wrap: bool,
    palette: Vec<u8>,
}

impl Terminal {
    fn new() -> Terminal {
        let mut palette = XTERM.to_vec();
        let levels = [0, 95, 135, 175, 215, 255];
        for r in levels {
            for g in levels {
                for b in levels {
                    palette.extend([r, g, b]);
                }
            }
        }
        for i in 0..24 {
            palette.extend([8 + 10 * i as u8; 3]);
        }
        let mut t = Terminal {
            grid: vec![0; COLUMNS * ROWS * 4],
            col: 0,
            row: 0,
            fg: 7,
            bg: 0,
            shown: 1,
            wrap: false,
            palette,
        };
        for cell in 0..COLUMNS * ROWS {
            t.put(cell, ' ' as u16);
        }
        t
    }

    fn put(&mut self, cell: usize, code: u16) {
        let [low, high] = code.to_le_bytes();
        let bytes = [low, high, self.fg, self.bg];
        let at = 4 * cell;
        self.grid[at..at + 4].copy_from_slice(&bytes);
    }

    fn newline(&mut self) {
        (self.col, self.wrap) = (0, false);
        if self.row < ROWS - 1 {
            self.row += 1;
            return;
        }
        self.grid.copy_within(COLUMNS * 4.., 0);
        for col in 0..COLUMNS {
            self.put((ROWS - 1) * COLUMNS + col, ' ' as u16);
        }
    }

    fn write(&mut self, text: &[u8]) {
        for c in String::from_utf8_lossy(text).chars() {
            if c == '\n' {
                self.newline();
            }
            if c < ' ' || c == '\x7f' {
                continue;
            }
            if self.wrap {
                self.newline();
            }
            let code = match u16::try_from(c) {
                Ok(0xffff) | Err(_) => 0xfffd,
                Ok(code) => code,
            };
            self.put(self.row * COLUMNS + self.col, code);
            if self.col == COLUMNS - 1 {
                self.wrap = true;
            } else {
                self.col += 1;
            }
        }
    }

    fn call(&mut self, body: &[u8]) -> (u32, u32) {
        let a = |i: usize| word(body, 4 + 4 * i);
        let data = &body[40..40 + word(body, 36) as usize];
        match body[1] {
            0 => {
                self.write(data);
                return (a(1), 2 * a(1));
            }
            1 if a(0) == 4 => {
                let line = self.row * COLUMNS;
                for col in self.col..COLUMNS {
                    self.put(line + col, ' ' as u16);
                }
            }
            1 if (1..=3).contains(&a(0)) => return (0, 150),
            2 => {
                self.col = (a(0) as usize).min(COLUMNS - 1);
                self.row = (a(1) as usize).min(ROWS - 1);
                self.wrap = false;
            }
            4 => {
                self.fg = u8::try_from(a(0)).unwrap_or(self.fg);
                self.bg = u8::try_from(a(1)).unwrap_or(self.bg);
            }
            7 => {
                let start = 4 * a(0) as usize;
                let size = self.grid.len();
                for (i, &byte) in data.iter().enumerate() {
                    self.grid[(start + i) % size] = byte;
                }
                return (a(2), (data.len() as u32).div_ceil(4));
            }
            _ => {}
        }
        (0, 0)
    }

    fn state(&self) -> Vec<u8> {
        let mut state = self.grid.clone();
        let (col, row) = (self.col as u8, self.row as u8);
        state.extend([col, row, self.fg, self.bg, self.shown]);
        state.push(self.wrap as u8);
        state.extend(&self.palette);
        state.extend([0; 4]);
        state
    }
}

fn hello(body: &[u8]) -> Vec<u8> {
    let name = "terminal (Rust example)";
    let mut reply = vec![0x81, 1, 0, 0, name.len() as u8, 0];
    reply.extend(name.as_bytes());
    reply.extend([0, 0, 0, 0, body[8]]);
    for &slot in &body[9..9 + body[8] as usize] {
        if slot != 2 {
            reply.extend([slot, 0, 0, 0]);
            continue;
        }
        reply.extend([2, 0, 0, TABLE.len() as u8]);
        for entry in &TABLE {
            reply.extend(entry.encode());
        }
    }
    reply.extend([0, 0, 0]);
    reply
}

fn main() {
    let path = env::var("TERSE_BUS_SOCKET").unwrap();
    let mut stream = UnixStream::connect(path).unwrap();
    let mut machines = HashMap::new();
    loop {
        let mut length = [0; 4];
        if stream.read_exact(&mut length).is_err() {
            return;
        }
        let mut message = vec![0; word(&length, 0) as usize];
        if stream.read_exact(&mut message).is_err() {
            return;
        }
        let (address, body) = message.split_at(8);
        let instance = word(address, 0);
        let reply = match body[0] {
            0x01 => hello(body),
            0x03 => {
                machines.insert(instance, Terminal::new());
                vec![0x83, 1, 0, 0, 0, 0]
            }
            0x04 => {
                machines.remove(&instance);
                continue;
            }
            0x10 => {
                let t = machines.get_mut(&instance).unwrap();
                let (result, charge) = t.call(body);
                let mut reply = vec![0x90, 0, 0, 0];
                words(&mut reply, &[result, 0, charge, 0]);
                reply
            }
            0x11 => {
                let mut at = 8;
                for _ in 0..word(body, 4) {
                    let size = word(body, at) as usize;
                    let call = &body[at + 4..at + 4 + size];
                    let t = machines.get_mut(&word(call, 0));
                    t.unwrap().call(&call[8..]);
                    at += 4 + size;
                }
                continue;
            }
            0x30 => {
                let mut inputs = vec![0; 20];
                inputs[0] = 0xb0;
                inputs
            }
            0x31 => continue,
            0x40 => {
                let state = machines[&instance].state();
                let mut reply = vec![0xc0, 0, 0, 0];
                words(&mut reply, &[state.len() as u32]);
                reply.extend(state);
                reply
            }
            0x60 => return,
            _ => b"\x7fnot handled".to_vec(),
        };
        let mut out = Vec::new();
        words(&mut out, &[8 + reply.len() as u32]);
        out.extend(address);
        out.extend(&reply);
        if stream.write_all(&out).is_err() || reply[0] == 0x7f {
            return;
        }
    }
}

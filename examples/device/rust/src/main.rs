use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

const COLUMNS: usize = 80;
const ROWS: usize = 30;
const CELLS: usize = COLUMNS * ROWS;
const CELL_SIZE: usize = 4;
const GRAYS: u8 = 24;
const CONTINUATION: u16 = 0xffff;

const WRITE: u8 = 0;
const COMMAND: u8 = 1;
const SET_CURSOR: u8 = 2;
const SET_COLOURS: u8 = 4;
const CELLS_WRITE: u8 = 7;

const CLEAR_SCREEN: u32 = 1;
const SCROLL_DOWN: u32 = 3;
const CLEAR_TO_END: u32 = 4;

const A0: u8 = 0;
const A1: u8 = 1;
const A2: u8 = 2;
const NONE: u8 = 0xff;
const DEFERRED: u8 = 2;
const BUFFERED: u8 = 4;
const READS: u8 = 1;
const IN_ROM: u8 = 4;
const FAILED: u32 = 0xffff_fffe;
const RETURNS_VALUE: u8 = 1;
const RETURNS_ARG: u8 = 2;
const PER_FOUR: u8 = 1;

fn words(bytes: &mut Vec<u8>, values: &[u32]) {
    for value in values {
        bytes.extend(value.to_le_bytes());
    }
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[derive(Default)]
struct Entry {
    function: u8,
    flags: u8,
    buffer: u8,
    length: u8,
    scale: u32,
    least: u32,
    most: u32,
    access: u8,
    checked: u8,
    checked_most: u32,
    length_result: u32,
    place_result: u32,
    rule: u8,
    rule_arg: u8,
    value: u32,
    value2: u32,
    charge: u32,
    charge_flags: u8,
    charge_arg: u8,
    per_byte: u16,
    low: u32,
    high: u32,
    add: u32,
}

fn plain() -> Entry {
    Entry {
        flags: DEFERRED,
        length: NONE,
        checked: NONE,
        rule: RETURNS_VALUE,
        charge_arg: NONE,
        ..Default::default()
    }
}

fn table() -> Vec<Entry> {
    vec![
        Entry {
            function: WRITE,
            flags: DEFERRED | BUFFERED,
            buffer: A0,
            length: A1,
            scale: 1,
            most: 4096,
            access: READS | IN_ROM,
            length_result: FAILED,
            place_result: FAILED,
            rule: RETURNS_ARG,
            rule_arg: A1,
            per_byte: 2,
            ..plain()
        },
        Entry {
            function: COMMAND,
            charge_arg: A0,
            low: CLEAR_SCREEN,
            high: SCROLL_DOWN,
            add: 150,
            ..plain()
        },
        Entry { function: SET_CURSOR, ..plain() },
        Entry { function: SET_COLOURS, ..plain() },
        Entry {
            function: CELLS_WRITE,
            flags: DEFERRED | BUFFERED,
            buffer: A1,
            length: A2,
            scale: CELL_SIZE as u32,
            most: (CELLS * CELL_SIZE) as u32,
            access: READS | IN_ROM,
            checked: A0,
            checked_most: CELLS as u32 - 1,
            length_result: FAILED,
            place_result: FAILED,
            rule: RETURNS_ARG,
            rule_arg: A2,
            charge_flags: PER_FOUR,
            ..plain()
        },
    ]
}

impl Entry {
    fn encode(&self) -> Vec<u8> {
        let e = self;
        let mut b =
            vec![e.function, e.flags, e.buffer, e.length];
        words(&mut b, &[e.scale, e.least, e.most]);
        b.extend([e.access, e.checked, 0, 0]);
        words(
            &mut b,
            &[e.checked_most, e.length_result, e.place_result],
        );
        b.extend([e.rule, e.rule_arg, 0, 0]);
        words(&mut b, &[e.value, e.value2, e.charge]);
        b.extend([e.charge_flags, e.charge_arg]);
        b.extend(e.per_byte.to_le_bytes());
        words(&mut b, &[e.low, e.high, e.add]);
        b
    }
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
    partial: [u8; 4],
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
        for i in 0..GRAYS {
            palette.extend([8 + 10 * i; 3]);
        }
        let mut t = Terminal {
            grid: vec![0; CELLS * CELL_SIZE],
            col: 0,
            row: 0,
            fg: 7,
            bg: 0,
            shown: 1,
            wrap: false,
            palette,
            partial: [0; 4],
        };
        for cell in 0..CELLS {
            t.put(cell, ' ' as u16);
        }
        t
    }

    fn put(&mut self, cell: usize, code: u16) {
        let [low, high] = code.to_le_bytes();
        let bytes = [low, high, self.fg, self.bg];
        let at = CELL_SIZE * cell;
        self.grid[at..at + CELL_SIZE].copy_from_slice(&bytes);
    }

    fn newline(&mut self) {
        (self.col, self.wrap) = (0, false);
        if self.row < ROWS - 1 {
            self.row += 1;
            return;
        }
        self.grid.copy_within(COLUMNS * CELL_SIZE.., 0);
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
                Ok(CONTINUATION) | Err(_) => {
                    char::REPLACEMENT_CHARACTER as u16
                }
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
        let a = |i: usize| word(body, ARGS_AT + 4 * i);
        let size = word(body, SIZE_AT) as usize;
        let data = &body[DATA_AT..DATA_AT + size];
        match body[FUNCTION_AT] {
            WRITE => {
                self.write(data);
                return (a(1), 2 * a(1));
            }
            COMMAND => match a(0) {
                CLEAR_SCREEN..=SCROLL_DOWN => return (0, 150),
                CLEAR_TO_END => {
                    let line = self.row * COLUMNS;
                    for col in self.col..COLUMNS {
                        self.put(line + col, ' ' as u16);
                    }
                }
                _ => {}
            },
            SET_CURSOR => {
                self.col = (a(0) as usize).min(COLUMNS - 1);
                self.row = (a(1) as usize).min(ROWS - 1);
                self.wrap = false;
            }
            SET_COLOURS => {
                self.fg = u8::try_from(a(0)).unwrap_or(self.fg);
                self.bg = u8::try_from(a(1)).unwrap_or(self.bg);
            }
            CELLS_WRITE => {
                let start = CELL_SIZE * a(0) as usize;
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
        state.extend(self.partial);
        state
    }
}

const HELLO: u8 = 0x01;
const OPEN: u8 = 0x03;
const CLOSE: u8 = 0x04;
const CALL: u8 = 0x10;
const BATCH: u8 = 0x11;
const BEGIN: u8 = 0x30;
const END: u8 = 0x31;
const STATE: u8 = 0x40;
const SHUTDOWN: u8 = 0x60;
const ERROR: u8 = 0x7f;
const REPLY: u8 = 0x80;

const ADDRESS_SIZE: usize = 8;
const PROTOCOL: u8 = 1;
const ON_SOCKET: u8 = 0;
const TOOK: u8 = 1;
const TERMINAL_SLOT: u8 = 2;
const DEVICE_VERSION: u8 = 0;
const SLOTS_AT: usize = 8;
const FUNCTION_AT: usize = 1;
const ARGS_AT: usize = 4;
const SIZE_AT: usize = 36;
const DATA_AT: usize = 40;
const BATCH_COUNT_AT: usize = 4;
const BATCH_CALLS_AT: usize = 8;

fn hello(body: &[u8]) -> Vec<u8> {
    let name = "terminal (Rust example)";
    let mut reply = vec![HELLO | REPLY, PROTOCOL, ON_SOCKET, 0];
    reply.extend((name.len() as u16).to_le_bytes());
    reply.extend(name.as_bytes());
    let count = body[SLOTS_AT] as usize;
    let slots = &body[SLOTS_AT + 1..][..count];
    reply.extend([0, 0, 0, 0, count as u8]);
    for &slot in slots {
        let mut entries = vec![];
        if slot == TERMINAL_SLOT {
            entries = table();
        }
        let count = entries.len() as u8;
        reply.extend([slot, DEVICE_VERSION, 0, count]);
        for entry in &entries {
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
        let mut size = [0; 4];
        if stream.read_exact(&mut size).is_err() {
            return;
        }
        let mut message = vec![0; word(&size, 0) as usize];
        if stream.read_exact(&mut message).is_err() {
            return;
        }
        let (address, body) = message.split_at(ADDRESS_SIZE);
        let instance = word(address, 0);
        let reply = match body[0] {
            HELLO => hello(body),
            OPEN => {
                machines.insert(instance, Terminal::new());
                vec![OPEN | REPLY, TOOK, 0, 0, 0, 0]
            }
            CLOSE => {
                machines.remove(&instance);
                continue;
            }
            CALL => {
                let t = machines.get_mut(&instance).unwrap();
                let (result, charge) = t.call(body);
                let mut reply = vec![CALL | REPLY, 0, 0, 0];
                words(&mut reply, &[result, 0, charge, 0]);
                reply
            }
            BATCH => {
                let mut calls = &body[BATCH_CALLS_AT..];
                for _ in 0..word(body, BATCH_COUNT_AT) {
                    let size = word(calls, 0) as usize;
                    let call = &calls[4..4 + size];
                    let t = machines.get_mut(&word(call, 0));
                    t.unwrap().call(&call[ADDRESS_SIZE..]);
                    calls = &calls[4 + size..];
                }
                continue;
            }
            BEGIN => {
                let mut reply = vec![BEGIN | REPLY, 0, 0, 0];
                words(&mut reply, &[0, 0, 0, 0]);
                reply
            }
            END => continue,
            STATE => {
                let state = machines[&instance].state();
                let mut reply = vec![STATE | REPLY, 0, 0, 0];
                words(&mut reply, &[state.len() as u32]);
                reply.extend(state);
                reply
            }
            SHUTDOWN => return,
            _ => [&[ERROR], &b"not handled"[..]].concat(),
        };
        let mut out = Vec::new();
        words(&mut out, &[(ADDRESS_SIZE + reply.len()) as u32]);
        out.extend(address);
        out.extend(&reply);
        let sent = stream.write_all(&out);
        if sent.is_err() || reply[0] == ERROR {
            return;
        }
    }
}

//! `terse`: the Rust runtime for Tersenet programs on the Terse32 machine
//! (SPEC.md, machine profile 1: RV32IMC). It is the Rust counterpart of
//! programs/lib (crt0.S, terse.h, terse.c) for the program profile: device 1
//! System (frames, time, random numbers, events, controllers, exit),
//! device 2 Terminal and device 3, the 2D vector display. The names follow
//! the C library's, without its `terse_` prefix (the crate's name says it).
//!
//! `terse build hello.rs` compiles this file as an rlib and the program
//! against it; docs/toolchain.md shows the rustc commands. A program is a
//! `#![no_std]`, `#![no_main]` crate that names its main function with
//! `terse::entry!`:
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use terse::*;
//! entry!(main);
//! fn main() {
//!     term_puts("Hello from Rust");
//!     loop {
//!         wait_frame();
//!     }
//! }
//! ```
//!
//! Every device call is an `ecall`: a7 = (slot << 8) | function, the
//! arguments in a0-a5, the result in a0 (and a1); SPEC §calls. A call never
//! touches memory the program did not hand it, so the functions taking
//! slices are safe. A panic writes where it happened on the terminal and
//! stops the machine with a breakpoint fault, which the host shows.
#![no_std]

use core::arch::{asm, global_asm};

// The machine starts at the first byte of ROM with sp at the end of RAM and
// RAM zero (SPEC §reset): copy .data from its load address in ROM, call the
// program's main (named by entry!), and exit with code 0 if it returns.
global_asm!(
    r#"
    .section .text.start, "ax"
    .globl _start
_start:
    la      a0, __data_load
    la      a1, __data_start
    la      a2, __data_end
    j       2f
1:  lw      a3, 0(a0)
    sw      a3, 0(a1)
    addi    a0, a0, 4
    addi    a1, a1, 4
2:  bltu    a1, a2, 1b
    call    terse_main
    li      a0, 0
    li      a7, 0x106
    ecall
3:  j       3b
"#
);

/// Names the program's main function, `fn main()`, which the runtime calls
/// at reset. Returning from it exits with code 0.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        extern "C" fn terse_main() {
            let main: fn() = $main;
            main()
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    term_puts("\npanic");
    if let Some(at) = info.location() {
        term_puts(" at ");
        term_puts(at.file());
        term_putc(b':');
        term_put_dec(at.line());
    }
    // A breakpoint fault (SPEC §stopping): the host shows it with the address.
    unsafe { asm!("ebreak", options(noreturn)) }
}

// ---------------------------------------------------------------- calls

/// A device call with three arguments (SPEC §calls); the result is a0.
///
/// # Safety
/// A call may read or write the memory its arguments point to.
#[inline(always)]
pub unsafe fn call(n: u32, a: u32, b: u32, c: u32) -> u32 {
    let r;
    unsafe { asm!("ecall", inlateout("a0") a => r, inlateout("a1") b => _, in("a2") c, in("a7") n) };
    r
}

/// A device call with two results, a0 and a1.
///
/// # Safety
/// As [`call`].
#[inline(always)]
pub unsafe fn call2(n: u32, a: u32, b: u32) -> (u32, u32) {
    let (r0, r1);
    unsafe { asm!("ecall", inlateout("a0") a => r0, inlateout("a1") b => r1, in("a7") n) };
    (r0, r1)
}

/// A device call with six arguments (a0-a5).
///
/// # Safety
/// As [`call`].
#[inline(always)]
pub unsafe fn call6(n: u32, a: u32, b: u32, c: u32, d: u32, e: u32, f: u32) -> u32 {
    let r;
    unsafe {
        asm!("ecall", inlateout("a0") a => r, inlateout("a1") b => _, in("a2") c, in("a3") d,
             in("a4") e, in("a5") f, in("a7") n)
    };
    r
}

/// Call numbers (SPEC §calls.table): (slot << 8) | function.
pub const CALL_WAIT_FRAME: u32 = 0x100;
pub const CALL_NEXT_EVENT: u32 = 0x101;
pub const CALL_FRAME: u32 = 0x102;
pub const CALL_TIME: u32 = 0x103;
pub const CALL_RANDOM: u32 = 0x104;
pub const CALL_CONTROLLER: u32 = 0x105;
pub const CALL_EXIT: u32 = 0x106;
pub const CALL_DEVICE: u32 = 0x109;
pub const CALL_TERM_WRITE: u32 = 0x200;
pub const CALL_TERM_COMMAND: u32 = 0x201;
pub const CALL_TERM_SET_CURSOR: u32 = 0x202;
pub const CALL_TERM_GET_CURSOR: u32 = 0x203;
pub const CALL_TERM_SET_COLOURS: u32 = 0x204;
pub const CALL_TERM_GET_COLOURS: u32 = 0x205;
pub const CALL_TERM_CELLS_READ: u32 = 0x206;
pub const CALL_TERM_CELLS_WRITE: u32 = 0x207;
pub const CALL_TERM_PALETTE_SET: u32 = 0x208;
pub const CALL_TERM_SIZE: u32 = 0x209;
pub const CALL_VEC_SUBMIT: u32 = 0x300;
pub const CALL_VEC_CLEAR: u32 = 0x301;
pub const CALL_VEC_STYLE: u32 = 0x302;
pub const CALL_VEC_PERSISTENCE: u32 = 0x303;
pub const CALL_VEC_FILL: u32 = 0x304;

/// What a call to a device the machine does not have returns.
pub const ERR_UNKNOWN: u32 = 0xFFFF_FFFF;
/// What a call given a bad buffer returns.
pub const ERR_BUFFER: u32 = 0xFFFF_FFFE;

// --------------------------------------------------------------- system

/// Gives up the rest of this frame's budget (wait_frame, SPEC §system) and
/// returns when the machine runs again: how many frames passed since the
/// previous call, at most 255 (usually 1).
pub fn wait_frame() -> u8 {
    static mut LAST: u32 = 0;
    let now = unsafe { call(CALL_WAIT_FRAME, 0, 0, 0) };
    // Only the machine's single thread runs; nothing else touches LAST.
    let last = unsafe { core::ptr::replace(&raw mut LAST, now) };
    now.wrapping_sub(last).min(255) as u8
}

/// The frame counter.
pub fn frame() -> u32 {
    unsafe { call(CALL_FRAME, 0, 0, 0) }
}

/// Unix time in seconds, as the host gave it at the start of the frame.
pub fn time() -> u32 {
    unsafe { call(CALL_TIME, 0, 0, 0) }
}

/// Stops the machine with an exit code (SPEC §system).
pub fn exit(code: u32) -> ! {
    unsafe { call(CALL_EXIT, code, 0, 0) };
    loop {}
}

/// Fills `buf` (in RAM) with the host's random bytes.
pub fn random(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(256) {
        unsafe { call(CALL_RANDOM, chunk.as_mut_ptr() as u32, chunk.len() as u32, 0) };
    }
}

/// One random byte.
pub fn random_byte() -> u8 {
    let mut b = [0u8];
    random(&mut b);
    b[0]
}

/// The version of device `slot` if this machine has it (SPEC §image: an
/// optional device may be absent). The answer never changes while it runs.
pub fn device(slot: u32) -> Option<u32> {
    match unsafe { call(CALL_DEVICE, slot, 0, 0) } {
        ERR_UNKNOWN => None,
        v => Some(v),
    }
}

/// Device 10, full text (SPEC §terminal.text): whether the terminal draws
/// characters beyond the basic repertoire. List it with `--optional 10:0`.
pub fn has_full_text() -> bool {
    device(10).is_some()
}

// --------------------------------------------------------------- events

/// Event types (SPEC §events).
pub const EV_KEY_DOWN: u8 = 0x01;
pub const EV_KEY_UP: u8 = 0x02;
pub const EV_TEXT: u8 = 0x03;
pub const EV_POINTER: u8 = 0x04;
pub const EV_WHEEL: u8 = 0x06;

/// USB HID key codes used by most programs.
pub const KEY_ENTER: u8 = 0x28;
pub const KEY_ESCAPE: u8 = 0x29;
pub const KEY_BACKSPACE: u8 = 0x2A;
pub const KEY_TAB: u8 = 0x2B;
pub const KEY_SPACE: u8 = 0x2C;
pub const KEY_HOME: u8 = 0x4A;
pub const KEY_PAGE_UP: u8 = 0x4B;
pub const KEY_END: u8 = 0x4D;
pub const KEY_PAGE_DOWN: u8 = 0x4E;
pub const KEY_RIGHT: u8 = 0x4F;
pub const KEY_LEFT: u8 = 0x50;
pub const KEY_DOWN: u8 = 0x51;
pub const KEY_UP: u8 = 0x52;

pub const MOD_SHIFT: u8 = 0x01;
pub const MOD_CONTROL: u8 = 0x02;
pub const MOD_ALT: u8 = 0x04;
pub const MOD_META: u8 = 0x08;

pub const BUTTON_PRIMARY: u8 = 0x01;
pub const BUTTON_SECONDARY: u8 = 0x02;
pub const BUTTON_MIDDLE: u8 = 0x04;

/// An event, the four bytes of SPEC §events: its type and three bytes whose
/// meaning depends on it (a key's code and modifiers, a character, ...).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Event {
    pub kind: u8,
    pub a: u8,
    pub b: u8,
    pub c: u8,
}

impl Event {
    /// Key down and key up: the key code (KEY_*).
    pub fn key(&self) -> u8 {
        self.a
    }
    /// Key down and key up: the modifiers (MOD_*).
    pub fn modifiers(&self) -> u8 {
        self.b
    }
    /// Text: the character typed.
    pub fn char(&self) -> char {
        let cp = self.a as u32 | (self.b as u32) << 8 | ((self.c & 0x1F) as u32) << 16;
        char::from_u32(cp).unwrap_or('\u{FFFD}')
    }
    /// Pointer: the position in screen units (0-639, 0-479).
    pub fn pointer(&self) -> (u16, u16) {
        (self.a as u16 | ((self.c & 3) as u16) << 8, self.b as u16 | ((self.c & 4) as u16) << 6)
    }
    /// Pointer: the buttons held (BUTTON_*).
    pub fn buttons(&self) -> u8 {
        self.c >> 3 & 7
    }
    /// Wheel: the vertical and horizontal steps (positive: down, right).
    pub fn wheel(&self) -> (i8, i8) {
        (self.a as i8, self.b as i8)
    }
}

/// Takes the next event from the queue, if there is one.
pub fn next_event() -> Option<Event> {
    let mut e = Event::default();
    match unsafe { call(CALL_NEXT_EVENT, &raw mut e as u32, 0, 0) } {
        0 => None,
        _ => Some(e),
    }
}

/// A controller's state (SPEC §controllers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Pad {
    pub connected: u8,
    /// Up, down, left, right, A, B, X, Y from bit 0 (PAD_*).
    pub buttons: u8,
    /// L1, R1, L2, R2, select, start, L3, R3 from bit 0 (PAD_L1, ...).
    pub buttons2: u8,
    pub lx: i8,
    pub ly: i8,
    pub rx: i8,
    pub ry: i8,
    pub zero: u8,
}

pub const PAD_UP: u8 = 0x01;
pub const PAD_DOWN: u8 = 0x02;
pub const PAD_LEFT: u8 = 0x04;
pub const PAD_RIGHT: u8 = 0x08;
pub const PAD_A: u8 = 0x10;
pub const PAD_B: u8 = 0x20;
pub const PAD_X: u8 = 0x40;
pub const PAD_Y: u8 = 0x80;
pub const PAD_L1: u8 = 0x01;
pub const PAD_R1: u8 = 0x02;
pub const PAD_L2: u8 = 0x04;
pub const PAD_R2: u8 = 0x08;
pub const PAD_SELECT: u8 = 0x10;
pub const PAD_START: u8 = 0x20;
pub const PAD_L3: u8 = 0x40;
pub const PAD_R3: u8 = 0x80;

/// Controller `n`'s state.
pub fn pad_read(n: u8) -> Pad {
    let mut pad = Pad::default();
    unsafe { call(CALL_CONTROLLER, n as u32, &raw mut pad as u32, 0) };
    pad
}

// ------------------------------------------------------------- terminal

pub const TERM_COLS: u32 = 80;
pub const TERM_ROWS: u32 = 30;
/// The longest write call (SPEC §terminal.calls).
pub const TERM_WRITE_MAX: usize = 4096;

/// Terminal commands (term_cmd).
pub const TERM_CLEAR: u8 = 0x01;
pub const TERM_SCROLL_UP: u8 = 0x02;
pub const TERM_SCROLL_DOWN: u8 = 0x03;
pub const TERM_CLEAR_EOL: u8 = 0x04;
pub const TERM_CURSOR_ON: u8 = 0x05;
pub const TERM_CURSOR_OFF: u8 = 0x06;

/// Writes bytes to the terminal (SPEC §terminal.put): UTF-8 text and the
/// control bytes it knows (newline, backspace, ...).
pub fn term_write(bytes: &[u8]) {
    for chunk in bytes.chunks(TERM_WRITE_MAX) {
        unsafe { call(CALL_TERM_WRITE, chunk.as_ptr() as u32, chunk.len() as u32, 0) };
    }
}

/// Writes a string.
pub fn term_puts(s: &str) {
    term_write(s.as_bytes());
}

/// Writes one byte.
pub fn term_putc(byte: u8) {
    term_write(&[byte]);
}

/// Writes one character as UTF-8.
pub fn term_put_char(c: char) {
    let mut buf = [0u8; 4];
    term_puts(c.encode_utf8(&mut buf));
}

/// Writes a number in decimal (without core::fmt, which costs several KiB).
pub fn term_put_dec(mut n: u32) {
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    term_write(&buf[i..]);
}

/// Moves the cursor to column `col` (0-79), row `row` (0-29).
pub fn term_goto(col: u8, row: u8) {
    unsafe { call(CALL_TERM_SET_CURSOR, col as u32, row as u32, 0) };
}

/// Sets the foreground and background palette indexes.
pub fn term_color(fg: u8, bg: u8) {
    unsafe { call(CALL_TERM_SET_COLOURS, fg as u32, bg as u32, 0) };
}

/// Sets the foreground only.
pub fn term_fg(fg: u8) {
    unsafe { call(CALL_TERM_SET_COLOURS, fg as u32, 0x100, 0) };
}

/// A terminal command (TERM_CLEAR, ...).
pub fn term_cmd(c: u8) {
    unsafe { call(CALL_TERM_COMMAND, c as u32, 0, 0) };
}

/// The cursor's column and row.
pub fn term_get_cursor() -> (u8, u8) {
    let (c, r) = unsafe { call2(CALL_TERM_GET_CURSOR, 0, 0) };
    (c as u8, r as u8)
}

/// The current foreground and background.
pub fn term_get_colours() -> (u8, u8) {
    let (f, b) = unsafe { call2(CALL_TERM_GET_COLOURS, 0, 0) };
    (f as u8, b as u8)
}

/// A cell of SPEC §terminal.cells: the character (16 bits), the foreground
/// and the background.
pub const fn cell(ch: u16, fg: u8, bg: u8) -> u32 {
    ch as u32 | (fg as u32) << 16 | (bg as u32) << 24
}

/// Writes cells from cell index `index` (row * 80 + column) on.
pub fn term_cells_write(index: u32, cells: &[u32]) {
    unsafe { call(CALL_TERM_CELLS_WRITE, index, cells.as_ptr() as u32, cells.len() as u32) };
}

/// Reads cells from cell index `index` on.
pub fn term_cells_read(index: u32, cells: &mut [u32]) {
    unsafe { call(CALL_TERM_CELLS_READ, index, cells.as_mut_ptr() as u32, cells.len() as u32) };
}

/// Sets palette entries from `first` on from red, green, blue triples.
pub fn term_palette_set(first: u8, rgb: &[[u8; 3]]) -> u32 {
    unsafe { call(CALL_TERM_PALETTE_SET, first as u32, rgb.as_ptr() as u32, rgb.len() as u32) }
}

/// Writes `n` cells of `ch` in colours fg, bg from (col, row); the cursor
/// does not move.
pub fn term_fill(col: u8, row: u8, n: u32, ch: u16, fg: u8, bg: u8) {
    let row_of = [cell(ch, fg, bg); TERM_COLS as usize];
    let mut at = row as u32 * TERM_COLS + col as u32;
    let mut left = n;
    while left > 0 {
        let k = left.min(TERM_COLS);
        term_cells_write(at, &row_of[..k as usize]);
        at += k;
        left -= k;
    }
}

/// `write!(Term, ...)` writes formatted text to the terminal. It pulls in
/// core::fmt, several KiB of ROM; term_puts and term_put_dec do not.
pub struct Term;

impl core::fmt::Write for Term {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        term_puts(s);
        Ok(())
    }
}

// ------------------------------------------------- 2D vector display

pub const VEC_DONE: u8 = 0;
pub const VEC_REJECTED: u8 = 1;
/// The longest list, in bytes.
pub const VEC_MAX_LIST: usize = 16384;
/// Coordinates are in sixteenths of a screen unit; the screen is 640 x 480
/// units.
pub const VEC_UNIT: i16 = 16;
pub const VEC_SCREEN_W: i16 = 10240;
pub const VEC_SCREEN_H: i16 = 7680;
pub const VEC_STYLE_CLEAN: u32 = 0;
pub const VEC_STYLE_PHOSPHOR: u32 = 1;
pub const VEC_FILL_ON: u32 = 0;
pub const VEC_FILL_OFF: u32 = 1;

const VEC_OP_END: u8 = 0x00;
const VEC_OP_COLOUR: u8 = 0x01;
const VEC_OP_WIDTH: u8 = 0x02;
const VEC_OP_ORIGIN: u8 = 0x03;
const VEC_OP_MOVE: u8 = 0x04;
const VEC_OP_LINE: u8 = 0x05;
const VEC_OP_CIRCLE: u8 = 0x06;
const VEC_OP_DISC: u8 = 0x07;
const VEC_OP_RECT: u8 = 0x08;
const VEC_OP_TRIANGLE: u8 = 0x09;

/// Builds a 2D list (SPEC §vector) in a buffer the program owns, as
/// vec_begin and the vec_* builders do in C. A command that does not fit
/// sets `overflow` and is not written, nor is any later one; `end` always
/// has room, as one byte is kept for it.
pub struct VecList<'a> {
    buf: &'a mut [u8],
    len: usize,
    pub overflow: bool,
}

impl<'a> VecList<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        VecList { buf, len: 0, overflow: false }
    }

    /// Starts again with an empty list.
    pub fn clear(&mut self) {
        self.len = 0;
        self.overflow = false;
    }

    /// The bytes written so far.
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    fn put(&mut self, cmd: &[u8]) {
        if self.overflow || self.buf.len() - 1 - self.len < cmd.len() {
            self.overflow = true;
            return;
        }
        self.buf[self.len..self.len + cmd.len()].copy_from_slice(cmd);
        self.len += cmd.len();
    }

    fn xy(&mut self, op: u8, x: i16, y: i16) {
        let (x, y) = (x.to_le_bytes(), y.to_le_bytes());
        self.put(&[op, x[0], x[1], y[0], y[1]]);
    }

    pub fn colour(&mut self, r: u8, g: u8, b: u8, a: u8) {
        self.put(&[VEC_OP_COLOUR, r, g, b, a]);
    }
    pub fn width(&mut self, w: u8) {
        self.put(&[VEC_OP_WIDTH, w]);
    }
    pub fn origin(&mut self, x: i16, y: i16) {
        self.xy(VEC_OP_ORIGIN, x, y);
    }
    pub fn move_to(&mut self, x: i16, y: i16) {
        self.xy(VEC_OP_MOVE, x, y);
    }
    pub fn line_to(&mut self, x: i16, y: i16) {
        self.xy(VEC_OP_LINE, x, y);
    }
    pub fn rect(&mut self, x: i16, y: i16) {
        self.xy(VEC_OP_RECT, x, y);
    }
    pub fn circle(&mut self, r: u16) {
        let r = r.to_le_bytes();
        self.put(&[VEC_OP_CIRCLE, r[0], r[1]]);
    }
    pub fn disc(&mut self, r: u16) {
        let r = r.to_le_bytes();
        self.put(&[VEC_OP_DISC, r[0], r[1]]);
    }
    pub fn tri(&mut self, x1: i16, y1: i16, x2: i16, y2: i16) {
        let (a, b, c, d) = (x1.to_le_bytes(), y1.to_le_bytes(), x2.to_le_bytes(), y2.to_le_bytes());
        self.put(&[VEC_OP_TRIANGLE, a[0], a[1], b[0], b[1], c[0], c[1], d[0], d[1]]);
    }

    /// Ends the list and submits it: VEC_DONE or VEC_REJECTED.
    pub fn submit(&mut self) -> u8 {
        self.buf[self.len] = VEC_OP_END;
        vec_submit(&self.buf[..self.len + 1])
    }
}

/// Submits a list (which must be in RAM): VEC_DONE or VEC_REJECTED.
pub fn vec_submit(list: &[u8]) -> u8 {
    unsafe { call(CALL_VEC_SUBMIT, list.as_ptr() as u32, list.len() as u32, 0) as u8 }
}

/// Clears the display.
pub fn vec_clear() {
    unsafe { call(CALL_VEC_CLEAR, 0, 0, 0) };
}

/// Display style (VEC_STYLE_*), persistence (0-255) and fill (VEC_FILL_*).
pub fn vec_style(s: u32) -> u8 {
    unsafe { call(CALL_VEC_STYLE, s, 0, 0) as u8 }
}
pub fn vec_persistence(p: u32) -> u8 {
    unsafe { call(CALL_VEC_PERSISTENCE, p, 0, 0) as u8 }
}
pub fn vec_fill(f: u32) -> u8 {
    unsafe { call(CALL_VEC_FILL, f, 0, 0) as u8 }
}

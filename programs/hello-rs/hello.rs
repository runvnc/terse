//! hello.rs: programs/hello (hello.S) in Rust, with the runtime that
//! `terse build` embeds (programs/lib/rust/terse.rs).
//!
//! Shows a title bar, some UTF-8 text, palette entries 0-15 written straight
//! into cell memory, and a frame counter; echoes typed text; Esc exits.
//! Build it with:  terse build hello.rs --optional 10:0 --ram-pages 1
//! (build.sh does exactly that; the image is checked in as hello-rs.terse).
#![no_std]
#![no_main]

use terse::*;

entry!(main);

/// Row, column, foreground, text.
const TEXTS: [(u8, u8, u8, &str); 5] = [
    (2, 2, 11, "héllo, wörld — привет, мир"),
    (3, 2, 7, "Ελληνικά · кириллица · ┌─┬─┐ · ♥ ★ ✓"),
    (4, 2, 8, "Type to echo text below. Enter: new line, Backspace, Esc: exit."),
    (6, 2, 7, "Palette entries 0-15, written with cells_write:"),
    (9, 2, 7, "frame"),
];

fn hex_digit(n: u32) -> u16 {
    b"0123456789ABCDEF"[(n & 15) as usize] as u16
}

fn main() {
    // Title bar: white on blue across the whole of row 0.
    term_color(15, 4);
    term_puts(" Tersenet · hello");
    term_cmd(TERM_CLEAR_EOL);

    for (row, col, fg, text) in TEXTS {
        term_goto(col, row);
        term_color(fg, 0);
        term_puts(text);
    }

    // Palette row: three cells per entry, background = entry; white text on
    // entries 0-6, black on the rest.
    let mut cells = [0u32; 48];
    for entry in 0..16u32 {
        let fg = if entry < 7 { 15 } else { 0 };
        let i = entry as usize * 3;
        cells[i] = cell(b' ' as u16, fg, entry as u8);
        cells[i + 1] = cell(hex_digit(entry), fg, entry as u8);
        cells[i + 2] = cells[i];
    }
    term_cells_write(7 * 80 + 2, &cells);

    // Prompt; typed text appears after it, white on the background as is.
    term_goto(0, 11);
    term_color(10, 0);
    term_puts("> ");
    term_fg(15);

    loop {
        wait_frame();
        // The frame counter as eight hex digits, yellow on black.
        let n = frame();
        let mut digits = [0u32; 8];
        for (i, d) in digits.iter_mut().enumerate() {
            *d = cell(hex_digit(n >> (28 - 4 * i)), 14, 0);
        }
        term_cells_write(9 * 80 + 8, &digits);

        while let Some(e) = next_event() {
            match e.kind {
                EV_TEXT => term_put_char(e.char()),
                EV_KEY_DOWN => match e.key() {
                    KEY_ESCAPE => exit(0),
                    KEY_ENTER => term_puts("\n> "),
                    KEY_BACKSPACE => term_write(&[8, b' ', 8]),
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

//! The host's consent prompt (SPEC 9.5): drawn by the host over the bottom
//! rows of the screen, on top of everything the program shows, in colours
//! of its own (the renderer's overlay, not the program's palette). The
//! program cannot draw, read or answer it.

use terse_core::{COLS, ROWS, Terminal};
use terse_net::{Asked, Prompt, to_hex};

/// Rows the prompt covers, at the bottom of the screen.
pub const PROMPT_ROWS: usize = 4;
/// The first row of the prompt.
pub const FIRST_ROW: usize = ROWS - PROMPT_ROWS;

/// Overlay colour codes, as the renderer's overlay draws them (foreground
/// in a cell's foreground byte, background in its background byte).
pub const FG_TEXT: u8 = 0;
pub const FG_KEY: u8 = 1;
pub const FG_DIM: u8 = 2;
pub const BG_BASE: u8 = 0;
pub const BG_BAND: u8 = 3;

/// `3fa9c2…c21e07`: the first and last six hex digits of a feed key.
pub fn short_key(key: &[u8; 32]) -> String {
    let hex = to_hex(key);
    format!("{}…{}", &hex[..6], &hex[58..])
}

/// A size in bytes for people: `812 bytes`, `6.1 KB`, `1.2 MB`.
pub fn size_text(n: u32) -> String {
    match n {
        0..1000 => format!("{n} bytes"),
        1000..1_000_000 => format!("{}.{} KB", n / 1000, n % 1000 / 100),
        _ => format!("{}.{} MB", n / 1_000_000, n % 1_000_000 / 100_000),
    }
}

/// Columns `ch` takes in the grid (SPEC 8.1; above U+FFFF it shows as U+FFFD).
fn width(ch: char) -> usize {
    match u16::try_from(ch as u32) {
        Ok(c) if terse_core::wide::is_wide(c) => 2,
        _ => 1,
    }
}

/// `s` with control characters replaced, cut with an ellipsis to `cols`.
pub fn fit(s: &str, cols: usize) -> String {
    let clean: Vec<char> = s.chars().map(|c| if c.is_control() { '\u{FFFD}' } else { c }).collect();
    let total: usize = clean.iter().map(|&c| width(c)).sum();
    if total <= cols {
        return clean.into_iter().collect();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in clean {
        if w + width(c) > cols.saturating_sub(1) {
            break;
        }
        w += width(c);
        out.push(c);
    }
    out.push('…');
    out
}

/// The prompt's lines: what is asked and what follows from it.
pub fn describe(program: &str, asked: &Asked) -> (String, String) {
    let program = fit(program, 16);
    match asked {
        Asked::Subscribe(key) => (
            format!("“{program}” asks to subscribe to the feed {}.", short_key(key)),
            "Its articles will be downloaded and shown in your timeline.".into(),
        ),
        Asked::Unsubscribe(key) => (
            format!("“{program}” asks to unsubscribe from the feed {}.", short_key(key)),
            "Its articles will no longer be shown in your timeline.".into(),
        ),
        Asked::Publish { title, size } => {
            let size = size_text(*size);
            let room = COLS - 2 - format!("“{program}” asks to publish “” ({size}).").chars().count();
            (
                format!("“{program}” asks to publish “{}” ({size}).", fit(title, room)),
                "It will be signed with your key and sent to the relay for anyone to read.".into(),
            )
        }
    }
}

fn line(t: &mut Terminal, row: usize, bg: u8, parts: &[(u8, &str)]) {
    t.set_colours(FG_TEXT, bg);
    t.set_cursor(0, row as u32);
    t.command(0x04); // clear to the end of the row
    for (fg, text) in parts {
        t.set_colours(*fg, bg);
        t.write(text.as_bytes());
    }
}

/// `shown` with the prompt for `prompt` in its bottom rows; `waiting` is
/// how many requests wait, this one included.
pub fn with_prompt(shown: &Terminal, program: &str, prompt: &Prompt, waiting: usize) -> Terminal {
    let mut t = shown.clone();
    let (what, then) = describe(program, &prompt.asked);
    let more = match waiting {
        0 | 1 => String::new(),
        n => format!("{} more waiting ", n - 1),
    };
    let head = " Terse host · permission";
    let pad = COLS.saturating_sub(head.chars().count() + more.chars().count());
    line(&mut t, FIRST_ROW, BG_BAND, &[(FG_KEY, head), (FG_TEXT, &" ".repeat(pad)), (FG_DIM, &more)]);
    line(&mut t, FIRST_ROW + 1, BG_BASE, &[(FG_TEXT, " "), (FG_TEXT, &fit(&what, COLS - 2))]);
    line(&mut t, FIRST_ROW + 2, BG_BASE, &[(FG_DIM, " "), (FG_DIM, &fit(&then, COLS - 2))]);
    line(
        &mut t,
        FIRST_ROW + 3,
        BG_BASE,
        &[(FG_TEXT, " Allow?   "), (FG_KEY, "Y"), (FG_TEXT, " yes     "), (FG_KEY, "N"), (FG_TEXT, " or "), (FG_KEY, "Esc"), (FG_TEXT, " no")],
    );
    t.command(0x06); // no cursor in the prompt
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_text() {
        assert_eq!(short_key(&[0xAB; 32]), "ababab…ababab");
        assert_eq!((size_text(812), size_text(6150), size_text(1_048_576)), ("812 bytes".into(), "6.1 KB".into(), "1.0 MB".into()));
        assert_eq!(fit("a\u{1b}b", 10), "a\u{FFFD}b");
        assert_eq!(fit("日本語の題名", 7), "日本語…");
        let p = Prompt { handle: 1, asked: Asked::Publish { title: "x".repeat(120), size: 2000 } };
        let t = with_prompt(&Terminal::default(), "reader", &p, 3);
        let text = t.text();
        let rows: Vec<&str> = text.lines().collect();
        assert!(rows[FIRST_ROW].contains("2 more waiting"), "{text}");
        assert!(rows[FIRST_ROW + 1].starts_with(" “reader” asks to publish “xxx") && rows[FIRST_ROW + 1].contains("…” (2.0 KB)."), "{text}");
        assert!(rows[FIRST_ROW + 3].contains("Y yes"));
        assert!(rows[..FIRST_ROW].iter().all(|r| r.trim().is_empty()), "the rest is the program's");
    }
}

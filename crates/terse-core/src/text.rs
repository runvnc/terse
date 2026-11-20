//! Basic and full text (SPEC §terminal.text): which characters a host must
//! draw in the terminal, and device 10, the capability that says a machine
//! is shown with every character. None of this is machine state: the
//! terminal holds and moves the same cells on every host.

use crate::terminal::{COLS, CONTINUATION, ROWS, Terminal};

/// Device 10, Full text: a capability with no calls, events or state.
pub const FULL_TEXT: u8 = 10;

/// The basic repertoire, as inclusive ranges of code points: printable
/// ASCII, the printable Latin-1 Supplement, dashes, quotation marks,
/// bullet, ellipsis, minus sign, box drawing and block elements, and the
/// replacement character. It is also the basic text repertoire of
/// articles (ARTICLE.md 1.1), and `vectors/basic-text.txt` lists it.
pub const BASIC: [(u16, u16); 10] = [
    (0x0020, 0x007E),
    (0x00A0, 0x00FF),
    (0x2013, 0x2014),
    (0x2018, 0x2019),
    (0x201C, 0x201D),
    (0x2022, 0x2022),
    (0x2026, 0x2026),
    (0x2212, 0x2212),
    (0x2500, 0x259F),
    (0xFFFD, 0xFFFD),
];

/// The number of characters in the basic repertoire.
pub const BASIC_COUNT: u32 = 361;

/// How a cell is drawn for a machine without full text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basic {
    /// A character of the basic repertoire, drawn as itself (U+0020 and
    /// U+00A0 draw nothing but the background).
    Char(char),
    /// A control code: the background only.
    Blank,
    /// Anything else, a continuation cell included: the placeholder.
    Placeholder,
}

/// Whether `code` is in the basic repertoire.
pub fn is_basic(code: u16) -> bool {
    BASIC.iter().any(|&(lo, hi)| (lo..=hi).contains(&code))
}

/// How a cell holding `code` is drawn without full text: each cell on its
/// own, never two as one wide character.
pub fn basic(code: u16) -> Basic {
    match code {
        _ if is_basic(code) => Basic::Char(char::from_u32(code as u32).unwrap()),
        0x0000..=0x001F | 0x007F..=0x009F => Basic::Blank,
        _ => Basic::Placeholder,
    }
}

/// The placeholder's coverage in a cell of `w` × `h` pixels (row major,
/// `out` zeroed, `w * h` bytes): an outlined box, inset from the cell's
/// edges, its lines about a twelfth of the cell's width thick.
pub fn placeholder(w: u32, h: u32, out: &mut [u8]) {
    if w < 3 || h < 3 {
        out.iter_mut().for_each(|p| *p = 255);
        return;
    }
    let t = (w / 12).max(1);
    let (x0, x1) = (w / 8, w - w / 8 - 1);
    let (y0, y1) = (h / 6, h - h / 8 - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let edge = x < x0 + t || x + t > x1 || y < y0 + t || y + t > y1;
            if edge {
                out[(y * w + x) as usize] = 255;
            }
        }
    }
}

/// The pseudo code renderers key the placeholder's glyph by: the
/// continuation code, which is never drawn as a character of its own.
pub const PLACEHOLDER_CODE: u16 = CONTINUATION;

/// The cells of `t` as a renderer draws them, row by row: (glyph slot,
/// foreground, background). `slot(code, wide)` gives the slot of a glyph
/// (0 for none), and of the placeholder for [`PLACEHOLDER_CODE`]; a wide
/// glyph's right half is the slot after its left. With full text a
/// character followed by a continuation cell is drawn across both (SPEC
/// §terminal.cells); without it every cell is drawn on its own, with the
/// basic repertoire and the placeholder (SPEC §terminal.text).
pub fn draw_cells(t: &Terminal, mut slot: impl FnMut(u16, bool) -> u32) -> Vec<(u32, u8, u8)> {
    let mut out = vec![(0, 0, 0); COLS * ROWS];
    let full = t.full_text();
    for row in 0..ROWS {
        let mut col = 0;
        while col < COLS {
            let cell = t.cell(col, row);
            if !full {
                let s = match basic(cell.ch) {
                    Basic::Char(_) => slot(cell.ch, false),
                    Basic::Blank => 0,
                    Basic::Placeholder => slot(PLACEHOLDER_CODE, false),
                };
                out[row * COLS + col] = (s, cell.fg, cell.bg);
                col += 1;
                continue;
            }
            let wide = cell.ch != CONTINUATION && col + 1 < COLS && t.cell(col + 1, row).ch == CONTINUATION;
            let s = if cell.ch == CONTINUATION { 0 } else { slot(cell.ch, wide) };
            for half in 0..1 + wide as usize {
                let c = t.cell(col + half, row);
                out[row * COLS + col + half] = (if s == 0 { 0 } else { s + half as u32 }, c.fg, c.bg);
            }
            col += 1 + wide as usize;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repertoire() {
        let n: u32 = BASIC.iter().map(|&(a, b)| (b - a + 1) as u32).sum();
        assert_eq!(n, BASIC_COUNT);
        assert!(BASIC.windows(2).all(|w| w[0].1 + 1 < w[1].0), "sorted, apart");
        // All narrow (SPEC §terminal.text), so that one fixed-width font
        // covers them; the state never depends on the repertoire.
        for &(lo, hi) in &BASIC {
            for c in lo..=hi {
                assert!(!crate::wide::is_wide(c), "U+{c:04X} is narrow");
            }
        }
        for c in ['A', '~', ' ', 'é', 'ÿ', '·', '×', '–', '—', '‘', '’', '“', '”', '•', '…', '−', '─', '┌', '█', '▟', '\u{FFFD}'] {
            assert!(is_basic(c as u16), "{c}");
        }
        for c in ['€', '♥', 'Ω', '你', '‖', '\u{2012}', '\u{2015}', '\u{2027}', '\u{7F}', '\u{9F}', '\u{25A0}', '\u{FFFC}', '\u{FFFE}'] {
            assert!(!is_basic(c as u16), "{c}");
        }
        assert_eq!(basic('a' as u16), Basic::Char('a'));
        assert_eq!(basic(0), Basic::Blank);
        assert_eq!(basic(0x85), Basic::Blank);
        assert_eq!(basic(0x4F60), Basic::Placeholder);
        assert_eq!(basic(CONTINUATION), Basic::Placeholder);
    }

    /// The repertoire is exactly the ranges of `vectors/basic-text.txt`,
    /// the list ARTICLE.md 1.1 and SPEC §terminal.text share.
    #[test]
    fn repertoire_is_the_shared_vector() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/basic-text.txt");
        let text = std::fs::read_to_string(path).unwrap();
        let mut ranges = Vec::new();
        let mut count = None;
        for line in text.lines() {
            let mut w = line.split_whitespace();
            match (w.next(), w.next(), w.next()) {
                (Some("range"), Some(a), Some(b)) => {
                    ranges.push((u16::from_str_radix(a, 16).unwrap(), u16::from_str_radix(b, 16).unwrap()))
                }
                (Some("count"), Some(n), None) => count = Some(n.parse::<u32>().unwrap()),
                _ => {}
            }
        }
        assert_eq!(ranges, BASIC);
        assert_eq!(count, Some(BASIC_COUNT));
    }

    /// The same terminal drawn with full text and with basic text: the
    /// state is one, the glyphs differ.
    #[test]
    fn draw_cells_with_and_without_full_text() {
        let (mut full, mut basic_t) = (Terminal::with_full_text(true), Terminal::with_full_text(false));
        for t in [&mut full, &mut basic_t] {
            // The control character is ignored; then U+0001 straight into cell 6.
            t.write("a你\u{1}é─€".as_bytes());
            t.cells_write(6, &[1, 0, 7, 0]);
        }
        assert_eq!(full, basic_t);
        let key = |code: u16, wide: bool| code as u32 * 2 + wide as u32 + 1;
        let f = draw_cells(&full, key);
        let b = draw_cells(&basic_t, key);
        // Full: 你 across cells 1 and 2.
        assert_eq!(f[1].0, key(0x4F60, true));
        assert_eq!(f[2].0, key(0x4F60, true) + 1);
        // Basic: a, two placeholders (你 and its continuation), é, ─, a
        // placeholder for €, and U+0001 blank.
        let p = key(PLACEHOLDER_CODE, false);
        let slots: Vec<u32> = b[..7].iter().map(|c| c.0).collect();
        assert_eq!(slots, [key('a' as u16, false), p, p, key(0xE9, false), key(0x2500, false), p, 0]);
        assert_eq!(b[6].1, 7, "a blank cell keeps its colours");
    }

    #[test]
    fn placeholder_is_a_box() {
        let (w, h) = (16, 32);
        let mut px = vec![0; (w * h) as usize];
        placeholder(w, h, &mut px);
        assert!(px.iter().any(|&p| p == 255));
        // Hollow: the centre is clear, the corners of the cell are clear.
        assert_eq!(px[(h / 2 * w + w / 2) as usize], 0);
        assert_eq!(px[0], 0);
        assert_eq!(px[(w * h - 1) as usize], 0);
    }
}

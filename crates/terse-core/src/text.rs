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

/// The full repertoire (SPEC §terminal.text): what a host with device 10
/// draws in the terminal at least, as inclusive ranges of code points. It
/// holds the basic repertoire and the characters of Latin, Greek and
/// Cyrillic, IPA, combining diacritics, punctuation, super- and
/// subscripts, currency, letterlike symbols, number forms, arrows,
/// mathematical operators, technical symbols, geometric shapes,
/// miscellaneous symbols and dingbats that the reference fonts (DejaVu Sans
/// Mono, Sans and Sans Bold 2.37) all have, wide characters left out: every
/// one is narrow (§terminal.cells). `vectors/full-text.txt` lists it.
pub const FULL: [(u16, u16); 156] = [
    (0x0020, 0x007E), (0x00A0, 0x01C3), (0x01CD, 0x01E3), (0x01E6, 0x01F0), (0x01F4, 0x01F6),
    (0x01F8, 0x01F9), (0x01FC, 0x0221), (0x0224, 0x0241), (0x0243, 0x0245), (0x024C, 0x024D),
    (0x0250, 0x02B9), (0x02BB, 0x02C1), (0x02C6, 0x02C9), (0x02CC, 0x02D3), (0x02D6, 0x02DE),
    (0x02E0, 0x02E9), (0x02EE, 0x02EE), (0x02F3, 0x02F3), (0x0300, 0x033F), (0x0343, 0x0343),
    (0x0358, 0x0358), (0x0361, 0x0361), (0x0374, 0x0377), (0x037A, 0x037F), (0x0384, 0x038A),
    (0x038C, 0x038C), (0x038E, 0x03A1), (0x03A3, 0x03CE), (0x03D0, 0x03E1), (0x03F0, 0x045F),
    (0x0462, 0x0463), (0x0472, 0x0473), (0x0490, 0x049B), (0x04A2, 0x04A5), (0x04AA, 0x04B3),
    (0x04BA, 0x04BB), (0x04C0, 0x04C4), (0x04C7, 0x04C8), (0x04CB, 0x04CC), (0x04CF, 0x04F9),
    (0x0510, 0x0511), (0x051A, 0x051D), (0x1E00, 0x1E13), (0x1E18, 0x1E2D), (0x1E30, 0x1E4D),
    (0x1E54, 0x1E63), (0x1E68, 0x1E79), (0x1E7C, 0x1E99), (0x1E9B, 0x1E9B), (0x1E9F, 0x1EA1),
    (0x1EAC, 0x1EAD), (0x1EB0, 0x1EB1), (0x1EB6, 0x1EB9), (0x1EBC, 0x1EBD), (0x1EC6, 0x1EC7),
    (0x1ECA, 0x1ECD), (0x1ED8, 0x1EDD), (0x1EE0, 0x1EE5), (0x1EE8, 0x1EEB), (0x1EEE, 0x1EF5),
    (0x1EF8, 0x1EF9), (0x1F00, 0x1F15), (0x1F18, 0x1F1D), (0x1F20, 0x1F45), (0x1F48, 0x1F4D),
    (0x1F50, 0x1F57), (0x1F59, 0x1F59), (0x1F5B, 0x1F5B), (0x1F5D, 0x1F5D), (0x1F5F, 0x1F7D),
    (0x1F80, 0x1FB4), (0x1FB6, 0x1FC4), (0x1FC6, 0x1FD3), (0x1FD6, 0x1FDB), (0x1FDD, 0x1FEF),
    (0x1FF2, 0x1FF4), (0x1FF6, 0x1FFE), (0x2000, 0x200A), (0x2010, 0x2023), (0x2026, 0x2026),
    (0x202F, 0x2037), (0x2039, 0x203A), (0x203C, 0x203F), (0x2045, 0x2049), (0x204B, 0x204B),
    (0x205F, 0x205F), (0x2070, 0x2071), (0x2074, 0x208E), (0x2090, 0x209C), (0x20A0, 0x20B5),
    (0x20B8, 0x20BA), (0x20BD, 0x20BD), (0x2102, 0x2102), (0x2105, 0x2105), (0x210D, 0x210F),
    (0x2115, 0x2117), (0x2119, 0x211A), (0x211D, 0x211D), (0x2122, 0x2122), (0x2124, 0x2124),
    (0x2126, 0x2126), (0x212A, 0x212B), (0x212E, 0x212E), (0x2148, 0x2148), (0x2150, 0x2151),
    (0x2153, 0x215F), (0x2189, 0x2189), (0x2190, 0x2213), (0x2215, 0x2215), (0x2217, 0x2220),
    (0x2223, 0x2223), (0x2227, 0x222D), (0x2234, 0x223D), (0x2241, 0x2269), (0x226D, 0x228B),
    (0x228D, 0x22A5), (0x22B2, 0x22B5), (0x22B8, 0x22B8), (0x22C2, 0x22C6), (0x22CD, 0x22D1),
    (0x22DA, 0x22E9), (0x22EF, 0x22EF), (0x2300, 0x2306), (0x2308, 0x2311), (0x2318, 0x2319),
    (0x231C, 0x2321), (0x2325, 0x2328), (0x232B, 0x232B), (0x2373, 0x2375), (0x237A, 0x237A),
    (0x237D, 0x237D), (0x239B, 0x23AE), (0x23CE, 0x23CF), (0x2500, 0x25FC), (0x25FF, 0x2613),
    (0x2616, 0x262F), (0x2638, 0x2647), (0x2654, 0x267E), (0x2680, 0x2689), (0x2690, 0x2692),
    (0x2694, 0x269C), (0x26A0, 0x26A0), (0x26B0, 0x26B1), (0x2701, 0x2704), (0x2706, 0x2709),
    (0x270C, 0x2727), (0x2729, 0x274B), (0x274D, 0x274D), (0x274F, 0x2752), (0x2756, 0x2756),
    (0x2758, 0x275E), (0x2761, 0x2775), (0x2794, 0x2794), (0x2798, 0x27AF), (0x27B1, 0x27BE),
    (0xFFFD, 0xFFFD),
];

/// The number of characters in the full repertoire.
pub const FULL_COUNT: u32 = 2461;

/// Whether `code` is in the full repertoire.
pub fn is_full(code: u16) -> bool {
    let i = FULL.partition_point(|&(_, hi)| hi < code);
    FULL.get(i).is_some_and(|&(lo, _)| lo <= code)
}

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
    fn full_repertoire() {
        let n: u32 = FULL.iter().map(|&(a, b)| (b - a + 1) as u32).sum();
        assert_eq!(n, FULL_COUNT);
        assert!(FULL.windows(2).all(|w| w[0].1 + 1 < w[1].0), "sorted, apart");
        for &(lo, hi) in &FULL {
            for c in lo..=hi {
                assert!(!crate::wide::is_wide(c), "U+{c:04X} is narrow");
            }
        }
        for c in 0..=0xFFFFu16 {
            if is_basic(c) {
                assert!(is_full(c), "basic U+{c:04X} is full");
            }
        }
        for c in ['é', 'Ł', 'Ω', 'λ', 'Ж', 'я', '€', '→', '∀', '≤', '⌂', '■', '♥', '★', '✓', '\u{FFFD}'] {
            assert!(is_full(c as u16), "{c}");
        }
        for c in ['你', 'ア', '한', 'א', 'ب', '\u{2614}', '\u{FFFC}', '\u{1F}', '\u{9F}'] {
            assert!(!is_full(c as u16), "{c}");
        }
    }

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

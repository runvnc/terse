//! Character width for the terminal (SPEC 8.1).
//!
//! `WIDE` lists the code points U+0000-U+FFFF whose Unicode 16.0.0 East Asian
//! Width property is W or F, including unassigned code points in the ranges
//! whose default is W. Generated from Python 3.14's `unicodedata`
//! (`unidata_version` 16.0.0) with:
//!
//! ```text
//! [cp for cp in range(0x10000) if unicodedata.east_asian_width(chr(cp)) in ("W", "F")]
//! ```
//!
//! collapsed into inclusive ranges.

const WIDE: [(u16, u16); 59] = [
    (0x1100, 0x115F), (0x231A, 0x231B), (0x2329, 0x232A), (0x23E9, 0x23EC),
    (0x23F0, 0x23F0), (0x23F3, 0x23F3), (0x25FD, 0x25FE), (0x2614, 0x2615),
    (0x2630, 0x2637), (0x2648, 0x2653), (0x267F, 0x267F), (0x268A, 0x268F),
    (0x2693, 0x2693), (0x26A1, 0x26A1), (0x26AA, 0x26AB), (0x26BD, 0x26BE),
    (0x26C4, 0x26C5), (0x26CE, 0x26CE), (0x26D4, 0x26D4), (0x26EA, 0x26EA),
    (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD),
    (0x2705, 0x2705), (0x270A, 0x270B), (0x2728, 0x2728), (0x274C, 0x274C),
    (0x274E, 0x274E), (0x2753, 0x2755), (0x2757, 0x2757), (0x2795, 0x2797),
    (0x27B0, 0x27B0), (0x27BF, 0x27BF), (0x2B1B, 0x2B1C), (0x2B50, 0x2B50),
    (0x2B55, 0x2B55), (0x2E80, 0x2E99), (0x2E9B, 0x2EF3), (0x2F00, 0x2FD5),
    (0x2FF0, 0x303E), (0x3041, 0x3096), (0x3099, 0x30FF), (0x3105, 0x312F),
    (0x3131, 0x318E), (0x3190, 0x31E5), (0x31EF, 0x321E), (0x3220, 0x3247),
    (0x3250, 0xA48C), (0xA490, 0xA4C6), (0xA960, 0xA97C), (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF), (0xFE10, 0xFE19), (0xFE30, 0xFE52), (0xFE54, 0xFE66),
    (0xFE68, 0xFE6B), (0xFF01, 0xFF60), (0xFFE0, 0xFFE6),
];

/// Whether `ch` occupies two terminal cells (SPEC 8.1).
pub fn is_wide(ch: u16) -> bool {
    WIDE.binary_search_by(|&(lo, hi)| {
        if hi < ch {
            std::cmp::Ordering::Less
        } else if lo > ch {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_disjoint() {
        assert!(WIDE.iter().all(|&(lo, hi)| lo <= hi));
        assert!(WIDE.windows(2).all(|w| w[0].1 + 1 < w[1].0));
    }

    #[test]
    fn samples() {
        for ch in ['你', '好', 'ア', '한', '　', 'Ａ', '￦', '⌚', '☰'] {
            assert!(is_wide(ch as u16), "{ch} U+{:04X}", ch as u32);
        }
        // Ambiguous (A), neutral (N), narrow (Na) and halfwidth (H) are narrow.
        for ch in ['a', 'é', '★', '♥', '─', '✓', 'ｱ', '\u{FFFD}', '\u{FFFF}', '\u{303F}'] {
            assert!(!is_wide(ch as u16), "{ch} U+{:04X}", ch as u32);
        }
        // Unassigned, but in a default-W block.
        assert!(is_wide(0xFA6E));
    }
}

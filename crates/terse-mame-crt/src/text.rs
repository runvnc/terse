//! The terminal's screen as strokes: each cell's character from the stroke
//! font ([`crate::stroke`]) in its foreground colour, the cursor as a
//! stroke under its cell, in reading order (the order the beam draws them).

use terse_core::{COLS, ROWS, Terminal};

use crate::stroke::{CELL_H, CELL_W, Stroke, strokes};

/// A stroke on the screen, in screen units, with its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lit {
    pub s: Stroke,
    pub rgb: [u8; 3],
}

/// The terminal as shown: the status line (a fault) over the bottom row,
/// as terse-host and terse-dev-terminal show it (colours 15 on 1, the row
/// cleared, the cursor hidden).
pub fn shown<'a>(t: &'a Terminal, status: &str) -> std::borrow::Cow<'a, Terminal> {
    if status.is_empty() {
        return std::borrow::Cow::Borrowed(t);
    }
    let mut t = t.clone();
    t.set_colours(15, 1);
    t.set_cursor(0, ROWS as u32 - 1);
    t.command(0x04);
    t.write(format!(" {status}").as_bytes());
    t.command(0x06);
    std::borrow::Cow::Owned(t)
}

/// The cells as drawn (terse-core's `draw_cells`: the basic repertoire
/// and the placeholder without full text, wide characters with it): per
/// cell, the code, whether it is a wide character's left half, its
/// foreground and background; a wide character's right half and a blank
/// cell have code 0.
pub fn cells(t: &Terminal) -> Vec<(u32, bool, u8, u8)> {
    let mut codes: Vec<(u32, bool)> = vec![(0, false)];
    let drawn = terse_core::text::draw_cells(t, |code, wide| {
        let slot = codes.len() as u32;
        let code = if code == terse_core::text::PLACEHOLDER_CODE { 0xFFFD } else { code as u32 };
        codes.push((code, wide));
        if wide {
            codes.push((0, false));
        }
        slot
    });
    drawn
        .into_iter()
        .map(|(slot, fg, bg)| {
            let (code, wide) = codes[slot as usize];
            (code, wide, fg, bg)
        })
        .collect()
}

/// The strokes of the screen, row by row, and the cursor's.
pub fn screen(t: &Terminal) -> Vec<Lit> {
    let cells = cells(t);
    let palette = t.palette();
    let mut out = Vec::new();
    let mut buf = Vec::new();
    for row in 0..ROWS {
        for col in 0..COLS {
            let (code, wide, fg, _) = cells[row * COLS + col];
            if code == 0 {
                continue;
            }
            buf.clear();
            strokes(code, wide, &mut buf);
            let (ox, oy) = (col as f32 * CELL_W, row as f32 * CELL_H);
            for s in &buf {
                out.push(Lit { s: Stroke { x0: s.x0 + ox, y0: s.y0 + oy, x1: s.x1 + ox, y1: s.y1 + oy, ..*s }, rgb: palette[fg as usize] });
            }
        }
    }
    if t.cursor_visible() {
        let (col, row) = t.cursor();
        let (col, row) = (col as usize, row as usize);
        if col < COLS && row < ROWS {
            let fg = cells[row * COLS + col].2;
            let (x, y) = (col as f32 * CELL_W, row as f32 * CELL_H + CELL_H - 1.0);
            out.push(Lit { s: Stroke { x0: x + 0.75, y0: y, x1: x + CELL_W - 0.75, y1: y, width: 1.5, level: 1.0 }, rgb: palette[fg as usize] });
        }
    }
    merge(out)
}

/// Joins strokes that continue one another along a row or a column (a
/// rule of box-drawing characters across cells) into one stroke, as a
/// vector terminal would draw a rule: one beam, not a bead at each cell's
/// edge where two round ends would add their light.
pub fn merge(strokes: Vec<Lit>) -> Vec<Lit> {
    let mut out: Vec<Lit> = Vec::with_capacity(strokes.len());
    let mut open: std::collections::HashMap<(bool, u32, u32, u32, [u8; 3]), usize> = std::collections::HashMap::new();
    for l in strokes {
        let s = l.s;
        let horizontal = s.y0 == s.y1 && s.x1 > s.x0;
        let vertical = s.x0 == s.x1 && s.y1 > s.y0;
        if !(horizontal || vertical) {
            out.push(l);
            continue;
        }
        let fixed = if horizontal { s.y0 } else { s.x0 };
        let key = (horizontal, fixed.to_bits(), s.width.to_bits(), s.level.to_bits(), l.rgb);
        if let Some(&i) = open.get(&key) {
            let prev = &mut out[i].s;
            let (end, start) = if horizontal { (prev.x1, s.x0) } else { (prev.y1, s.y0) };
            if end == start {
                prev.x1 = s.x1;
                prev.y1 = s.y1;
                continue;
            }
        }
        open.insert(key, out.len());
        out.push(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_is_one_stroke() {
        let mut t = Terminal::default();
        t.write("──────────".as_bytes());
        t.command(0x06);
        let s = screen(&t);
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!((s[0].s.x0, s[0].s.x1), (0.0, 80.0));
    }

    #[test]
    fn text_in_reading_order() {
        let mut t = Terminal::default();
        t.write(b"Hi");
        let s = screen(&t);
        // H: three strokes in the first cell; i: its stem and its dot in
        // the second; the cursor under the third.
        assert!(s[0].s.x0 < 8.0 && s.iter().any(|l| l.s.x0 >= 8.0));
        let last = s.last().unwrap().s;
        assert!(last.x0 >= 16.0 && last.y0 > 14.0, "{last:?}");
    }
}

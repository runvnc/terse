//! The terminal's glyphs (SPEC §terminal.text), from the fonts loaded when
//! the host started ([`TerminalFonts`]): the monospace font, which sets the
//! cell, then the fonts behind it (the built-in one, if the user replaced
//! it) and the user's extra fonts, in order. A character none of them has
//! is drawn as the placeholder in each of its cells; nothing is looked for
//! elsewhere.
//!
//! The first font is parsed whole by fontdue when the glyphs are made (it
//! is small); the others are read a glyph at a time with swash as their
//! characters are first shown, so that an extra CJK collection of 20 MB
//! costs nothing until then. Each glyph is rasterised once, when it is
//! first shown, and cached by the renderers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fontdue::{Font, FontSettings};
use swash::scale::{Render, ScaleContext};
use swash::zeno::Format;

use crate::display::GlyphSource;
use crate::fonts::{FontData, TerminalFonts};

/// Glyphs at one size. Copies at other sizes (`resized`, one per machine
/// screen) share the fonts and what was learned about them.
#[derive(Clone)]
pub struct TermGlyphs {
    px: f32,
    cell: (u32, u32),
    baseline: f32,
    faces: Arc<Mutex<Faces>>,
}

/// The fonts, loaded once for every size.
struct Faces {
    /// The first font, parsed whole.
    primary: Font,
    primary_label: String,
    /// The fonts behind it, read a glyph at a time.
    others: Vec<LazyFont>,
    scaler: ScaleContext,
    /// For each character the primary lacks: the index of the first other
    /// font that has it, or none.
    found: HashMap<char, Option<usize>>,
}

/// A font whose glyphs are read and drawn one at a time (with swash).
struct LazyFont {
    data: FontData,
    offset: u32,
    key: swash::CacheKey,
}

impl LazyFont {
    fn new(data: &FontData) -> Option<Self> {
        let f = swash::FontRef::from_index(data.data(), 0)?;
        Some(LazyFont { data: data.clone(), offset: f.offset, key: f.key })
    }

    fn font(&self) -> swash::FontRef<'_> {
        swash::FontRef { data: self.data.data(), offset: self.offset, key: self.key }
    }

    fn has_glyph(&self, ch: char) -> bool {
        self.font().charmap().map(ch) != 0
    }
}

impl TermGlyphs {
    /// The built-in monospace font at `px` pixels.
    pub fn builtin(px: f32) -> Self {
        Self::new(px, &TerminalFonts::builtin()).expect("the built-in font")
    }

    /// `px` is the font size in pixels.
    pub fn new(px: f32, fonts: &TerminalFonts) -> Result<Self, String> {
        let first = fonts.mono.first().ok_or("no monospace font")?;
        let primary = Font::from_bytes(first.data(), FontSettings::default()).map_err(|e| format!("{}: {e}", first.label))?;
        let others = fonts.all().skip(1).filter_map(LazyFont::new).collect();
        // SPEC §screen: screen units are square and a cell is 8 x 16 of them, so
        // the cell is exactly 1:2. Its width is the monospace font's advance;
        // the text body (ascent to descent) is centred in the height, which
        // is within a pixel or two of the font's line height for usual
        // monospace fonts.
        let line = primary.horizontal_line_metrics(px).ok_or("the font has no line metrics")?;
        let width = primary.metrics('0', px).advance_width.round().max(1.0) as u32;
        let cell = (width, 2 * width);
        let body = line.ascent - line.descent;
        let baseline = (line.ascent + (cell.1 as f32 - body) / 2.0).round();
        let faces = Faces { primary, primary_label: first.label.clone(), others, scaler: ScaleContext::new(), found: HashMap::new() };
        Ok(TermGlyphs { px, cell, baseline, faces: Arc::new(Mutex::new(faces)) })
    }

    /// The font that sets the cell, for the user.
    pub fn primary_family(&self) -> String {
        self.faces.lock().unwrap().primary_label.clone()
    }
}

/// A glyph's coverage mask, placed as fontdue places one: `xmin` from the
/// pen position, `ymin` from the baseline up to the mask's bottom edge.
struct Mask {
    xmin: i32,
    ymin: i32,
    width: usize,
    height: usize,
    advance: f32,
    bitmap: Vec<u8>,
}

/// Which font draws a character.
#[derive(Clone, Copy, PartialEq)]
enum Which {
    Primary,
    Other(usize),
}

impl Faces {
    fn which(&mut self, ch: char) -> Option<Which> {
        if self.primary.has_glyph(ch) {
            return Some(Which::Primary);
        }
        let others = &self.others;
        let i = *self.found.entry(ch).or_insert_with(|| others.iter().position(|f| f.has_glyph(ch)));
        i.map(Which::Other)
    }

    /// Font `i`'s glyph for `ch` at `px` (as fontdue's `rasterize` gives one),
    /// and the font's ascent and descent (below the baseline: negative) at
    /// `line_px`.
    fn other_mask(&mut self, i: usize, ch: char, px: f32, line_px: f32) -> (Mask, (f32, f32)) {
        let font = self.others[i].font();
        let glyph = font.charmap().map(ch);
        let advance = font.glyph_metrics(&[]).scale(px).advance_width(glyph);
        let line = font.metrics(&[]).scale(line_px);
        let mut scaler = self.scaler.builder(font).size(px).hint(false).build();
        let mask = match Render::new(&[swash::scale::Source::Outline]).format(Format::Alpha).render(&mut scaler, glyph) {
            Some(img) => Mask {
                xmin: img.placement.left,
                ymin: img.placement.top - img.placement.height as i32,
                width: img.placement.width as usize,
                height: img.placement.height as usize,
                advance,
                bitmap: img.data,
            },
            None => Mask { xmin: 0, ymin: 0, width: 0, height: 0, advance, bitmap: Vec::new() },
        };
        (mask, (line.ascent, -line.descent))
    }

    fn other_advance(&self, i: usize, ch: char, px: f32) -> f32 {
        let font = self.others[i].font();
        font.glyph_metrics(&[]).scale(px).advance_width(font.charmap().map(ch))
    }
}

/// The placeholder (SPEC §terminal.text) in each of `cells` cells of `w` × `h`.
fn placeholders(w: u32, h: u32, cells: u32, out: &mut [u8]) {
    let mut one = vec![0; (w * h) as usize];
    terse_core::text::placeholder(w, h, &mut one);
    for y in 0..h as usize {
        for c in 0..cells as usize {
            let at = y * (cells * w) as usize + c * w as usize;
            out[at..at + w as usize].copy_from_slice(&one[y * w as usize..(y + 1) * w as usize]);
        }
    }
}

impl GlyphSource for TermGlyphs {
    fn cell_size(&self) -> (u32, u32) {
        self.cell
    }

    fn resized(&self, width: u32) -> Box<dyn GlyphSource> {
        let mut g = self.clone();
        g.px = self.px * width as f32 / self.cell.0 as f32;
        g.cell = (width, 2 * width);
        // The text body centred in the cell, as at the native size.
        let line = self.faces.lock().unwrap().primary.horizontal_line_metrics(g.px);
        if let Some(line) = line {
            let body = line.ascent - line.descent;
            g.baseline = (line.ascent + (g.cell.1 as f32 - body) / 2.0).round();
        }
        Box::new(g)
    }

    fn rasterize(&mut self, ch: char, cells: u32, out: &mut [u8]) {
        let mut faces = self.faces.lock().unwrap();
        let Some(which) = faces.which(ch) else {
            if !ch.is_control() && !ch.is_whitespace() {
                placeholders(self.cell.0, self.cell.1, cells, out);
            }
            return;
        };
        let (cw, ch_px, baseline, px) = ((self.cell.0 * cells) as i32, self.cell.1 as i32, self.baseline, self.px);
        // Shrink glyphs wider than their cells (e.g. a wide character drawn
        // in one cell because no continuation cell follows it).
        let shrink = |advance: f32| if advance > cw as f32 * 1.05 { cw as f32 / advance } else { 1.0 };
        let (m, line, scale) = match which {
            Which::Primary => {
                let font = &faces.primary;
                let scale = shrink(font.metrics(ch, px).advance_width);
                let (m, bitmap) = font.rasterize(ch, px * scale);
                let mask = Mask { xmin: m.xmin, ymin: m.ymin, width: m.width, height: m.height, advance: m.advance_width, bitmap };
                (mask, font.horizontal_line_metrics(px).map(|l| (l.ascent, l.descent)), scale)
            }
            Which::Other(i) => {
                let scale = shrink(faces.other_advance(i, ch, px));
                let (mask, line) = faces.other_mask(i, ch, px * scale, px);
                (mask, Some(line), scale)
            }
        };
        // Centre shrunk glyphs vertically around the normal text body.
        let lift = (1.0 - scale) * baseline / 2.0;
        let x0 = m.xmin + ((cw as f32 - m.advance) / 2.0).round().max(0.0) as i32;
        let y0 = (baseline - lift).round() as i32 - m.ymin - m.height as i32;
        // Box drawing (U+2500-U+257F) is designed to span the font's ascent to
        // descent; stretch that band over the whole cell height so lines join
        // across rows.
        let band = match line {
            Some((ascent, descent)) if ('\u{2500}'..='\u{257F}').contains(&ch) => (baseline - ascent, ascent - descent),
            _ => (0.0, ch_px as f32),
        };
        for ty in 0..ch_px {
            let src_y = (band.0 + (ty as f32 + 0.5) * band.1 / ch_px as f32).floor() as i32;
            let y = src_y - y0;
            if !(0..m.height as i32).contains(&y) {
                continue;
            }
            for x in 0..m.width as i32 {
                let tx = x0 + x;
                if (0..cw).contains(&tx) {
                    let v = m.bitmap[(y * m.width as i32 + x) as usize];
                    let o = &mut out[(ty * cw + tx) as usize];
                    *o = (*o).max(v);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(g: &mut TermGlyphs, ch: char, cells: u32) -> Vec<u8> {
        let (w, h) = g.cell_size();
        let mut out = vec![0; (w * h * cells) as usize];
        g.rasterize(ch, cells, &mut out);
        out
    }

    fn placeholder(g: &TermGlyphs, cells: u32) -> Vec<u8> {
        let (w, h) = g.cell_size();
        let mut out = vec![0; (w * h * cells) as usize];
        placeholders(w, h, cells, &mut out);
        out
    }

    /// Every character of the full repertoire is drawn by the built-in
    /// monospace font itself (SPEC §terminal.text), none as the placeholder.
    #[test]
    fn the_builtin_font_draws_the_full_repertoire() {
        let mut g = TermGlyphs::builtin(crate::FONT_PX);
        let mut faces = g.faces.lock().unwrap();
        for &(lo, hi) in &terse_core::text::FULL {
            for c in lo..=hi {
                let ch = char::from_u32(c as u32).unwrap();
                assert!(faces.which(ch) == Some(Which::Primary), "U+{c:04X} is in the built-in monospace font");
            }
        }
        drop(faces);
        let empty = vec![0; (g.cell.0 * g.cell.1) as usize];
        for ch in ['A', 'λ', 'Ж', '♥', '★', '✓', '⌂', '■', '→'] {
            let d = drawn(&mut g, ch, 1);
            assert_ne!(d, empty, "{ch} has ink");
            assert_ne!(d, placeholder(&g, 1), "{ch} is not the placeholder");
        }
    }

    /// A character no loaded font has is the placeholder, in each of its
    /// cells; a control code is blank.
    #[test]
    fn missing_characters_are_placeholders() {
        let mut g = TermGlyphs::builtin(crate::FONT_PX);
        assert_eq!(drawn(&mut g, '你', 2), placeholder(&g, 2));
        assert_eq!(drawn(&mut g, 'א', 1), placeholder(&g, 1));
        assert!(drawn(&mut g, '\u{85}', 1).iter().all(|&v| v == 0));
    }

    /// An extra font adds the characters it has, after the built-in font.
    #[test]
    fn an_extra_font_adds_coverage() {
        let Some(cjk) = crate::fonts::tests::system_cjk() else {
            eprintln!("no CJK font on this system: skipped");
            return;
        };
        let config = tempfile::tempdir().unwrap();
        let s = terse_net::fontlist::FontSettings { extra: vec![cjk], ..Default::default() };
        terse_net::fontlist::write(config.path(), &s).unwrap();
        let fonts = TerminalFonts::load(Some(config.path()));
        assert!(fonts.notes.is_empty(), "{:?}", fonts.notes);
        let mut g = TermGlyphs::new(crate::FONT_PX, &fonts).unwrap();
        let d = drawn(&mut g, '你', 2);
        assert_ne!(d, placeholder(&g, 2));
        assert!(d.iter().any(|&v| v > 0));
        assert!(g.faces.lock().unwrap().which('A') == Some(Which::Primary), "the built-in font comes first");
    }
}

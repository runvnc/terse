//! Glyphs from system fonts: the fontconfig monospace font, with fallback to
//! other installed fonts for characters it lacks (e.g. CJK).

use std::collections::{HashMap, HashSet};

use fontdb::{Database, Family, ID, Query, Style, Weight};
use fontdue::{Font, FontSettings};

use crate::renderer::GlyphSource;

pub struct SystemGlyphs {
    db: Database,
    px: f32,
    cell: (u32, u32),
    baseline: f32,
    primary: ID,
    /// Other regular-weight upright faces, monospaced ones first.
    fallbacks: Vec<ID>,
    fonts: HashMap<ID, Option<Font>>,
    /// The fallback face that last had a glyph: tried before the full scan,
    /// since text in another script tends to come from one font.
    last_fallback: Option<ID>,
    /// Characters no face has, so they are not scanned for again.
    missing: HashSet<char>,
}

impl SystemGlyphs {
    /// `px` is the font size in pixels.
    pub fn new(px: f32) -> Result<Self, String> {
        let mut db = Database::new();
        db.load_system_fonts();
        let regular = |families: &[Family]| {
            db.query(&Query { families, weight: Weight::NORMAL, style: Style::Normal, ..Query::default() })
        };
        let primary = regular(&[Family::Monospace])
            .or_else(|| db.faces().find(|f| f.monospaced).map(|f| f.id))
            .ok_or("no monospace font found on this system")?;

        let mut fallbacks: Vec<_> = db
            .faces()
            .filter(|f| f.id != primary && f.style == Style::Normal && f.weight == Weight::NORMAL)
            .map(|f| (!f.monospaced, f.families.first().map(|(n, _)| n.clone()).unwrap_or_default(), f.id))
            .collect();
        fallbacks.sort();
        let fallbacks = fallbacks.into_iter().map(|(_, _, id)| id).collect();

        let mut glyphs = SystemGlyphs {
            db,
            px,
            cell: (1, 1),
            baseline: 0.0,
            primary,
            fallbacks,
            fonts: HashMap::new(),
            last_fallback: None,
            missing: HashSet::new(),
        };
        let font = glyphs.font(primary).ok_or("cannot load the monospace font")?;
        let line = font.horizontal_line_metrics(px).ok_or("monospace font has no line metrics")?;
        let advance = font.metrics('0', px).advance_width;
        // SPEC 5.4: screen units are square and a cell is 8 x 16 of them, so
        // the cell is exactly 1:2. Its width is the font's advance; the text
        // body (ascent to descent) is centred in the height, which is within
        // a pixel or two of the font's line height for usual monospace fonts.
        let width = advance.round().max(1.0) as u32;
        glyphs.cell = (width, 2 * width);
        let body = line.ascent - line.descent;
        glyphs.baseline = (line.ascent + (glyphs.cell.1 as f32 - body) / 2.0).round();
        Ok(glyphs)
    }

    pub fn primary_family(&self) -> String {
        self.db.face(self.primary).and_then(|f| f.families.first()).map_or(String::new(), |(n, _)| n.clone())
    }

    fn font(&mut self, id: ID) -> Option<&Font> {
        let db = &self.db;
        self.fonts
            .entry(id)
            .or_insert_with(|| {
                db.with_face_data(id, |data, index| {
                    let settings = FontSettings { collection_index: index, ..FontSettings::default() };
                    Font::from_bytes(data, settings).ok()
                })
                .flatten()
            })
            .as_ref()
    }

    /// A face that has a glyph for `ch`: the primary, else the fallback that
    /// last had one, else the first fallback that does. Coverage is checked
    /// with ttf-parser so fallback fonts are only fully loaded when needed;
    /// that scan reads every font file, so its results are remembered.
    fn face_for(&mut self, ch: char) -> Option<ID> {
        if self.font(self.primary).is_some_and(|f| f.has_glyph(ch)) {
            return Some(self.primary);
        }
        if self.missing.contains(&ch) {
            return None;
        }
        if let Some(id) = self.last_fallback
            && self.font(id).is_some_and(|f| f.has_glyph(ch))
        {
            return Some(id);
        }
        let db = &self.db;
        let Some(id) = self.fallbacks.iter().copied().find(|&id| {
            db.with_face_data(id, |data, index| {
                ttf_parser::Face::parse(data, index).is_ok_and(|f| f.glyph_index(ch).is_some())
            })
            .unwrap_or(false)
        }) else {
            self.missing.insert(ch);
            return None;
        };
        self.last_fallback = Some(id);
        self.font(id).map(|_| id)
    }
}

impl GlyphSource for SystemGlyphs {
    fn cell_size(&self) -> (u32, u32) {
        self.cell
    }

    fn rasterize(&mut self, ch: char, cells: u32, out: &mut [u8]) {
        let Some(id) = self.face_for(ch) else { return };
        let (cw, ch_px, baseline, px) = ((self.cell.0 * cells) as i32, self.cell.1 as i32, self.baseline, self.px);
        let font = self.font(id).expect("face_for loaded it");
        // Shrink glyphs wider than their cells (e.g. a wide character drawn
        // in one cell because no continuation cell follows it).
        let advance = font.metrics(ch, px).advance_width;
        let scale = if advance > cw as f32 * 1.05 { cw as f32 / advance } else { 1.0 };
        let (m, bitmap) = font.rasterize(ch, px * scale);
        // Centre shrunk glyphs vertically around the normal text body.
        let lift = (1.0 - scale) * baseline / 2.0;
        let x0 = m.xmin + ((cw as f32 - m.advance_width) / 2.0).round().max(0.0) as i32;
        let y0 = (baseline - lift).round() as i32 - m.ymin - m.height as i32;
        // Box drawing (U+2500-U+257F) is designed to span the font's ascent to
        // descent; stretch that band over the whole cell height so lines join
        // across rows.
        let band = match font.horizontal_line_metrics(px) {
            Some(l) if ('\u{2500}'..='\u{257F}').contains(&ch) => (baseline - l.ascent, l.ascent - l.descent),
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
                    let v = bitmap[(y * m.width as i32 + x) as usize];
                    let o = &mut out[(ty * cw + tx) as usize];
                    *o = (*o).max(v);
                }
            }
        }
    }
}

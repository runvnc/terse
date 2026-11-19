//! Glyphs from system fonts: the fontconfig monospace font (or one chosen
//! with `--font`), with fallback to other installed fonts for characters it
//! lacks (e.g. CJK).
//!
//! A proportional font (`--font`, for trying the look) still fills the
//! fixed grid: the cell is the one the default monospace font gives, the
//! size is chosen so the font's text body (ascent to descent) is as tall as
//! that font's, and each glyph is centred in its cell (or two cells, for a
//! wide character), squeezed horizontally only if it would overflow.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use fontdb::{Database, Family, ID, Query, Source, Style, Weight};
use fontdue::{Font, FontSettings};

use crate::display::GlyphSource;

/// Glyphs at one size. Copies at other sizes (`resized`, one per machine
/// screen) share the fonts and what was learned about them.
#[derive(Clone)]
pub struct SystemGlyphs {
    px: f32,
    cell: (u32, u32),
    baseline: f32,
    primary: ID,
    /// The primary face is proportional: its glyphs are centred in the
    /// cell and squeezed horizontally to fit (see the module comment).
    proportional: bool,
    faces: Arc<Mutex<Faces>>,
}

/// The fonts, loaded once for every size.
struct Faces {
    db: Database,
    /// Other regular-weight upright faces, monospaced ones first.
    fallbacks: Vec<ID>,
    fonts: HashMap<ID, Option<Font>>,
    /// The fallback face that last had a glyph: tried before the full scan,
    /// since text in another script tends to come from one font.
    last_fallback: Option<ID>,
    /// Characters no face has, so they are not scanned for again.
    missing: HashSet<char>,
    /// The face found for a character that the primary lacks.
    found: HashMap<char, ID>,
}

impl SystemGlyphs {
    /// `px` is the font size in pixels (for a proportional `font`, that of
    /// the default monospace font it stands in for). `font` is a family
    /// name or the path of a font file; `None`: the system's monospace font.
    pub fn new(px: f32, font: Option<&str>) -> Result<Self, String> {
        let base_px = px;
        let mut db = Database::new();
        db.load_system_fonts();
        let regular = |db: &Database, families: &[Family]| {
            db.query(&Query { families, weight: Weight::NORMAL, style: Style::Normal, ..Query::default() })
        };
        let monospace = regular(&db, &[Family::Monospace]).or_else(|| db.faces().find(|f| f.monospaced).map(|f| f.id));
        let primary = match font {
            None => monospace.ok_or("no monospace font found on this system")?,
            Some(name) if Path::new(name).is_file() => {
                let ids = db.load_font_source(Source::File(name.into()));
                // The regular upright face of a collection, else its first.
                let regular_face = ids.iter().copied().find(|&id| {
                    db.face(id).is_some_and(|f| f.style == Style::Normal && f.weight == Weight::NORMAL)
                });
                regular_face.or(ids.first().copied()).ok_or(format!("{name}: not a font file this host can read"))?
            }
            Some(name) => regular(&db, &[Family::Name(name)])
                .or_else(|| {
                    // Any face of the family, upright ones first.
                    let named = |f: &&fontdb::FaceInfo| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));
                    let faces: Vec<_> = db.faces().filter(named).collect();
                    faces.iter().find(|f| f.style == Style::Normal).or(faces.first()).map(|f| f.id)
                })
                .ok_or(format!("no font family named {name:?} (fc-list : family lists them)"))?,
        };
        let proportional = !db.face(primary).is_some_and(|f| f.monospaced);

        let mut fallbacks: Vec<_> = db
            .faces()
            .filter(|f| f.id != primary && f.style == Style::Normal && f.weight == Weight::NORMAL)
            .map(|f| (!f.monospaced, f.families.first().map(|(n, _)| n.clone()).unwrap_or_default(), f.id))
            .collect();
        fallbacks.sort();
        let fallbacks = fallbacks.into_iter().map(|(_, _, id)| id).collect();

        let mut faces = Faces {
            db,
            fallbacks,
            fonts: HashMap::new(),
            last_fallback: None,
            missing: HashSet::new(),
            found: HashMap::new(),
        };
        // SPEC 5.4: screen units are square and a cell is 8 x 16 of them, so
        // the cell is exactly 1:2. Its width is the monospace font's advance
        // (for a proportional font, the default monospace font's, else 0.6
        // of the size); the text body (ascent to descent) is centred in the
        // height, which is within a pixel or two of the font's line height
        // for usual monospace fonts.
        let reference = match (proportional, monospace) {
            (true, Some(mono)) => faces.font(mono).and_then(|f| {
                let line = f.horizontal_line_metrics(px)?;
                Some((f.metrics('0', px).advance_width, line.ascent - line.descent))
            }),
            _ => None,
        };
        let font = faces.font(primary).ok_or("cannot load the font")?;
        let line = font.horizontal_line_metrics(px).ok_or("the font has no line metrics")?;
        let advance = match (proportional, reference) {
            (false, _) => font.metrics('0', px).advance_width,
            (true, Some((advance, _))) => advance,
            (true, None) => 0.6 * px,
        };
        // A proportional font is sized so its body matches the reference's.
        let body_px = |px: f32| font.horizontal_line_metrics(px).map_or(px, |l| l.ascent - l.descent);
        let px = match reference {
            Some((_, body)) if proportional => px * body / body_px(px),
            _ => px,
        };
        let line = if px == base_px { line } else { font.horizontal_line_metrics(px).unwrap_or(line) };
        let width = advance.round().max(1.0) as u32;
        let cell = (width, 2 * width);
        let body = line.ascent - line.descent;
        let baseline = (line.ascent + (cell.1 as f32 - body) / 2.0).round();
        Ok(SystemGlyphs { px, cell, baseline, primary, proportional, faces: Arc::new(Mutex::new(faces)) })
    }

    pub fn primary_family(&self) -> String {
        let faces = self.faces.lock().unwrap();
        let name = faces.db.face(self.primary).and_then(|f| f.families.first()).map_or(String::new(), |(n, _)| n.clone());
        if self.proportional { format!("{name} (proportional, centred in the cells)") } else { name }
    }
}

impl Faces {
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
    fn face_for(&mut self, primary: ID, ch: char) -> Option<ID> {
        if self.font(primary).is_some_and(|f| f.has_glyph(ch)) {
            return Some(primary);
        }
        if self.missing.contains(&ch) {
            return None;
        }
        if let Some(&id) = self.found.get(&ch) {
            return Some(id);
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
        self.found.insert(ch, id);
        self.font(id).map(|_| id)
    }
}

impl GlyphSource for SystemGlyphs {
    fn cell_size(&self) -> (u32, u32) {
        self.cell
    }

    fn resized(&self, width: u32) -> Box<dyn GlyphSource> {
        let mut g = self.clone();
        g.px = self.px * width as f32 / self.cell.0 as f32;
        g.cell = (width, 2 * width);
        // The text body centred in the cell, as at the native size.
        let px = g.px;
        let line = self.faces.lock().unwrap().font(g.primary).and_then(|f| f.horizontal_line_metrics(px));
        if let Some(line) = line {
            let body = line.ascent - line.descent;
            g.baseline = (line.ascent + (g.cell.1 as f32 - body) / 2.0).round();
        }
        Box::new(g)
    }

    fn rasterize(&mut self, ch: char, cells: u32, out: &mut [u8]) {
        let mut faces = self.faces.lock().unwrap();
        let Some(id) = faces.face_for(self.primary, ch) else { return };
        let (cw, ch_px, baseline, px) = ((self.cell.0 * cells) as i32, self.cell.1 as i32, self.baseline, self.px);
        let squeeze = self.proportional && id == self.primary;
        let font = faces.font(id).expect("face_for loaded it");
        let advance = font.metrics(ch, px).advance_width;
        if squeeze {
            return squeezed(font, ch, px, cw, ch_px, baseline, out);
        }
        // Shrink glyphs wider than their cells (e.g. a wide character drawn
        // in one cell because no continuation cell follows it).
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

/// A proportional font's glyph `ch` in a `cw` × `ch_px` cell area: centred
/// horizontally by its advance, and if that is wider than the area,
/// squeezed horizontally (each output column the average of the glyph's
/// columns it covers) to fit; never stretched. Box drawing is not special:
/// proportional fonts rarely make it join across cells anyway.
fn squeezed(font: &Font, ch: char, px: f32, cw: i32, ch_px: i32, baseline: f32, out: &mut [u8]) {
    let (m, bitmap) = font.rasterize(ch, px);
    if m.width == 0 || m.height == 0 {
        return;
    }
    let s = if m.advance_width > cw as f32 { cw as f32 / m.advance_width } else { 1.0 };
    // Glyph space x (pixels from the pen position) to output column.
    let left = (cw as f32 - m.advance_width * s) / 2.0;
    let y0 = baseline.round() as i32 - m.ymin - m.height as i32;
    for tx in 0..cw {
        // The span of glyph space this column covers, in bitmap columns.
        let g0 = (tx as f32 - left) / s - m.xmin as f32;
        let g1 = (tx as f32 + 1.0 - left) / s - m.xmin as f32;
        let (first, last) = (g0.floor().max(0.0) as usize, (g1.ceil() as usize).min(m.width));
        if first >= last {
            continue;
        }
        for ty in 0..ch_px {
            let y = ty - y0;
            if !(0..m.height as i32).contains(&y) {
                continue;
            }
            let row = &bitmap[y as usize * m.width..][..m.width];
            let mut sum = 0.0;
            for (x, &v) in row.iter().enumerate().take(last).skip(first) {
                let overlap = (g1.min(x as f32 + 1.0) - g0.max(x as f32)).max(0.0);
                sum += overlap * v as f32;
            }
            let v = (sum / (g1 - g0)).round().min(255.0) as u8;
            let o = &mut out[(ty * cw + tx) as usize];
            *o = (*o).max(v);
        }
    }
}

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
use swash::scale::{Render, ScaleContext};
use swash::zeno::Format;

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
    /// The primary face (and the monospace face a proportional one is sized
    /// by), parsed whole by fontdue.
    fonts: HashMap<ID, Option<Font>>,
    /// Fallback faces, whose glyphs are read and drawn one at a time (with
    /// swash) as they are first shown. fontdue reads every glyph of a font
    /// when it parses it, which for a CJK collection of 20 MB took 0.7 s, in
    /// the frame that first showed one of its characters, and 300 MB of
    /// memory.
    fallback_fonts: HashMap<ID, Option<LazyFont>>,
    scaler: ScaleContext,
    /// The fallback face that last had a glyph: tried before the full scan,
    /// since text in another script tends to come from one font.
    last_fallback: Option<ID>,
    /// Characters no face has, so they are not scanned for again.
    missing: HashSet<char>,
    /// The face found for a character that the primary lacks.
    found: HashMap<char, ID>,
    /// Which fallback has each character, worked out on a thread of its own
    /// as soon as the fonts are loaded.
    coverage: Coverage,
}

/// For each character, the first fallback face (an index into
/// `Faces::fallbacks`) with a glyph for it. Reading every font's character
/// map takes a few hundred milliseconds on a system with a thousand faces,
/// so it is done once, beside the frame loop, from the moment the host
/// starts. (Looking for a character by reading the font files one by one,
/// in the frame that first showed it, took about 0.1 s for the first
/// character of each script on such a system.)
struct Coverage {
    map: Option<HashMap<char, u32>>,
    building: Option<std::thread::JoinHandle<HashMap<char, u32>>>,
}

impl Coverage {
    fn start(db: &Database, fallbacks: &[ID]) -> Self {
        let (db, order) = (db.clone(), fallbacks.to_vec());
        let building = std::thread::Builder::new()
            .name("terse-font-coverage".into())
            .spawn(move || {
                let mut map = HashMap::new();
                for (i, &id) in order.iter().enumerate() {
                    db.with_face_data(id, |data, index| {
                        let Ok(face) = ttf_parser::Face::parse(data, index) else { return };
                        let Some(cmap) = face.tables().cmap else { return };
                        for sub in cmap.subtables.into_iter().filter(|s| s.is_unicode()) {
                            sub.codepoints(|c| {
                                if let Some(ch) = char::from_u32(c)
                                    && !map.contains_key(&ch)
                                    && face.glyph_index(ch).is_some()
                                {
                                    map.insert(ch, i as u32);
                                }
                            });
                        }
                    });
                }
                map
            })
            .ok();
        Coverage { map: None, building }
    }

    /// The first fallback with a glyph for `ch`, waiting for the map if it
    /// is still being built.
    fn get(&mut self, ch: char) -> Option<u32> {
        if let Some(handle) = self.building.take() {
            self.map = handle.join().ok();
        }
        self.map.as_ref()?.get(&ch).copied()
    }
}

/// The monospace family to use where fontconfig does not choose one (macOS,
/// Windows), if the system has it: fontdb's own choice there is Courier New,
/// a thin typewriter face that suits a terminal grid poorly.
pub(crate) fn platform_monospace(has: impl Fn(&str) -> bool) -> Option<&'static str> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["Menlo", "Monaco"]
    } else if cfg!(windows) {
        &["Cascadia Mono", "Consolas"]
    } else {
        &[]
    };
    candidates.iter().copied().find(|&n| has(n))
}

impl SystemGlyphs {
    /// `px` is the font size in pixels (for a proportional `font`, that of
    /// the default monospace font it stands in for). `font` is a family
    /// name or the path of a font file; `None`: the system's monospace font.
    pub fn new(px: f32, font: Option<&str>) -> Result<Self, String> {
        let base_px = px;
        let mut db = Database::new();
        db.load_system_fonts();
        if let Some(name) = platform_monospace(|n| db.faces().any(|f| f.families.iter().any(|(m, _)| m == n))) {
            db.set_monospace_family(name);
        }
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
        let fallbacks: Vec<ID> = fallbacks.into_iter().map(|(_, _, id)| id).collect();

        let coverage = Coverage::start(&db, &fallbacks);
        let mut faces = Faces {
            coverage,
            db,
            fallbacks,
            fonts: HashMap::new(),
            fallback_fonts: HashMap::new(),
            scaler: ScaleContext::new(),
            last_fallback: None,
            missing: HashSet::new(),
            found: HashMap::new(),
        };
        // SPEC §screen: screen units are square and a cell is 8 x 16 of them, so
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

/// A fallback face's bytes, for swash.
struct LazyFont {
    data: Vec<u8>,
    offset: u32,
    key: swash::CacheKey,
}

impl LazyFont {
    fn font(&self) -> swash::FontRef<'_> {
        swash::FontRef { data: &self.data, offset: self.offset, key: self.key }
    }

    fn has_glyph(&self, ch: char) -> bool {
        self.font().charmap().map(ch) != 0
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

    fn fallback_font(&mut self, id: ID) -> Option<&LazyFont> {
        let db = &self.db;
        self.fallback_fonts
            .entry(id)
            .or_insert_with(|| {
                db.with_face_data(id, |data, index| {
                    let f = swash::FontRef::from_index(data, index as usize)?;
                    Some(LazyFont { data: data.to_vec(), offset: f.offset, key: f.key })
                })
                .flatten()
            })
            .as_ref()
    }

    /// A face that has a glyph for `ch`: the primary, else the fallback that
    /// last had one, else the first fallback that does (from the coverage
    /// map).
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
            && self.fallback_font(id).is_some_and(|f| f.has_glyph(ch))
        {
            return Some(id);
        }
        let Some(id) = self.coverage.get(ch).and_then(|i| self.fallbacks.get(i as usize).copied()) else {
            self.missing.insert(ch);
            return None;
        };
        self.last_fallback = Some(id);
        self.found.insert(ch, id);
        self.fallback_font(id).map(|_| id)
    }

    /// Fallback face `id`'s advance for `ch` at `px`.
    fn fallback_advance(&mut self, id: ID, ch: char, px: f32) -> Option<f32> {
        let font = self.fallback_font(id)?.font();
        Some(font.glyph_metrics(&[]).scale(px).advance_width(font.charmap().map(ch)))
    }

    /// Fallback face `id`'s glyph for `ch` at `px` (as fontdue's `rasterize`
    /// gives one), and the face's ascent and descent (below the baseline:
    /// negative) at `line_px`.
    fn fallback_mask(&mut self, id: ID, ch: char, px: f32, line_px: f32) -> Option<(Mask, (f32, f32))> {
        let font = self.fallback_fonts.get(&id)?.as_ref()?.font();
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
        Some((mask, (line.ascent, -line.descent)))
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
        // Shrink glyphs wider than their cells (e.g. a wide character drawn
        // in one cell because no continuation cell follows it).
        let shrink = |advance: f32| if advance > cw as f32 * 1.05 { cw as f32 / advance } else { 1.0 };
        let (m, line, scale) = if id == self.primary {
            let font = faces.font(id).expect("face_for loaded it");
            if self.proportional {
                return squeezed(font, ch, px, cw, ch_px, baseline, out);
            }
            let scale = shrink(font.metrics(ch, px).advance_width);
            let (m, bitmap) = font.rasterize(ch, px * scale);
            let mask = Mask { xmin: m.xmin, ymin: m.ymin, width: m.width, height: m.height, advance: m.advance_width, bitmap };
            (mask, font.horizontal_line_metrics(px).map(|l| (l.ascent, l.descent)), scale)
        } else {
            let Some(advance) = faces.fallback_advance(id, ch, px) else { return };
            let scale = shrink(advance);
            let Some((mask, line)) = faces.fallback_mask(id, ch, px * scale, px) else { return };
            (mask, Some(line), scale)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The coverage map names, for each character, the face a scan of the
    /// font files one by one finds: the first fallback with a glyph for it
    /// (or none). Checked on this system's fonts.
    #[test]
    fn coverage_agrees_with_a_scan() {
        let Ok(glyphs) = SystemGlyphs::new(16.0, None) else { return };
        let mut faces = glyphs.faces.lock().unwrap();
        for ch in "日本語こんにちは世界中文你好한국어안녕하세요ＴＥＲＳＥ┌─┬αβγ∀∂€😀\u{10FFFD}".chars() {
            let scanned = faces.fallbacks.iter().position(|&id| {
                faces
                    .db
                    .with_face_data(id, |data, index| ttf_parser::Face::parse(data, index).is_ok_and(|f| f.glyph_index(ch).is_some()))
                    .unwrap_or(false)
            });
            assert_eq!(faces.coverage.get(ch), scanned.map(|i| i as u32), "{ch:?}");
        }
    }
}

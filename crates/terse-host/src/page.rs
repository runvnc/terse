//! The page device (SPEC 11) in the host: layout with real fonts
//! ([cosmic-text](https://crates.io/crates/cosmic-text): shaping, the
//! Unicode bidirectional algorithm and line breaking, font fallback), on a
//! worker thread so the frame loop never waits, and drawing: glyphs
//! rasterised with swash at the window's pixel size into an atlas, drawn as
//! quads clipped to the page's rectangle.
//!
//! Layout is in screen units, without hinting, so it does not depend on the
//! window's size: a resize only rasterises glyphs at the new size, and
//! scrolling only moves the lines. The lines' positions are rounded to whole
//! pixels when drawn, so glyphs stay crisp while the page scrolls smoothly
//! at sub-unit steps.

use std::collections::HashMap;
#[cfg(feature = "gpu")]
use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use cosmic_text::fontdb::{self, Database, ID, Query};
use cosmic_text::{
    Align as CtAlign, Attrs, Buffer, CacheKey, CacheKeyFlags, Color, Family, Font, FontSystem, LayoutGlyph, Metrics,
    Shaping, Style, Weight, Wrap,
};
#[cfg(feature = "gpu")]
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};
use terse_core::page::{
    self as core_page, Align, FigureBox, FigurePath, LayoutKey, Layouter, Page, PageWatch, Paragraph,
};
use terse_core::Event;
#[cfg(feature = "gpu")]
use terse_core::{SCREEN_HEIGHT, SCREEN_WIDTH};

#[cfg(feature = "gpu")]
use crate::figures::{self, FigureKey, Rasterizer};
#[cfg(feature = "gpu")]
use crate::phosphor;
#[cfg(feature = "gpu")]
use crate::vectors::PHOSPHOR_FORMAT;

/// The smallest line height the host uses, as a multiple of the size; a
/// font whose own (ascent + descent + gap) is tighter is given this.
const MIN_LINE_HEIGHT: f32 = 1.3;
/// The thickness of a rule in screen units (SPEC 11.3: about one).
pub(crate) const RULE_THICKNESS: f32 = 1.0;
/// Atlas size in texels (RGBA: coverage masks in alpha, colour glyphs as
/// they are).
#[cfg(feature = "gpu")]
const ATLAS: u32 = 2048;
/// Per presented machine frame, the shown scroll moves this share of the
/// way to the program's scroll value; jumps longer than this many rectangle
/// heights are not animated.
const SCROLL_EASE: f32 = 0.45;
const SCROLL_JUMP: f32 = 3.0;
/// Figures are rasterised while they are within this many rectangle
/// heights of the view (above and below), and dropped beyond.
pub(crate) const FIGURE_MARGIN: f32 = 0.5;
/// The most pixels one figure's texture has; a bigger one is rasterised at
/// a lower resolution and stretched.
pub(crate) const FIGURE_PIXELS: f32 = 4.0 * 1024.0 * 1024.0;
/// At most this many figure textures, and this many bytes (but at least
/// one texture), are uploaded in one frame.
#[cfg(feature = "gpu")]
const FIGURE_UPLOADS: usize = 32;
#[cfg(feature = "gpu")]
const FIGURE_UPLOAD_BYTES: usize = 8 << 20;

/// The fonts of the three faces (SPEC 11.3): family names as given (or as
/// loaded from a file), `None` for the system's default of that kind.
#[derive(Clone, Debug, Default)]
pub struct PageFonts {
    pub sans: Option<String>,
    pub serif: Option<String>,
    pub mono: Option<String>,
}

/// One face, resolved: the family to ask for and its natural line height
/// as a multiple of the size.
#[derive(Clone, Debug)]
struct Face {
    family: String,
    line_height: f32,
}

/// A FontSystem with the three faces resolved.
pub struct Typesetter {
    fonts: FontSystem,
    faces: [Face; 3],
}

/// The family a generic name (`sans-serif`, `serif`, `monospace`) stands
/// for on this system: what `fc-match` says, if fontconfig is there and
/// the family is in `db`; else what fontdb's own reading of the fontconfig
/// files gives.
fn generic(db: &Database, family: Family) -> String {
    let name = match family {
        Family::Serif => "serif",
        Family::Monospace => "monospace",
        _ => "sans-serif",
    };
    let matched = std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", name])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .filter(|f| has_family(db, f));
    matched.unwrap_or_else(|| {
        db.query(&Query { families: &[family], ..Query::default() })
            .and_then(|id| db.face(id))
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
            .unwrap_or_else(|| db.family_name(&family).to_string())
    })
}

fn has_family(db: &Database, name: &str) -> bool {
    db.faces().any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
}

impl Typesetter {
    /// Loads the system fonts (and any font files named in `fonts`) and
    /// resolves the faces. Takes a while (system fonts are scanned): call it
    /// off the frame loop.
    pub fn new(fonts: &PageFonts) -> Result<Self, String> {
        let mut db = Database::new();
        // With fontconfig, this also sets the generic families (sans-serif,
        // serif, monospace) to the system's preferred ones.
        db.load_system_fonts();
        let mut family = |given: &Option<String>, kind: Family| -> Result<String, String> {
            let Some(name) = given else { return Ok(generic(&db, kind)) };
            if Path::new(name).is_file() {
                let before: Vec<ID> = db.faces().map(|f| f.id).collect();
                db.load_font_file(name).map_err(|e| format!("{name}: {e}"))?;
                let new = db.faces().find(|f| !before.contains(&f.id));
                return new
                    .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
                    .ok_or(format!("{name}: not a font file this host can read"));
            }
            if !has_family(&db, name) {
                return Err(format!("no font family named {name:?} (fc-list : family lists them)"));
            }
            Ok(name.clone())
        };
        let names = [
            family(&fonts.sans, Family::SansSerif)?,
            family(&fonts.serif, Family::Serif)?,
            family(&fonts.mono, Family::Monospace)?,
        ];
        let locale = std::env::var("LANG")
            .ok()
            .and_then(|l| l.split('.').next().map(|s| s.replace('_', "-")))
            .filter(|l| !l.is_empty() && l != "C" && l != "POSIX")
            .unwrap_or_else(|| "en-US".into());
        let mut fonts = FontSystem::new_with_locale_and_db(locale, db);
        let faces = names.map(|family| {
            let id = fonts.db().query(&Query { families: &[Family::Name(&family)], ..Query::default() });
            let line_height = id
                .and_then(|id| fonts.get_font(id, Weight::NORMAL))
                .map(|f| {
                    let m = f.metrics();
                    (m.ascent - m.descent + m.leading) / m.units_per_em as f32
                })
                .filter(|h| h.is_finite())
                .unwrap_or(MIN_LINE_HEIGHT)
                .max(MIN_LINE_HEIGHT);
            Face { family, line_height }
        });
        Ok(Typesetter { fonts, faces })
    }

    /// The families of sans, serif and monospace, for the user.
    pub fn describe(&self) -> String {
        let [s, r, m] = &self.faces;
        format!(
            "sans {} (line {:.2}), serif {} (line {:.2}), mono {} (line {:.2})",
            s.family, s.line_height, r.family, r.line_height, m.family, m.line_height
        )
    }

    /// Lays `doc` out at `width` screen units (SPEC 11.4).
    pub fn layout(&mut self, key: LayoutKey, doc: &[u8], width: u16) -> HostLayout {
        let width = width as f32;
        let mut out =
            HostLayout {
            key,
            height: 0.0,
            lines: Vec::new(),
            glyphs: Vec::new(),
            figures: Vec::new(),
            fonts: HashMap::new(),
            ms: 0.0,
        };
        let start = std::time::Instant::now();
        let mut y = 0.0f32;
        for p in core_page::parse(doc) {
            y += p.space as f32;
            if let Some(rgb) = p.rule {
                out.lines.push(HostLine { top: y, height: core_page::RULE_HEIGHT as f32, baseline: 0.0, x: 0.0, glyphs: 0..0, rule: Some(rgb) });
                y += core_page::RULE_HEIGHT as f32;
                continue;
            }
            if let Some(f) = &p.figure {
                // SPEC 11.3: a block of its own, aligned in the rectangle and
                // cut at its right edge; drawn from the decoded paths.
                let b = FigureBox::place(f, p.align, width as u32);
                out.figures.push(HostFigure {
                    top: y,
                    x: b.x as f32,
                    width: b.shown as f32,
                    height: f.height as f32,
                    anchor: f.anchor,
                    paths: Arc::new(f.paths()),
                });
                y += f.height as f32;
                continue;
            }
            if !p.has_text() {
                continue;
            }
            y = self.paragraph(&p, width, y, &mut out);
        }
        out.height = y;
        for g in &out.glyphs {
            out.fonts.entry((g.glyph.font_id, g.glyph.font_weight)).or_insert(None);
        }
        for (&(id, weight), font) in out.fonts.iter_mut() {
            *font = self.fonts.get_font(id, weight);
        }
        out.ms = start.elapsed().as_secs_f64() * 1000.0;
        out
    }
}

/// The cosmic-text attributes of a run's style.
fn attrs<'a>(faces: &'a [Face; 3], style: &core_page::Style) -> Attrs<'a> {
    let face = &faces[style.face as usize];
    let [r, g, b] = style.rgb;
    let size = style.size as f32;
    Attrs::new()
        .family(Family::Name(&face.family))
        .weight(if style.bold { Weight::BOLD } else { Weight::NORMAL })
        .style(if style.italic { Style::Italic } else { Style::Normal })
        .color(Color::rgb(r, g, b))
        .metadata(style.anchor as usize)
        .metrics(Metrics::new(size, (size * face.line_height).ceil()))
}

impl Typesetter {
    /// Lays out one paragraph with its top at `y`; returns its bottom.
    fn paragraph(&mut self, p: &Paragraph, width: f32, y: f32, out: &mut HostLayout) -> f32 {
        let indent = (p.indent as f32).min(width);
        let avail = (width - indent).max(1.0);
        let last = p.runs.last().map(|r| r.style).unwrap_or_default();
        let first_size = p.runs[0].style.size as f32;
        let Typesetter { fonts, faces } = self;
        let mut buffer = Buffer::new(fonts, Metrics::new(first_size, first_size * 1.25));
        buffer.set_wrap(if p.nowrap { Wrap::None } else { Wrap::WordOrGlyph });
        buffer.set_size(Some(avail), None);
        let align = match p.align {
            Align::Start => None,
            Align::Centre => Some(CtAlign::Center),
            Align::End => Some(CtAlign::End),
        };
        let spans: Vec<(&str, Attrs)> = p.runs.iter().map(|r| (r.text.as_str(), attrs(faces, &r.style))).collect();
        let default = attrs(faces, &last);
        buffer.set_rich_text(spans, &default, Shaping::Advanced, align);
        buffer.shape_until_scroll(fonts, false);
        // SPEC 11.3: indent from the start edge, which is the right for a
        // right-to-left paragraph.
        let rtl = buffer.is_rtl(0).unwrap_or(false);
        let x = if rtl { 0.0 } else { indent };
        let mut bottom = y;
        for run in buffer.layout_runs() {
            let start = out.glyphs.len();
            for g in run.glyphs {
                out.glyphs.push(HostGlyph { glyph: g.clone() });
            }
            out.lines.push(HostLine {
                top: y + run.line_top,
                height: run.line_height,
                baseline: y + run.line_y,
                x,
                glyphs: start..out.glyphs.len(),
                rule: None,
            });
            bottom = y + run.line_top + run.line_height;
        }
        bottom
    }
}

/// A glyph as cosmic-text laid it out (x relative to its paragraph, y 0 at
/// the baseline; colour and anchor in its attributes).
#[derive(Clone, Debug)]
pub struct HostGlyph {
    pub(crate) glyph: LayoutGlyph,
}

impl HostGlyph {
    fn anchor(&self) -> u16 {
        self.glyph.metadata as u16
    }

    pub(crate) fn rgb(&self) -> [u8; 3] {
        let c = self.glyph.color_opt.unwrap_or(Color::rgb(229, 229, 229));
        [c.r(), c.g(), c.b()]
    }
}

/// A laid-out line, in screen units from the top of the document.
#[derive(Clone, Debug)]
pub struct HostLine {
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
    /// The paragraph's x offset (its indent, for left-to-right text).
    pub x: f32,
    pub(crate) glyphs: std::ops::Range<usize>,
    /// A rule across the rectangle in this colour.
    pub rule: Option<[u8; 3]>,
}

/// A laid-out figure, in screen units: its box (x from the rectangle's
/// left edge, top from the document's top, the width shown) and its paths.
#[derive(Clone, Debug)]
pub struct HostFigure {
    pub top: f32,
    pub x: f32,
    pub width: f32,
    pub height: f32,
    pub anchor: u16,
    pub paths: Arc<Vec<FigurePath>>,
}

/// A document laid out with the host's fonts.
pub struct HostLayout {
    /// The document generation and width it was laid out for.
    pub key: LayoutKey,
    /// The height in screen units.
    pub height: f32,
    pub lines: Vec<HostLine>,
    pub(crate) glyphs: Vec<HostGlyph>,
    /// The figures, top to bottom.
    pub figures: Vec<HostFigure>,
    /// The fonts the glyphs use, for rasterising them on another thread.
    pub(crate) fonts: HashMap<(ID, fontdb::Weight), Option<Arc<Font>>>,
    /// How long the layout took, in ms.
    pub ms: f64,
}

impl HostLayout {
    /// The height SPEC 11.5 reports: whole screen units, rounded up.
    pub fn reported_height(&self) -> u32 {
        self.height.ceil().clamp(0.0, u32::MAX as f32) as u32
    }

    /// The anchor of the text at (x, y), screen units from the rectangle's
    /// left and the document's top; 0 if none.
    pub fn anchor_at(&self, x: f32, y: f32) -> u16 {
        for f in &self.figures {
            if f.anchor != 0 && f.top <= y && y < f.top + f.height && f.x <= x && x < f.x + f.width {
                return f.anchor;
            }
        }
        for line in self.lines.iter().filter(|l| l.top <= y && y < l.top + l.height) {
            for g in &self.glyphs[line.glyphs.clone()] {
                let x0 = line.x + g.glyph.x;
                if g.anchor() != 0 && x0 <= x && x < x0 + g.glyph.w {
                    return g.anchor();
                }
            }
        }
        0
    }
}

/// The host's layout as a [`Layouter`] for a synchronous host (`--screenshot`
/// runs the machine like `terse run` does, but with these fonts). The
/// latest layout is also kept in `latest` for drawing.
pub struct SyncLayouter {
    typesetter: Arc<Mutex<Typesetter>>,
    pub latest: Arc<Mutex<Option<Arc<HostLayout>>>>,
    generation: u64,
}

impl SyncLayouter {
    /// A layouter using fonts shared with others (one per machine).
    pub fn shared(typesetter: Arc<Mutex<Typesetter>>) -> Self {
        SyncLayouter { typesetter, latest: Arc::new(Mutex::new(None)), generation: 0 }
    }
}

impl Layouter for SyncLayouter {
    fn layout(&mut self, doc: &[u8], width: u16) -> u32 {
        self.generation += 1;
        let layout = Arc::new(self.typesetter.lock().unwrap().layout((self.generation, width), doc, width));
        let height = layout.reported_height();
        *self.latest.lock().unwrap() = Some(layout);
        height
    }

    fn anchor_at(&self, x: u32, y: u32) -> u16 {
        self.latest.lock().unwrap().as_ref().map_or(0, |l| l.anchor_at(x as f32 + 0.5, y as f32 + 0.5))
    }

    fn height(&self) -> u32 {
        self.latest.lock().unwrap().as_ref().map_or(0, |l| l.reported_height())
    }
}

/// A layout request for the thread: from which worker, and where the
/// result goes.
struct Job {
    client: u64,
    key: LayoutKey,
    doc: Vec<u8>,
    reply: Sender<Result<HostLayout, String>>,
}

/// The thread that lays out the pages of every machine of a windowed host,
/// with one set of fonts.
#[derive(Clone)]
pub struct PageThread {
    jobs: Sender<Job>,
    clients: Arc<std::sync::atomic::AtomicU64>,
}

impl PageThread {
    pub fn start(fonts: PageFonts) -> Self {
        let (jobs, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("page layout".into())
            .spawn(move || {
                let mut typesetter = Typesetter::new(&fonts);
                match &typesetter {
                    Ok(t) => eprintln!("terse-host: page fonts: {}", t.describe()),
                    Err(e) => eprintln!("terse-host: page fonts: {e}"),
                }
                while let Ok(job) = rx.recv() {
                    // Only each worker's latest request matters (SPEC 11.5).
                    let mut batch = vec![job];
                    loop {
                        match rx.try_recv() {
                            Ok(newer) => match batch.iter_mut().find(|j| j.client == newer.client) {
                                Some(older) => *older = newer,
                                None => batch.push(newer),
                            },
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }
                    for job in batch {
                        let result = match &mut typesetter {
                            Ok(t) => Ok(t.layout(job.key, &job.doc, job.key.1)),
                            Err(e) => Err(e.clone()),
                        };
                        let _ = job.reply.send(result);
                    }
                }
            })
            .expect("start the page layout thread");
        PageThread { jobs, clients: Arc::new(std::sync::atomic::AtomicU64::new(0)) }
    }

    /// A worker for one machine's page.
    pub fn worker(&self) -> PageWorker {
        let (reply, done) = std::sync::mpsc::channel();
        let client = self.clients.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        PageWorker { jobs: self.jobs.clone(), client, reply, done, watch: PageWatch::default(), requested: None, shown: None }
    }
}

/// Lays one machine's page out on the [`PageThread`] and decides, with
/// [`PageWatch`], when the height event is sent.
pub struct PageWorker {
    jobs: Sender<Job>,
    client: u64,
    reply: Sender<Result<HostLayout, String>>,
    done: Receiver<Result<HostLayout, String>>,
    watch: PageWatch,
    /// The key last sent to the worker.
    requested: Option<LayoutKey>,
    /// The layout shown, if any.
    pub shown: Option<Arc<HostLayout>>,
}

impl PageWorker {
    /// Before a frame: takes finished layouts. Returns the height event to
    /// queue for this frame, if a layout of the current page finished; an
    /// error if the fonts could not be loaded.
    pub fn poll(&mut self, page: Option<&Page>) -> Result<Option<Event>, String> {
        let mut event = None;
        loop {
            let layout = match self.done.try_recv() {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => return Err(e),
                Err(_) => break,
            };
            let key = layout.key;
            let current = page.and_then(|p| self.watch.wanted(p));
            if current == Some(key) {
                event = self.watch.finished(key, layout.reported_height()).or(event);
            }
            // Show it unless an older document's layout came late.
            if self.shown.as_ref().is_none_or(|s| s.key.0 <= key.0) && page.is_some_and(|p| p.document().is_some()) {
                self.shown = Some(Arc::new(layout));
            }
        }
        Ok(event)
    }

    /// After a frame: starts a layout if the page changed. A cleared page
    /// is done at once (height 0); its event is returned for the next frame.
    pub fn after_frame(&mut self, page: Option<&Page>) -> Option<Event> {
        let page = page?;
        if page.document().is_none() {
            self.shown = None;
        }
        let key = self.watch.wanted(page)?;
        match page.document() {
            None => self.watch.finished(key, 0),
            Some(doc) => {
                if self.requested != Some(key) {
                    self.requested = Some(key);
                    let job = Job { client: self.client, key, doc: doc.to_vec(), reply: self.reply.clone() };
                    let _ = self.jobs.send(job);
                }
                None
            }
        }
    }
}

// --- Drawing -------------------------------------------------------------

/// The scroll to show next, in screen units: the shown scroll `shown`
/// moves [`SCROLL_EASE`] of the way to the program's `target` per presented
/// machine frame, unless not animated or the jump is longer than
/// [`SCROLL_JUMP`] heights `h` of the rectangle.
pub(crate) fn next_scroll(shown: Option<f32>, target: f32, h: f32, animate: bool) -> f32 {
    match shown {
        Some(s) if animate && (target - s).abs() <= SCROLL_JUMP * h => {
            let next = s + (target - s) * SCROLL_EASE;
            if (target - next).abs() < 0.1 { target } else { next }
        }
        _ => target,
    }
}

/// The page glyph `key` of `font` as swash renders it: a coverage mask, or
/// a colour image (emoji), without hinting.
pub(crate) fn render_glyph(scale_context: &mut ScaleContext, font: &Font, key: CacheKey) -> Option<swash::scale::image::Image> {
    let swash_font = font.as_swash();
    let wght = swash_font.variations().find_by_tag(swash::Tag::from_be_bytes(*b"wght"));
    let mut builder = scale_context.builder(swash_font).size(f32::from_bits(key.font_size_bits)).hint(false);
    if let Some(v) = wght {
        builder = builder.normalized_coords(swash_font.variations().normalized_coords([(
            swash::Tag::from_be_bytes(*b"wght"),
            f32::from(key.font_weight.0).clamp(v.min_value(), v.max_value()),
        )]));
    }
    let mut scaler = builder.build();
    Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline])
        .format(Format::Alpha)
        .offset(Vector::new(key.x_bin.as_float(), key.y_bin.as_float()))
        .transform(
            key.flags
                .contains(CacheKeyFlags::FAKE_ITALIC)
                .then(|| Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0))),
        )
        .render(&mut scaler, key.glyph_id)
}


#[cfg(feature = "gpu")]
const SHADER: &str = r#"
struct Params {
    viewport: vec2<f32>,
    // bit 0: phosphor light (linear, additive) instead of the layer's sRGB values
    flags: u32,
    gain: f32,
}
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var atlas: texture_2d<f32>;

struct In {
    @location(0) rect: vec4<f32>,   // x0, y0, x1, y1 in target pixels
    @location(1) uv: vec4<f32>,     // u0, v0, u1, v1 in atlas texels
    @location(2) colour: vec4<f32>, // sRGB, alpha = opacity
    @location(3) mode: u32,         // 0 solid, 1 coverage mask, 2 colour glyph, 3 premultiplied image
}
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) @interpolate(flat) mode: u32,
}

@vertex
fn vs(@builtin(vertex_index) i: u32, v: In) -> Out {
    let corner = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0))[i];
    let xy = mix(v.rect.xy, v.rect.zw, corner);
    var o: Out;
    o.pos = vec4<f32>(xy.x / p.viewport.x * 2.0 - 1.0, 1.0 - xy.y / p.viewport.y * 2.0, 0.0, 1.0);
    o.uv = mix(v.uv.xy, v.uv.zw, corner);
    o.colour = v.colour;
    o.mode = v.mode;
    return o;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    var rgb = in.colour.rgb;
    var a = in.colour.a;
    if in.mode == 1u {
        a *= textureLoad(atlas, vec2<i32>(floor(in.uv)), 0).a;
    } else if in.mode == 2u {
        let t = textureLoad(atlas, vec2<i32>(floor(in.uv)), 0);
        rgb = t.rgb;
        a *= t.a;
    } else if in.mode == 3u {
        let t = textureLoad(atlas, vec2<i32>(floor(in.uv)), 0);
        rgb = t.rgb / max(t.a, 1.0 / 255.0);
        a *= t.a;
    }
    if (p.flags & 1u) != 0u {
        return vec4<f32>(to_linear(rgb) * a * p.gain, 0.0);
    }
    return vec4<f32>(rgb, a);
}
"#;

#[cfg(feature = "gpu")]
const INSTANCE_BYTES: usize = 16 + 16 + 4 + 4;

#[cfg(feature = "gpu")]
/// Where a glyph image is in the atlas, and how it sits on its pen position.
#[derive(Clone, Copy)]
struct Slot {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left: i32,
    top: i32,
    colour: bool,
}

#[cfg(feature = "gpu")]
/// A shelf packer over the atlas.
struct Shelves {
    x: u32,
    y: u32,
    row: u32,
}

#[cfg(feature = "gpu")]
impl Shelves {
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > ATLAS || h > ATLAS {
            return None;
        }
        if self.x + w > ATLAS {
            self.x = 0;
            self.y += self.row + 1;
            self.row = 0;
        }
        if self.y + h > ATLAS {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w + 1;
        self.row = self.row.max(h);
        Some(at)
    }
}

/// The page device's state as the renderer needs it.
#[derive(Clone, Copy, Debug)]
pub struct PageView {
    pub region: terse_core::Region,
    pub background: [u8; 4],
}

#[cfg(feature = "gpu")]
/// A figure's texture, with bind groups for the clean and the light pipeline.
struct FigureTexture {
    _texture: wgpu::Texture,
    groups: [wgpu::BindGroup; 2],
    bytes: usize,
}

#[cfg(feature = "gpu")]
/// The page's GPU side: the glyph atlas and the quads of what is in view.
pub struct PageLayer {
    clean_pipeline: wgpu::RenderPipeline,
    light_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    light_bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    light_params: wgpu::Buffer,
    atlas: wgpu::Texture,
    instances: wgpu::Buffer,
    capacity: usize,
    count: u32,
    shelves: Shelves,
    slots: HashMap<CacheKey, Option<Slot>>,
    scale_context: ScaleContext,
    fonts: HashMap<(ID, fontdb::Weight), Option<Arc<Font>>>,
    scratch: Vec<u8>,
    /// What the page device holds (without its document) and its layout.
    view: Option<PageView>,
    layout: Option<Arc<HostLayout>>,
    /// The scroll shown, in screen units (eases towards the program's).
    shown_scroll: Option<f32>,
    animate: bool,
    /// The rectangle in target pixels for the scissor, if anything is drawn.
    scissor: Option<(u32, u32, u32, u32)>,
    bind_layout: wgpu::BindGroupLayout,
    /// Figures: the thread that rasterises them (started at the first),
    /// their textures, those asked for, and the quads (instance index and
    /// texture) prepared last, which follow the text's `text_count` quads.
    rasterizer: Option<Rasterizer>,
    figure_textures: HashMap<FigureKey, FigureTexture>,
    figures_pending: HashSet<FigureKey>,
    figures_ready: Vec<figures::Done>,
    figure_log: Vec<String>,
    figure_draws: Vec<(u32, FigureKey)>,
    text_count: u32,
    /// Whether filled shapes are drawn as outlines (the vector display's
    /// fill mode 1); the phosphor style always outlines.
    outlines: bool,
    /// Rasterise figures at once, on this thread (for screenshots).
    sync_figures: bool,
}

#[cfg(feature = "gpu")]
fn blend(c: wgpu::BlendComponent) -> wgpu::BlendState {
    wgpu::BlendState { color: c, alpha: c }
}

#[cfg(feature = "gpu")]
impl PageLayer {
    /// `layer_format` is the format of the layer of backgrounds and shapes.
    pub fn new(device: &wgpu::Device, layer_format: wgpu::TextureFormat) -> Self {
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("page atlas"),
            size: wgpu::Extent3d { width: ATLAS, height: ATLAS, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let uniform = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params = uniform("page params");
        let light_params = uniform("page light params");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("page"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let group = |params: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("page"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                ],
            })
        };
        let bind_group = group(&params);
        let light_bind_group = group(&light_params);
        let shader = crate::renderer::shader_module(device, "page", || SHADER.to_string());
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("page"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Unorm8x4, 3 => Uint32];
        let pipeline = |format, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("page"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: INSTANCE_BYTES as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
        // The layer holds sRGB-encoded colours; text is blended over it in
        // that space, as the terminal's glyphs are.
        let over = blend(C { src_factor: F::SrcAlpha, dst_factor: F::OneMinusSrcAlpha, operation: O::Add });
        let add = blend(C { src_factor: F::One, dst_factor: F::One, operation: O::Add });
        let capacity = 4096;
        PageLayer {
            clean_pipeline: pipeline(layer_format, over),
            light_pipeline: pipeline(PHOSPHOR_FORMAT, add),
            bind_group,
            light_bind_group,
            params,
            light_params,
            atlas,
            instances: Self::instance_buffer(device, capacity),
            capacity,
            count: 0,
            shelves: Shelves { x: 0, y: 0, row: 0 },
            slots: HashMap::new(),
            scale_context: ScaleContext::new(),
            fonts: HashMap::new(),
            scratch: Vec::new(),
            view: None,
            layout: None,
            shown_scroll: None,
            animate: true,
            scissor: None,
            bind_layout: layout,
            rasterizer: None,
            figure_textures: HashMap::new(),
            figures_pending: HashSet::new(),
            figures_ready: Vec::new(),
            figure_log: Vec::new(),
            figure_draws: Vec::new(),
            text_count: 0,
            outlines: false,
            sync_figures: false,
        }
    }

    fn instance_buffer(device: &wgpu::Device, n: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("page instances"),
            size: (n * INSTANCE_BYTES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Whether scrolling is animated and figures are rasterised on their
    /// own thread (not for screenshots, which show the finished page).
    pub fn set_animate(&mut self, animate: bool) {
        self.animate = animate;
        self.sync_figures = !animate;
    }

    /// Whether filled shapes are drawn as outlines (SPEC 10.3, fill mode 1).
    pub fn set_outlines(&mut self, outlines: bool) {
        self.outlines = outlines;
    }

    /// The scroll shown now, in screen units.
    pub fn shown_scroll(&self) -> f32 {
        self.shown_scroll.unwrap_or(0.0)
    }

    /// Once per machine frame: takes the page's state and the layout to
    /// show, and moves the shown scroll towards the program's (clamped)
    /// scroll value.
    pub fn set(&mut self, page: Option<&Page>, layout: Option<Arc<HostLayout>>) {
        self.view = page.map(|p| PageView { region: p.region(), background: p.background() });
        self.layout = layout;
        let (Some(page), Some(layout)) = (page, &self.layout) else {
            self.shown_scroll = None;
            return;
        };
        let target = page.scroll_for(layout.reported_height()) as f32;
        let h = page.region().h as f32;
        self.shown_scroll = Some(next_scroll(self.shown_scroll, target, h, self.animate));
    }

    fn glyph(&mut self, queue: &wgpu::Queue, key: CacheKey) -> Option<Slot> {
        if let Some(slot) = self.slots.get(&key) {
            return *slot;
        }
        let image = self.fonts.get(&(key.font_id, key.font_weight)).cloned().flatten().and_then(|font| render_glyph(&mut self.scale_context, &font, key));
        let slot = image.filter(|i| i.placement.width > 0 && i.placement.height > 0).and_then(|image| {
            let (w, h) = (image.placement.width, image.placement.height);
            let (x, y) = self.shelves.alloc(w, h)?;
            let colour = image.content == Content::Color;
            let rgba: Vec<u8> = match image.content {
                Content::Mask => image.data.iter().flat_map(|&a| [255, 255, 255, a]).collect(),
                Content::SubpixelMask => image.data.chunks_exact(4).flat_map(|p| [255, 255, 255, p[1]]).collect(),
                Content::Color => image.data.clone(),
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: None },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            Some(Slot { x, y, w, h, left: image.placement.left, top: image.placement.top, colour })
        });
        // A glyph that did not fit is not remembered, so it is tried again
        // once the atlas has been emptied.
        if slot.is_some() || self.shelves.y + 1 < ATLAS {
            self.slots.insert(key, slot);
        }
        slot
    }

    /// Builds the quads of what is in view: the background (clean style),
    /// then the visible lines. `screen` is the 640 × 480 screen in target
    /// pixels (x, y, w, h); `viewport` the target's size.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: (f32, f32, f32, f32),
        viewport: (u32, u32),
        phosphor: bool,
    ) {
        self.count = 0;
        self.scissor = None;
        let Some(view) = self.view else { return };
        let layout = self.layout.clone();
        let layout = layout.as_deref();
        let r = view.region;
        if !r.visible() {
            return;
        }
        let s = screen.2 / SCREEN_WIDTH as f32;
        let _ = SCREEN_HEIGHT;
        let rx = screen.0 + r.x as f32 * s;
        let ry = screen.1 + r.y as f32 * s;
        let (rw, rh) = (r.w as f32 * s, r.h as f32 * s);
        let x0 = rx.round().max(0.0) as u32;
        let y0 = ry.round().max(0.0) as u32;
        let x1 = ((rx + rw).round() as u32).min(viewport.0);
        let y1 = ((ry + rh).round() as u32).min(viewport.1);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        self.scissor = Some((x0, y0, x1 - x0, y1 - y0));
        // Fonts for rasterising.
        if let Some(l) = layout {
            for (k, f) in &l.fonts {
                self.fonts.entry(*k).or_insert_with(|| f.clone());
            }
        }
        for attempt in 0..2 {
            self.scratch.clear();
            let mut full = false;
            let bg = view.background;
            if !phosphor && bg[3] > 0 {
                push(&mut self.scratch, [x0 as f32, y0 as f32, x1 as f32, y1 as f32], [0.0; 4], bg, 0);
            }
            if let Some(layout) = layout {
                let scroll = self.shown_scroll.unwrap_or(0.0);
                let top = scroll;
                let bottom = scroll + r.h as f32;
                let first = layout.lines.partition_point(|l| l.top + l.height <= top);
                for line in layout.lines[first..].iter().take_while(|l| l.top < bottom) {
                    if let Some([cr, cg, cb]) = line.rule {
                        let y = (ry + (line.top + core_page::RULE_HEIGHT as f32 / 2.0 - scroll) * s).round();
                        let t = (RULE_THICKNESS * s).round().max(1.0);
                        let y = y - (t / 2.0).floor();
                        push(&mut self.scratch, [x0 as f32, y, x1 as f32, y + t], [0.0; 4], [cr, cg, cb, 255], 0);
                        continue;
                    }
                    // Whole pixels for the baseline, so glyphs stay crisp.
                    let base = (ry + (line.baseline - scroll) * s).round();
                    let lx = rx + line.x * s;
                    for i in line.glyphs.clone() {
                        let g = &layout.glyphs[i];
                        let phys = g.glyph.physical((lx, base), s);
                        let Some(slot) = self.glyph(queue, phys.cache_key) else {
                            if self.shelves.y + 1 >= ATLAS {
                                full = true;
                            }
                            continue;
                        };
                        let gx = (phys.x + slot.left) as f32;
                        let gy = (phys.y - slot.top) as f32;
                        let [cr, cg, cb] = g.rgb();
                        let uv = [slot.x as f32, slot.y as f32, (slot.x + slot.w) as f32, (slot.y + slot.h) as f32];
                        let mode = if slot.colour { 2 } else { 1 };
                        push(&mut self.scratch, [gx, gy, gx + slot.w as f32, gy + slot.h as f32], uv, [cr, cg, cb, 255], mode);
                    }
                }
            }
            if full && attempt == 0 {
                // The atlas is full: start it again with only what is in view.
                self.slots.clear();
                self.shelves = Shelves { x: 0, y: 0, row: 0 };
                continue;
            }
            break;
        }
        self.text_count = (self.scratch.len() / INSTANCE_BYTES) as u32;
        self.figure_draws.clear();
        if let Some(layout) = layout {
            let scroll = self.shown_scroll.unwrap_or(0.0);
            let geometry = FigureGeometry { rx, ry, s, scroll, h: r.h as f32, outline: phosphor || self.outlines };
            self.figures(device, queue, layout, geometry);
        } else {
            self.figure_textures.clear();
            self.figures_pending.clear();
        }
        let n = self.scratch.len() / INSTANCE_BYTES;
        if n > self.capacity {
            self.capacity = n.next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
        }
        if n > 0 {
            queue.write_buffer(&self.instances, 0, &self.scratch);
        }
        self.count = n as u32;
        let mut params = Vec::with_capacity(16);
        for v in [viewport.0 as f32, viewport.1 as f32] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        params.extend_from_slice(&(phosphor as u32).to_le_bytes());
        params.extend_from_slice(&phosphor::GLYPH.to_le_bytes());
        queue.write_buffer(if phosphor { &self.light_params } else { &self.params }, 0, &params);
    }

    /// Draws the quads prepared last into `pass` (the layer, or in the
    /// phosphor style the beam texture).
    pub fn draw(&self, pass: &mut wgpu::RenderPass, phosphor: bool) {
        let Some((x, y, w, h)) = self.scissor.filter(|_| self.count > 0) else { return };
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(if phosphor { &self.light_pipeline } else { &self.clean_pipeline });
        pass.set_bind_group(0, if phosphor { &self.light_bind_group } else { &self.bind_group }, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..(self.count as u64 * INSTANCE_BYTES as u64)));
        pass.draw(0..6, 0..self.text_count);
        for (i, key) in &self.figure_draws {
            if let Some(t) = self.figure_textures.get(key) {
                pass.set_bind_group(0, &t.groups[phosphor as usize], &[]);
                pass.draw(0..6, *i..*i + 1);
            }
        }
    }

    /// The figures near the view: asks for those not yet rasterised (or
    /// rasterises them at once, for screenshots), uploads those that came,
    /// drops the textures of the others, and adds the quads of those in
    /// view to the instances.
    fn figures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, layout: &HostLayout, g: FigureGeometry) {
        let FigureGeometry { rx, ry, s, scroll, h, outline } = g;
        let top = scroll - FIGURE_MARGIN * h;
        let bottom = scroll + (1.0 + FIGURE_MARGIN) * h;
        let max_side = device.limits().max_texture_dimension_2d;
        let mut wanted = HashSet::new();
        let mut jobs = Vec::new();
        let first = layout.figures.partition_point(|f| f.top + f.height <= top);
        for (i, f) in layout.figures.iter().enumerate().skip(first).take_while(|(_, f)| f.top < bottom) {
            // The quad in target pixels, and the texture's size: the same
            // unless the figure is bigger than FIGURE_PIXELS or the GPU allows.
            let (qw, qh) = ((f.width * s).ceil().max(1.0), (f.height * s).ceil().max(1.0));
            let mut scale = s.min((FIGURE_PIXELS / (qw * qh)).sqrt() * s);
            scale = scale.min(max_side as f32 / (f.width.max(f.height)));
            let size = (((f.width * scale).ceil() as u32).clamp(1, max_side), ((f.height * scale).ceil() as u32).clamp(1, max_side));
            let key: FigureKey = (layout.key, i, size, outline);
            wanted.insert(key);
            if !self.figure_textures.contains_key(&key) && !self.figures_pending.contains(&key) {
                self.figures_pending.insert(key);
                jobs.push(figures::Job { key, paths: f.paths.clone(), scale });
            }
            if f.top < scroll + h && f.top + f.height > scroll {
                let x = (rx + f.x * s).round();
                let y = (ry + (f.top - scroll) * s).round();
                let uv = [0.0, 0.0, size.0 as f32, size.1 as f32];
                self.figure_draws.push(((self.scratch.len() / INSTANCE_BYTES) as u32, key));
                push(&mut self.scratch, [x, y, x + qw, y + qh], uv, [255; 4], 3);
            }
        }
        self.figure_textures.retain(|k, _| wanted.contains(k));
        self.figures_pending.retain(|k| wanted.contains(k));
        let mut done = Vec::new();
        if self.sync_figures {
            done.extend(jobs.iter().map(figures::run));
        } else {
            let r = self.rasterizer.get_or_insert_with(Rasterizer::start);
            *r.wanted.lock().unwrap() = wanted.clone();
            for job in jobs {
                r.send(job);
            }
            done.extend(r.done.try_iter());
        }
        // Uploads: all at once for a screenshot, else a bounded amount a
        // frame (the rest wait for the next).
        self.figures_ready.extend(done);
        let pending = &self.figures_pending;
        self.figures_ready.retain(|d| pending.contains(&d.key));
        let (mut bytes, mut n) = (0, 0);
        while let Some(d) = self.figures_ready.first() {
            if !self.sync_figures && n > 0 && (n == FIGURE_UPLOADS || bytes + d.data.len() > FIGURE_UPLOAD_BYTES) {
                break;
            }
            let d = self.figures_ready.remove(0);
            bytes += d.data.len();
            n += 1;
            self.figures_pending.remove(&d.key);
            let texture = self.figure_texture(device, queue, d.key.2, &d.data);
            self.figure_textures.insert(d.key, texture);
            let (w, h) = d.key.2;
            if self.figure_log.len() >= 256 {
                continue; // nobody is reading it
            }
            self.figure_log.push(format!(
                "figure {} ({w} x {h} px{}) rasterised in {:.1} ms; {} figures held, {} KiB",
                d.key.1,
                if d.key.3 { ", outlines" } else { "" },
                d.ms,
                self.figure_textures.len(),
                self.figure_bytes() / 1024
            ));
        }
    }

    /// What the figures did since the last call, for `TERSE_PAGE_TRACE`.
    pub fn take_figure_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.figure_log)
    }

    fn figure_texture(&self, device: &wgpu::Device, queue: &wgpu::Queue, (w, h): (u32, u32), data: &[u8]) -> FigureTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("page figure"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: None },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = |params: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("page figure"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                ],
            })
        };
        let groups = [group(&self.params), group(&self.light_params)];
        FigureTexture { _texture: texture, groups, bytes: data.len() }
    }

    /// Bytes of figure textures held (for measurements).
    pub fn figure_bytes(&self) -> usize {
        self.figure_textures.values().map(|t| t.bytes).sum()
    }



    /// The anchor under (x, y) in screen units as the page is shown now; 0
    /// if none.
    pub fn anchor_at(&self, x: f32, y: f32) -> u16 {
        let (Some(view), Some(layout)) = (self.view, &self.layout) else { return 0 };
        let r = view.region;
        let (x, y) = (x - r.x as f32, y - r.y as f32);
        if !r.visible() || x < 0.0 || y < 0.0 || x >= r.w as f32 || y >= r.h as f32 {
            return 0;
        }
        layout.anchor_at(x, y + self.shown_scroll())
    }

    /// Quads prepared last (for measurements).
    pub fn quads(&self) -> u32 {
        self.count
    }
}

#[cfg(feature = "gpu")]
/// Where the page is drawn, for its figures: the rectangle's corner in
/// target pixels, pixels per unit, the scroll shown and the rectangle's
/// height in units, and whether figures are outlined.
#[derive(Clone, Copy)]
struct FigureGeometry {
    rx: f32,
    ry: f32,
    s: f32,
    scroll: f32,
    h: f32,
    outline: bool,
}

#[cfg(feature = "gpu")]
fn push(out: &mut Vec<u8>, rect: [f32; 4], uv: [f32; 4], rgba: [u8; 4], mode: u32) {
    for v in rect.iter().chain(&uv) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&rgba);
    out.extend_from_slice(&mode.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use terse_core::page::op;

    fn text(s: &str) -> Vec<u8> {
        let mut v = vec![op::TEXT];
        v.extend_from_slice(&(s.len() as u16).to_le_bytes());
        v.extend_from_slice(s.as_bytes());
        v
    }

    /// An article of about `bytes` bytes as the reader builds it: a title,
    /// a rule, then paragraphs of body text.
    fn article(bytes: usize) -> Vec<u8> {
        let mut doc = vec![op::SIZE, 26, op::FONT, 0, 1];
        doc.extend(text("A title for the timing"));
        doc.extend_from_slice(&[op::PARAGRAPH, 0, 0, 8, 0, op::RULE, op::FONT, 0, 0, op::SIZE, 15]);
        let para = "Good typography is invisible. A reader who notices the type has been interrupted; one who \
                    does not is free to follow the argument, which is the point of the whole exercise. ";
        let mut n = 0;
        while doc.len() < bytes {
            doc.extend_from_slice(&[op::PARAGRAPH, 0, 0, 10, 0]);
            doc.extend(text(&para.repeat(2 + n % 3)));
            n += 1;
        }
        doc
    }

    fn typesetter() -> Option<Typesetter> {
        // A system without fonts cannot run these.
        Typesetter::new(&PageFonts::default()).ok().filter(|t| !t.fonts.db().is_empty())
    }

    #[test]
    fn layout_heights_anchors_and_rules() {
        let Some(mut t) = typesetter() else { return };
        let mut doc = vec![op::SIZE, 20];
        doc.extend(text("Hello "));
        doc.extend_from_slice(&[op::ANCHOR, 7, 0]);
        doc.extend(text("link"));
        doc.extend_from_slice(&[op::ANCHOR, 0, 0, op::RULE, op::PARAGRAPH, 0, 0, 30, 0]);
        doc.extend(text("second"));
        let l = t.layout((1, 400), &doc, 400);
        assert_eq!(l.lines.len(), 3, "{:?}", l.lines);
        let first = &l.lines[0];
        assert!(first.height >= 24.0, "line height at least 1.2 × 20");
        assert_eq!(l.lines[1].rule, Some(core_page::DEFAULT_COLOUR));
        assert_eq!(l.lines[1].top, first.height);
        assert_eq!(l.lines[2].top, first.height + 8.0 + 30.0);
        assert_eq!(l.reported_height(), (l.lines[2].top + l.lines[2].height).ceil() as u32);
        // "Hello " is about 60 units wide at size 20: the link starts after it.
        let y = first.top + first.height / 2.0;
        assert_eq!(l.anchor_at(5.0, y), 0);
        let link = l.glyphs[first.glyphs.clone()].iter().find(|g| g.anchor() == 7).unwrap();
        assert_eq!(l.anchor_at(link.glyph.x + 1.0, y), 7);
        assert_eq!(l.anchor_at(link.glyph.x + 1.0, l.lines[2].top + 2.0), 0);
        // Narrower: more lines, and never shorter.
        let narrow = t.layout((1, 40), &doc, 40);
        assert!(narrow.height > l.height);
    }

    #[test]
    fn figures_are_blocks_with_anchors() {
        let Some(mut t) = typesetter() else { return };
        let mut doc = vec![op::SIZE, 20];
        doc.extend(text("Above"));
        doc.extend_from_slice(&[op::PARAGRAPH, 0, 0, 12, 0, op::ANCHOR, 4, 0]);
        // 100 x 50, end-aligned, one Line command.
        doc.extend_from_slice(&[op::FIGURE, 100, 0, 50, 0, 2, 5, 0, 4, 16, 0, 16, 0]);
        doc.extend_from_slice(&[op::ANCHOR, 0, 0]);
        doc.extend(text("Below"));
        let l = t.layout((1, 400), &doc, 400);
        let first = &l.lines[0];
        assert_eq!(l.figures.len(), 1);
        let f = &l.figures[0];
        let top = first.top + first.height + 12.0;
        assert_eq!((f.top, f.x, f.width, f.height), (top, 300.0, 100.0, 50.0));
        assert_eq!(f.paths.len(), 1);
        // Text after it: a paragraph like the latest one (space 12).
        assert_eq!(l.lines[1].top, top + 50.0 + 12.0);
        assert_eq!(l.anchor_at(350.0, top + 25.0), 4);
        assert_eq!(l.anchor_at(250.0, top + 25.0), 0);
        // Narrower than the figure: cut at the edge.
        let n = t.layout((1, 60), &doc, 60);
        assert_eq!((n.figures[0].x, n.figures[0].width), (0.0, 60.0));
    }

    #[test]
    fn right_to_left_paragraphs_indent_from_the_right() {
        let Some(mut t) = typesetter() else { return };
        let mut doc = vec![op::PARAGRAPH, 0, 40, 0, 0];
        doc.extend(text("مرحبا بالعالم"));
        let l = t.layout((1, 300), &doc, 300);
        let line = &l.lines[0];
        assert_eq!(line.x, 0.0);
        let right = l.glyphs[line.glyphs.clone()].iter().map(|g| g.glyph.x + g.glyph.w).fold(0.0, f32::max);
        assert!((right - 260.0).abs() < 1.0, "flush with the start (right) edge less the indent: {right}");
    }

    /// `cargo test --release -p terse-host layout_time -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn layout_time() {
        let Some(mut t) = typesetter() else { return };
        for bytes in [6_000, 48 * 1024, 65_000] {
            let doc = article(bytes);
            let _ = t.layout((0, 500), &doc, 500); // warm the font caches
            let runs = 5;
            let start = std::time::Instant::now();
            let mut l = None;
            for i in 0..runs {
                l = Some(t.layout((i, 500), &doc, 500));
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / runs as f64;
            let l = l.unwrap();
            println!("{} bytes: {:.1} ms, {} lines, height {}", doc.len(), ms, l.lines.len(), l.reported_height());
        }
    }
}

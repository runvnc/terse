//! The page device (SPEC 11) on the CPU: the same quads the GPU renderer
//! draws (`page.rs`): the background, the glyphs of the lines in view at
//! whole-pixel positions, rules, and the figures near the view rasterised
//! with tiny-skia (on their thread, or at once for screenshots), all
//! clipped to the page's rectangle. Glyph images are rendered with swash
//! once and kept.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cosmic_text::{CacheKey, Font, fontdb};
use swash::scale::ScaleContext;
use swash::scale::image::Content;
use terse_core::page::{self as core_page, Page};
use terse_core::SCREEN_WIDTH;

use super::base::{IRect, mix, over, pack};
use crate::figures::{self, FigureKey, Rasterizer};
use crate::page::{FIGURE_MARGIN, FIGURE_PIXELS, HostLayout, PageView, RULE_THICKNESS, next_scroll, render_glyph};
use crate::phosphor;

/// Bytes of glyph images kept before the cache starts over.
const GLYPH_CACHE_BYTES: usize = 32 << 20;
/// The largest side of a figure's image.
const FIGURE_SIDE: u32 = 8192;

/// A glyph as rendered: placement and pixels (coverage, or RGBA for a
/// colour glyph).
pub struct GlyphImage {
    w: u32,
    h: u32,
    left: i32,
    top: i32,
    colour: bool,
    data: Vec<u8>,
}

/// A figure as rasterised: premultiplied RGBA.
pub struct FigureImage {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

#[derive(Clone)]
enum Paint {
    Solid([u8; 4]),
    Mask(Arc<GlyphImage>, [u8; 3]),
    Colour(Arc<GlyphImage>),
    Figure(Arc<FigureImage>),
}

#[derive(Clone)]
struct Quad {
    r: IRect,
    paint: Paint,
}

/// What the page shows, to tell whether it changed since it was drawn.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PageKey {
    view: Option<(terse_core::Region, [u8; 4])>,
    layout: usize,
    scroll: Option<u32>,
    figures: usize,
    outlines: bool,
    screen: (u32, u32, u32, u32),
    phosphor: bool,
}

pub struct SoftPage {
    view: Option<PageView>,
    layout: Option<Arc<HostLayout>>,
    shown_scroll: Option<f32>,
    animate: bool,
    sync_figures: bool,
    outlines: bool,
    glyphs: HashMap<CacheKey, Option<Arc<GlyphImage>>>,
    glyph_bytes: usize,
    scale_context: ScaleContext,
    fonts: HashMap<(fontdb::ID, fontdb::Weight), Option<Arc<Font>>>,
    rasterizer: Option<Rasterizer>,
    figure_images: HashMap<FigureKey, Arc<FigureImage>>,
    figures_pending: HashSet<FigureKey>,
    figure_log: Vec<String>,
    /// The quads prepared last.
    pub quads: PageQuads,
    /// Figures that arrived (a count, to tell that the page changed).
    arrived: usize,
    /// Quads of figures not yet rasterised when they were prepared.
    pending_quads: Vec<(usize, FigureKey)>,
}

impl Default for SoftPage {
    fn default() -> Self {
        SoftPage {
            view: None,
            layout: None,
            shown_scroll: None,
            animate: true,
            sync_figures: false,
            outlines: false,
            glyphs: HashMap::new(),
            glyph_bytes: 0,
            scale_context: ScaleContext::new(),
            fonts: HashMap::new(),
            rasterizer: None,
            figure_images: HashMap::new(),
            figures_pending: HashSet::new(),
            figure_log: Vec::new(),
            quads: PageQuads::default(),
            arrived: 0,
            pending_quads: Vec::new(),
        }
    }
}

impl SoftPage {
    /// Whether scrolling is animated and figures are rasterised on their
    /// own thread (not for screenshots, which show the finished page).
    pub fn set_animate(&mut self, animate: bool) {
        self.animate = animate;
        self.sync_figures = !animate;
    }

    pub fn set_outlines(&mut self, outlines: bool) {
        self.outlines = outlines;
    }

    /// Once per machine frame: the page's state and the layout to show;
    /// the shown scroll moves towards the program's.
    pub fn set(&mut self, page: Option<&Page>, layout: Option<Arc<HostLayout>>) {
        self.view = page.map(|p| PageView { region: p.region(), background: p.background() });
        self.layout = layout;
        let (Some(page), Some(layout)) = (page, &self.layout) else {
            self.shown_scroll = None;
            return;
        };
        let target = page.scroll_for(layout.reported_height()) as f32;
        self.shown_scroll = Some(next_scroll(self.shown_scroll, target, page.region().h as f32, self.animate));
    }

    pub fn anchor_at(&self, x: f32, y: f32) -> u16 {
        let (Some(view), Some(layout)) = (self.view, &self.layout) else { return 0 };
        let r = view.region;
        let (x, y) = (x - r.x as f32, y - r.y as f32);
        if !r.visible() || x < 0.0 || y < 0.0 || x >= r.w as f32 || y >= r.h as f32 {
            return 0;
        }
        layout.anchor_at(x, y + self.shown_scroll.unwrap_or(0.0))
    }

    pub fn take_figure_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.figure_log)
    }

    /// What the page would show drawn into `screen` (the 640 × 480 screen
    /// in pixels); `phosphor`: as light.
    pub fn key(&mut self, screen: (f32, f32, f32, f32), phosphor: bool) -> PageKey {
        self.collect_figures();
        PageKey {
            view: self.view.map(|v| (v.region, v.background)),
            layout: self.layout.as_ref().map_or(0, |l| Arc::as_ptr(l) as usize),
            scroll: self.shown_scroll.map(f32::to_bits),
            figures: self.arrived,
            outlines: self.outlines,
            screen: (screen.0.to_bits(), screen.1.to_bits(), screen.2.to_bits(), screen.3.to_bits()),
            phosphor,
        }
    }

    /// Figures finished on their thread since the last look.
    fn collect_figures(&mut self) {
        let Some(r) = &self.rasterizer else { return };
        let done: Vec<figures::Done> = r.done.try_iter().collect();
        for d in done {
            if self.figures_pending.remove(&d.key) {
                self.store_figure(d);
            }
        }
    }

    fn store_figure(&mut self, d: figures::Done) {
        let (w, h) = d.key.2;
        let bytes = d.data.len();
        self.figure_images.insert(d.key, Arc::new(FigureImage { w, h, data: d.data }));
        self.arrived += 1;
        if self.figure_log.len() < 256 {
            let held: usize = self.figure_images.values().map(|f| f.data.len()).sum();
            self.figure_log.push(format!(
                "figure {} ({w} x {h} px{}) rasterised in {:.1} ms; {} figures held, {} KiB ({bytes} bytes)",
                d.key.1,
                if d.key.3 { ", outlines" } else { "" },
                d.ms,
                self.figure_images.len(),
                held / 1024
            ));
        }
    }

    fn glyph(&mut self, key: CacheKey) -> Option<Arc<GlyphImage>> {
        if let Some(g) = self.glyphs.get(&key) {
            return g.clone();
        }
        if self.glyph_bytes > GLYPH_CACHE_BYTES {
            self.glyphs.clear();
            self.glyph_bytes = 0;
        }
        let image = self.fonts.get(&(key.font_id, key.font_weight)).cloned().flatten().and_then(|font| render_glyph(&mut self.scale_context, &font, key));
        let glyph = image.filter(|i| i.placement.width > 0 && i.placement.height > 0).map(|image| {
            let (w, h) = (image.placement.width, image.placement.height);
            let (colour, data) = match image.content {
                Content::Mask => (false, image.data),
                Content::SubpixelMask => (false, image.data.chunks_exact(4).map(|p| p[1]).collect()),
                Content::Color => (true, image.data),
            };
            Arc::new(GlyphImage { w, h, left: image.placement.left, top: image.placement.top, colour, data })
        });
        self.glyph_bytes += glyph.as_ref().map_or(16, |g| g.data.len() + 64);
        self.glyphs.insert(key, glyph.clone());
        glyph
    }

    /// Builds the quads of what is in view, as `PageLayer::prepare` does:
    /// the background (clean style), the visible lines, the figures.
    /// `screen` is the 640 × 480 screen in target pixels; `viewport` the
    /// target's size.
    pub fn prepare(&mut self, screen: (f32, f32, f32, f32), viewport: (u32, u32), phosphor: bool) {
        self.quads.quads.clear();
        self.quads.scissor = IRect::EMPTY;
        let Some(view) = self.view else { return };
        let layout = self.layout.clone();
        let layout = layout.as_deref();
        let r = view.region;
        if !r.visible() {
            return;
        }
        let s = screen.2 / SCREEN_WIDTH as f32;
        let rx = screen.0 + r.x as f32 * s;
        let ry = screen.1 + r.y as f32 * s;
        let (rw, rh) = (r.w as f32 * s, r.h as f32 * s);
        let x0 = rx.round().max(0.0) as i32;
        let y0 = ry.round().max(0.0) as i32;
        let x1 = ((rx + rw).round() as i32).min(viewport.0 as i32);
        let y1 = ((ry + rh).round() as i32).min(viewport.1 as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        self.quads.scissor = IRect::new(x0, y0, x1, y1);
        if let Some(l) = layout {
            for (k, f) in &l.fonts {
                self.fonts.entry(*k).or_insert_with(|| f.clone());
            }
        }
        let bg = view.background;
        if !phosphor && bg[3] > 0 {
            self.quads.quads.push(Quad { r: self.quads.scissor, paint: Paint::Solid(bg) });
        }
        let Some(layout) = layout else {
            self.figure_images.clear();
            self.figures_pending.clear();
            return;
        };
        let scroll = self.shown_scroll.unwrap_or(0.0);
        let (top, bottom) = (scroll, scroll + r.h as f32);
        let first = layout.lines.partition_point(|l| l.top + l.height <= top);
        for line in layout.lines[first..].iter().take_while(|l| l.top < bottom) {
            if let Some([cr, cg, cb]) = line.rule {
                let y = (ry + (line.top + core_page::RULE_HEIGHT as f32 / 2.0 - scroll) * s).round();
                let t = (RULE_THICKNESS * s).round().max(1.0);
                let y = (y - (t / 2.0).floor()) as i32;
                self.quads.quads.push(Quad { r: IRect::new(x0, y, x1, y + t as i32), paint: Paint::Solid([cr, cg, cb, 255]) });
                continue;
            }
            let base = (ry + (line.baseline - scroll) * s).round();
            let lx = rx + line.x * s;
            for i in line.glyphs.clone() {
                let g = &layout.glyphs[i];
                let phys = g.glyph.physical((lx, base), s);
                let Some(image) = self.glyph(phys.cache_key) else { continue };
                let gx = phys.x + image.left;
                let gy = phys.y - image.top;
                let r = IRect::new(gx, gy, gx + image.w as i32, gy + image.h as i32);
                let paint = if image.colour { Paint::Colour(image) } else { Paint::Mask(image, g.rgb()) };
                self.quads.quads.push(Quad { r, paint });
            }
        }
        self.figures(layout, (rx, ry, s, scroll, r.h as f32), phosphor || self.outlines);
    }

    /// The figures near the view: asks for those not yet rasterised (or
    /// rasterises them at once, for screenshots), drops the others, and
    /// adds quads for those in view.
    fn figures(&mut self, layout: &HostLayout, (rx, ry, s, scroll, h): (f32, f32, f32, f32, f32), outline: bool) {
        let top = scroll - FIGURE_MARGIN * h;
        let bottom = scroll + (1.0 + FIGURE_MARGIN) * h;
        let mut wanted = HashSet::new();
        let mut jobs = Vec::new();
        let first = layout.figures.partition_point(|f| f.top + f.height <= top);
        for (i, f) in layout.figures.iter().enumerate().skip(first).take_while(|(_, f)| f.top < bottom) {
            let (qw, qh) = ((f.width * s).ceil().max(1.0), (f.height * s).ceil().max(1.0));
            let mut scale = s.min((FIGURE_PIXELS / (qw * qh)).sqrt() * s);
            scale = scale.min(FIGURE_SIDE as f32 / f.width.max(f.height));
            let size = (((f.width * scale).ceil() as u32).clamp(1, FIGURE_SIDE), ((f.height * scale).ceil() as u32).clamp(1, FIGURE_SIDE));
            let key: FigureKey = (layout.key, i, size, outline);
            wanted.insert(key);
            if !self.figure_images.contains_key(&key) && !self.figures_pending.contains(&key) {
                self.figures_pending.insert(key);
                jobs.push(figures::Job { key, paths: f.paths.clone(), scale });
            }
            if f.top < scroll + h && f.top + f.height > scroll {
                let x = (rx + f.x * s).round() as i32;
                let y = (ry + (f.top - scroll) * s).round() as i32;
                // Drawn once it has arrived (the key is looked up at draw time).
                self.quads.quads.push(Quad { r: IRect::new(x, y, x + qw as i32, y + qh as i32), paint: Paint::Solid([0; 4]) });
                let at = self.quads.quads.len() - 1;
                if let Some(img) = self.figure_images.get(&key) {
                    self.quads.quads[at].paint = Paint::Figure(img.clone());
                } else {
                    self.pending_quads.push((at, key));
                }
            }
        }
        self.figure_images.retain(|k, _| wanted.contains(k));
        self.figures_pending.retain(|k| wanted.contains(k));
        if self.sync_figures {
            for job in &jobs {
                let d = figures::run(job);
                self.figures_pending.remove(&d.key);
                self.store_figure(d);
            }
        } else if !jobs.is_empty() || self.rasterizer.is_some() {
            let r = self.rasterizer.get_or_insert_with(Rasterizer::start);
            *r.wanted.lock().unwrap() = wanted;
            for job in jobs {
                r.send(job);
            }
        }
        // Quads of figures that arrived just now.
        for (at, key) in std::mem::take(&mut self.pending_quads) {
            if let Some(img) = self.figure_images.get(&key) {
                self.quads.quads[at].paint = Paint::Figure(img.clone());
            }
        }
    }
}

/// The page's quads as prepared, and the rectangle they are clipped to.
#[derive(Default)]
pub struct PageQuads {
    quads: Vec<Quad>,
    pub scissor: IRect,
}

impl PageQuads {

    /// Whether anything is drawn.
    pub fn is_empty(&self) -> bool {
        self.quads.is_empty() || self.scissor.is_empty()
    }

    /// Draws the quads that meet `run` (a rectangle of the target) into
    /// `rows`: an image whose rows start at `y0` and whose columns start at
    /// `x0`, `stride` pixels a row; source-over in sRGB values, as the GPU
    /// renderer's 8-bit layer does.
    pub fn draw_clean(&self, rows: &mut [u32], x0: i32, y0: i32, stride: usize, run: IRect) {
        let clip = run.intersect(&self.scissor);
        if clip.is_empty() {
            return;
        }
        for q in &self.quads {
            let r = q.r.intersect(&clip);
            if r.is_empty() {
                continue;
            }
            for y in r.y0..r.y1 {
                let row = &mut rows[(y - y0) as usize * stride..][..stride];
                let out = &mut row[(r.x0 - x0) as usize..(r.x1 - x0) as usize];
                match &q.paint {
                    Paint::Solid([cr, cg, cb, a]) => {
                        if *a == 0 {
                            continue;
                        }
                        let c = pack(*cr, *cg, *cb);
                        for p in out.iter_mut() {
                            *p = mix(*p, c, *a as u32);
                        }
                    }
                    Paint::Mask(g, [cr, cg, cb]) => {
                        let c = pack(*cr, *cg, *cb);
                        let src = &g.data[(y - q.r.y0) as usize * g.w as usize..][..g.w as usize];
                        let src = &src[(r.x0 - q.r.x0) as usize..(r.x1 - q.r.x0) as usize];
                        for (p, &a) in out.iter_mut().zip(src) {
                            if a > 0 {
                                *p = mix(*p, c, a as u32);
                            }
                        }
                    }
                    Paint::Colour(g) => {
                        let src = &g.data[(y - q.r.y0) as usize * g.w as usize * 4..][..g.w as usize * 4];
                        let src = &src[(r.x0 - q.r.x0) as usize * 4..(r.x1 - q.r.x0) as usize * 4];
                        for (p, t) in out.iter_mut().zip(src.chunks_exact(4)) {
                            if t[3] > 0 {
                                *p = over(*p, [t[0] as f32, t[1] as f32, t[2] as f32], t[3] as f32 / 255.0);
                            }
                        }
                    }
                    Paint::Figure(f) => {
                        let (qw, qh) = (q.r.width() as f32, q.r.height() as f32);
                        let v = (((y - q.r.y0) as f32 + 0.5) / qh * f.h as f32) as usize;
                        let src = &f.data[v.min(f.h as usize - 1) * f.w as usize * 4..][..f.w as usize * 4];
                        for (i, p) in out.iter_mut().enumerate() {
                            let x = r.x0 + i as i32;
                            let u = (((x - q.r.x0) as f32 + 0.5) / qw * f.w as f32) as usize;
                            let t = &src[u.min(f.w as usize - 1) * 4..][..4];
                            if t[3] > 0 {
                                // Premultiplied: back to straight colour, then over.
                                let a = t[3] as f32 / 255.0;
                                let k = 1.0 / a.max(1.0 / 255.0);
                                *p = over(*p, [t[0] as f32 * k, t[1] as f32 * k, t[2] as f32 * k], a);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Adds the quads' light (phosphor style: linear colour × opacity ×
    /// `phosphor::GLYPH`) that meets `run` into `rows`, linear RGB floats
    /// (3 a texel), rows starting at `y0`, `width` texels a row; each row's
    /// `lit` mask takes the blocks (of `1 << shift` columns) it touched.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_light(&self, rows: &mut [f32], y0: i32, width: usize, run: IRect, lit: &mut [u128], shift: u32, lin: &[f32; 256]) {
        let clip = run.intersect(&self.scissor);
        if clip.is_empty() {
            return;
        }
        let gain = phosphor::GLYPH;
        for q in &self.quads {
            let r = q.r.intersect(&clip);
            if r.is_empty() {
                continue;
            }
            for y in r.y0..r.y1 {
                let (b0, b1) = (r.x0 as u32 >> shift, (r.x1 - 1) as u32 >> shift);
                lit[(y - y0) as usize] |= (u128::MAX >> (127 - b1)) & (u128::MAX << b0);
                let row = &mut rows[(y - y0) as usize * width * 3..][..width * 3];
                let out = &mut row[r.x0 as usize * 3..r.x1 as usize * 3];
                match &q.paint {
                    Paint::Solid([cr, cg, cb, a]) => {
                        let k = *a as f32 / 255.0 * gain;
                        let c = [lin[*cr as usize] * k, lin[*cg as usize] * k, lin[*cb as usize] * k];
                        for p in out.chunks_exact_mut(3) {
                            p[0] += c[0];
                            p[1] += c[1];
                            p[2] += c[2];
                        }
                    }
                    Paint::Mask(g, [cr, cg, cb]) => {
                        let c = [lin[*cr as usize] * gain / 255.0, lin[*cg as usize] * gain / 255.0, lin[*cb as usize] * gain / 255.0];
                        let src = &g.data[(y - q.r.y0) as usize * g.w as usize..][..g.w as usize];
                        let src = &src[(r.x0 - q.r.x0) as usize..(r.x1 - q.r.x0) as usize];
                        for (p, &a) in out.chunks_exact_mut(3).zip(src) {
                            let a = a as f32;
                            p[0] += c[0] * a;
                            p[1] += c[1] * a;
                            p[2] += c[2] * a;
                        }
                    }
                    Paint::Colour(g) => {
                        let src = &g.data[(y - q.r.y0) as usize * g.w as usize * 4..][..g.w as usize * 4];
                        let src = &src[(r.x0 - q.r.x0) as usize * 4..(r.x1 - q.r.x0) as usize * 4];
                        for (p, t) in out.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
                            let a = t[3] as f32 / 255.0 * gain;
                            p[0] += lin[t[0] as usize] * a;
                            p[1] += lin[t[1] as usize] * a;
                            p[2] += lin[t[2] as usize] * a;
                        }
                    }
                    Paint::Figure(f) => {
                        let (qw, qh) = (q.r.width() as f32, q.r.height() as f32);
                        let v = (((y - q.r.y0) as f32 + 0.5) / qh * f.h as f32) as usize;
                        let src = &f.data[v.min(f.h as usize - 1) * f.w as usize * 4..][..f.w as usize * 4];
                        for (i, p) in out.chunks_exact_mut(3).enumerate() {
                            let x = r.x0 + i as i32;
                            let u = (((x - q.r.x0) as f32 + 0.5) / qw * f.w as f32) as usize;
                            let t = &src[u.min(f.w as usize - 1) * 4..][..4];
                            if t[3] > 0 {
                                let a = t[3] as f32 / 255.0;
                                let k = 255.0 / t[3] as f32;
                                let c = |v: u8| super::phosphor::to_linear((v as f32 * k / 255.0).min(1.0));
                                p[0] += c(t[0]) * a * gain;
                                p[1] += c(t[1]) * a * gain;
                                p[2] += c(t[2]) * a * gain;
                            }
                        }
                    }
                }
            }
        }
    }
}

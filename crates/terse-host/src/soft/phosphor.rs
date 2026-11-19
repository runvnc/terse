//! The phosphor presentation (SPEC 10.6) on the CPU: the model of
//! `phosphor.rs` with its constants, computed as the GPU renderer's passes
//! compute it, in linear-light floats the size of the screen:
//!
//! 1. the glyph light: each glyph and the cursor in its foreground colour,
//!    times `GLYPH`, plus the average of eight copies shifted by
//!    `GLYPH_BLOOM_RADIUS` times `GLYPH_BLOOM` (the GPU samples the glyphs at
//!    those offsets; here the light image is shifted by whole pixels);
//! 2. the page's text and figures as light, and the shapes' beam (core,
//!    halo, vertex dwell), all added up;
//! 3. the afterglow: both components faded (× their factor, − `EPSILON`),
//!    then the beam deposited with `max`, then the smear of moving shapes;
//! 4. the bloom: the light (whitened above one stroke) blurred by a
//!    separable Gaussian, horizontally then vertically;
//! 5. light + `BLOOM` × bloom, spilled into white where far brighter than
//!    one stroke, through the tone curve to sRGB.
//!
//! The differences from the GPU are of rounding: the GPU keeps the light
//! in 16-bit floats, samples the glyph halo at fractional offsets, and
//! converts to sRGB exactly where this uses a 4,096-entry table.
//!
//! Work is limited to where there is light: each row keeps, per buffer, a
//! bit mask of the blocks of columns where it may be non-zero (and keeps
//! it zero elsewhere), so a dark screen with a few shapes costs those
//! shapes and their glow, not the whole screen.

use std::f32::consts::FRAC_PI_4;

use super::base::{IRect, Pool, pack};
use super::page::PageQuads;
use super::screen::Grid;
use super::shapes::{Geom, Mode, Shape, visit};
use crate::phosphor::{
    BLOOM, BLOOM_MAX_TAPS, BLOOM_SIGMA, BLOOM_WHITE, BLOOM_WHITE_FROM, DEPOSIT, EPSILON, FAST_DECAY, FAST_SHARE, GLYPH,
    GLYPH_BLOOM, GLYPH_BLOOM_RADIUS, SPILL, SPILL_FROM, TONE_KNEE,
};

/// Rows handed to a thread at a time.
const BAND: usize = 16;

/// A row's blocks of columns that may hold light: bit i covers columns
/// `i << shift` up to the next block.
pub type Mask = u128;

/// The blocks of a row: 128 at most, each `1 << shift` columns (16 up to
/// 2,048 columns).
#[derive(Clone, Copy)]
pub struct Blocks {
    pub shift: u32,
    pub width: usize,
}

impl Blocks {
    fn new(width: usize) -> Self {
        let mut shift = 4;
        while (128usize << shift) < width {
            shift += 1;
        }
        Blocks { shift, width }
    }

    fn all(&self) -> Mask {
        let n = self.width.div_ceil(1 << self.shift);
        if n >= 128 { Mask::MAX } else { (1 << n) - 1 }
    }

    /// The blocks holding columns `x0..x1`.
    #[inline]
    pub fn of(&self, x0: usize, x1: usize) -> Mask {
        if x0 >= x1 {
            return 0;
        }
        let (b0, b1) = (x0 >> self.shift, (x1 - 1) >> self.shift);
        (Mask::MAX >> (127 - b1)) & (Mask::MAX << b0)
    }

    /// The column runs (x0, x1) of the set blocks.
    fn runs(&self, mut m: Mask) -> impl Iterator<Item = (usize, usize)> + '_ {
        std::iter::from_fn(move || {
            if m == 0 {
                return None;
            }
            let b0 = m.trailing_zeros();
            let len = (m >> b0).trailing_ones();
            m &= if b0 + len >= 128 { 0 } else { Mask::MAX << (b0 + len) };
            Some(((b0 as usize) << self.shift, (((b0 + len) as usize) << self.shift).min(self.width)))
        })
    }

    /// `m` with each block's `k` neighbours on both sides.
    fn dilate(&self, m: Mask, k: u32) -> Mask {
        let mut r = m;
        for i in 1..=k.min(127) {
            r |= m << i | m >> i;
        }
        r & self.all()
    }
}

pub fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Linear light of each 8-bit sRGB value.
pub fn linear_table() -> [f32; 256] {
    std::array::from_fn(|i| to_linear(i as f32 / 255.0))
}

/// Entries of the table from tone-mapped light (0–1) to 8-bit sRGB.
const SRGB_STEPS: usize = 4096;

fn srgb_table() -> Vec<u8> {
    (0..=SRGB_STEPS).map(|i| (to_srgb(i as f32 / SRGB_STEPS as f32) * 255.0 + 0.5) as u8).collect()
}

/// The tone curve of `phosphor.rs`, without branches (it vectorises).
#[inline]
fn tone(x: f32) -> f32 {
    let u = (x - TONE_KNEE).max(0.0) / (1.0 - TONE_KNEE);
    let soft = TONE_KNEE + (1.0 - TONE_KNEE) * u / (1.0 + u);
    if x <= TONE_KNEE { x } else { soft }
}

/// One afterglow deposit of a moving shape (`phosphor::deposits`): the
/// shape and what it adds to the fast and slow components per unit light.
pub struct Smear {
    pub shape: Shape,
    pub fast: f32,
    pub slow: f32,
}

/// What a machine frame of light is made of.
pub struct Light<'a> {
    pub grid: Grid<'a>,
    /// The page, prepared for the tube (`SoftPage::prepare` with the screen
    /// at the origin and the tube's size).
    pub page: &'a PageQuads,
    pub shapes: &'a [Shape],
    pub smears: &'a [Smear],
    /// Width of the screen in pixels (the halo and the bloom scale with it).
    pub width: f32,
    pub lin: &'a [f32; 256],
    /// Persistence 0–255 (0: no trails).
    pub persistence: u8,
    /// Start the afterglow afresh.
    pub clear: bool,
    /// Advance the afterglow (else only the beam is redrawn).
    pub advance: bool,
}

/// One screen's tube: the light textures of the GPU renderer as floats,
/// linear RGB, three floats a texel.
pub struct Tube {
    pub size: (usize, usize),
    blocks: Blocks,
    beam: Vec<f32>,
    fast: Vec<f32>,
    slow: Vec<f32>,
    glyph: Vec<f32>,
    hblur: Vec<f32>,
    bloom: Vec<f32>,
    /// Per row: the blocks where each buffer may be non-zero (it is zero
    /// elsewhere; the glyph light is only read inside its mask), and those
    /// the last composite left non-black.
    beam_m: Vec<Mask>,
    glow_m: Vec<Mask>,
    h_m: Vec<Mask>,
    bloom_m: Vec<Mask>,
    glyph_m: Vec<Mask>,
    shown_m: Vec<Mask>,
    srgb: Vec<u8>,
}

/// Per-row slices of the tube's buffers for one band of rows.
struct Band<'a> {
    y0: usize,
    beam: &'a mut [f32],
    fast: &'a mut [f32],
    slow: &'a mut [f32],
    hblur: &'a mut [f32],
    beam_m: &'a mut [Mask],
    glow_m: &'a mut [Mask],
    h_m: &'a mut [Mask],
}

impl Tube {
    pub fn new(size: (usize, usize)) -> Self {
        let n = size.0 * size.1 * 3;
        let rows = size.1;
        let blocks = Blocks::new(size.0);
        Tube {
            size,
            blocks,
            beam: vec![0.0; n],
            fast: vec![0.0; n],
            slow: vec![0.0; n],
            glyph: vec![0.0; n],
            hblur: vec![0.0; n],
            bloom: vec![0.0; n],
            beam_m: vec![0; rows],
            glow_m: vec![0; rows],
            h_m: vec![0; rows],
            bloom_m: vec![0; rows],
            glyph_m: vec![0; rows],
            shown_m: vec![blocks.all(); rows],
            srgb: srgb_table(),
        }
    }

    /// One frame of light (or with `advance` false, the beam redrawn).
    pub fn frame(&mut self, l: &Light, pool: &Pool) {
        let (tw, th) = self.size;
        let stride = tw * 3;
        let blocks = self.blocks;
        self.glyph_light(l, pool);

        // The halo's offsets: eight directions at the radius, whole pixels.
        let r = GLYPH_BLOOM_RADIUS * l.width / 640.0;
        let offsets: Vec<(i32, i32)> = (0..8)
            .map(|i| {
                let a = i as f32 * FRAC_PI_4;
                ((0.5 + r * a.cos()).floor() as i32, (0.5 + r * a.sin()).floor() as i32)
            })
            .collect();
        let trails = l.persistence > 0;
        let slow_decay = crate::phosphor::slow_decay(l.persistence);
        let geom = Geom::for_rect((0.0, 0.0, tw as f32, th as f32));
        let sigma = (BLOOM_SIGMA * l.width / 640.0).max(0.5);
        let taps = (3.0 * sigma).ceil().min(BLOOM_MAX_TAPS) as usize;
        let mut weights: Vec<f32> = (-(taps as i32)..=taps as i32).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
        let total: f32 = weights.iter().sum();
        weights.iter_mut().for_each(|w| *w /= total);
        // Blocks within the blur's reach of a block.
        let reach = taps.div_ceil(1 << blocks.shift) as u32;
        let shift = blocks.shift;

        // Pass 1: beam, afterglow and the horizontal blur, band by band.
        {
            let glyph = &self.glyph;
            let glyph_m = &self.glyph_m;
            let bands: Vec<Band> = self
                .beam
                .chunks_mut(BAND * stride)
                .zip(self.fast.chunks_mut(BAND * stride))
                .zip(self.slow.chunks_mut(BAND * stride))
                .zip(self.hblur.chunks_mut(BAND * stride))
                .zip(self.beam_m.chunks_mut(BAND))
                .zip(self.glow_m.chunks_mut(BAND))
                .zip(self.h_m.chunks_mut(BAND))
                .enumerate()
                .map(|(i, ((((((beam, fast), slow), hblur), beam_m), glow_m), h_m))| Band {
                    y0: i * BAND,
                    beam,
                    fast,
                    slow,
                    hblur,
                    beam_m,
                    glow_m,
                    h_m,
                })
                .collect();
            pool.each(bands, |b| {
                let rows = b.beam_m.len();
                let run = IRect::new(0, b.y0 as i32, tw as i32, (b.y0 + rows) as i32);
                // Last frame's beam is gone.
                for j in 0..rows {
                    for (x0, x1) in blocks.runs(b.beam_m[j]) {
                        b.beam[j * stride + x0 * 3..j * stride + x1 * 3].fill(0.0);
                    }
                    b.beam_m[j] = 0;
                }
                // Glyph light and its halo.
                for j in 0..rows {
                    let y = b.y0 + j;
                    let row = &mut b.beam[j * stride..(j + 1) * stride];
                    let own = glyph_m[y];
                    if own != 0 {
                        let src = &glyph[y * stride..][..stride];
                        for (x0, x1) in blocks.runs(own) {
                            for (o, s) in row[x0 * 3..x1 * 3].iter_mut().zip(&src[x0 * 3..x1 * 3]) {
                                *o += GLYPH * s;
                            }
                        }
                        b.beam_m[j] |= own;
                    }
                    for &(dx, dy) in &offsets {
                        let sy = y as i32 + dy;
                        if sy < 0 || sy >= th as i32 {
                            continue;
                        }
                        let m = glyph_m[sy as usize];
                        if m == 0 {
                            continue;
                        }
                        let src = &glyph[sy as usize * stride..][..stride];
                        let k = GLYPH_BLOOM / 8.0;
                        for (s0, s1) in blocks.runs(m) {
                            // Output x takes source x + dx.
                            let lo = (s0 as i32 - dx).max(0) as usize;
                            let hi = ((s1 as i32 - dx).min(tw as i32)).max(0) as usize;
                            if lo >= hi {
                                continue;
                            }
                            let out = &mut row[lo * 3..hi * 3];
                            let inp = &src[(lo as i32 + dx) as usize * 3..(hi as i32 + dx) as usize * 3];
                            for (o, s) in out.iter_mut().zip(inp) {
                                *o += k * s;
                            }
                            b.beam_m[j] |= blocks.of(lo, hi);
                        }
                    }
                }
                // The page's light.
                l.page.draw_light(b.beam, b.y0 as i32, tw, run, b.beam_m, shift, l.lin);
                // The shapes' beam.
                for s in l.shapes {
                    let [r, g, bl, a] = s.rgba();
                    let k = a as f32 / 255.0;
                    let c = [l.lin[r as usize] * k, l.lin[g as usize] * k, l.lin[bl as usize] * k];
                    visit(s, &geom, Mode::Light, run, |x, y, v| {
                        let j = (y - b.y0 as i32) as usize;
                        let p = &mut b.beam[j * stride + x as usize * 3..][..3];
                        p[0] += c[0] * v;
                        p[1] += c[1] * v;
                        p[2] += c[2] * v;
                        b.beam_m[j] |= 1 << (x as u32 >> shift);
                    });
                }
                if l.advance {
                    let (df, ds) = (DEPOSIT * FAST_SHARE, DEPOSIT * (1.0 - FAST_SHARE));
                    let (kf, ks) = if l.clear { (0.0, 0.0) } else { (FAST_DECAY, slow_decay) };
                    let eps = if l.clear { 0.0 } else { EPSILON };
                    for j in 0..rows {
                        let m = b.glow_m[j] | b.beam_m[j];
                        for (x0, x1) in blocks.runs(m) {
                            let (lo, hi) = (j * stride + x0 * 3, j * stride + x1 * 3);
                            let (beam, fast, slow) = (&b.beam[lo..hi], &mut b.fast[lo..hi], &mut b.slow[lo..hi]);
                            if trails {
                                for ((f, s), &bm) in fast.iter_mut().zip(slow.iter_mut()).zip(beam) {
                                    *f = (*f * kf - eps).max(bm * df);
                                    *s = (*s * ks - eps).max(bm * ds);
                                }
                            } else {
                                fast.fill(0.0);
                                slow.fill(0.0);
                            }
                        }
                        b.glow_m[j] = if trails { m } else { 0 };
                    }
                    if trails {
                        for m in l.smears {
                            let [r, g, bl, a] = m.shape.rgba();
                            let k = a as f32 / 255.0;
                            let c = [l.lin[r as usize] * k, l.lin[g as usize] * k, l.lin[bl as usize] * k];
                            visit(&m.shape, &geom, Mode::Light, run, |x, y, v| {
                                let j = (y - b.y0 as i32) as usize;
                                let at = j * stride + x as usize * 3;
                                for ch in 0..3 {
                                    let light = c[ch] * v;
                                    b.fast[at + ch] = b.fast[at + ch].max(light * m.fast);
                                    b.slow[at + ch] = b.slow[at + ch].max(light * m.slow);
                                }
                                b.glow_m[j] |= 1 << (x as u32 >> shift);
                            });
                        }
                    }
                    // Blocks of afterglow that have gone dark.
                    for j in 0..rows {
                        let mut m = b.glow_m[j];
                        let mut keep = 0;
                        while m != 0 {
                            let i = m.trailing_zeros();
                            m &= m - 1;
                            let (x0, x1) = ((i as usize) << shift, (((i + 1) as usize) << shift).min(tw));
                            let (lo, hi) = (j * stride + x0 * 3, j * stride + x1 * 3);
                            if b.fast[lo..hi].iter().chain(&b.slow[lo..hi]).any(|&v| v > 0.0) {
                                keep |= 1 << i;
                            } else {
                                // Exactly zero, as outside the mask.
                                b.fast[lo..hi].fill(0.0);
                                b.slow[lo..hi].fill(0.0);
                            }
                        }
                        b.glow_m[j] = keep;
                    }
                }
                // The horizontal blur of the whitened light, over the lit
                // blocks and those within its reach.
                let mut src = Vec::new();
                for j in 0..rows {
                    let lit = b.beam_m[j] | b.glow_m[j];
                    let out_m = blocks.dilate(lit, reach);
                    // Keep the blur zero outside its mask.
                    for (x0, x1) in blocks.runs(b.h_m[j] & !out_m) {
                        b.hblur[j * stride + x0 * 3..j * stride + x1 * 3].fill(0.0);
                    }
                    b.h_m[j] = out_m;
                    let n = taps;
                    for (lo, hi) in blocks.runs(out_m) {
                        // The source over the run, zero-padded by the taps on
                        // each side: column x at index x - lo + n.
                        src.clear();
                        src.resize((hi - lo + 2 * n) * 3, 0.0);
                        for x in lo..hi {
                            let at = j * stride + x * 3;
                            let c = [
                                b.beam[at] + b.fast[at] + b.slow[at],
                                b.beam[at + 1] + b.fast[at + 1] + b.slow[at + 1],
                                b.beam[at + 2] + b.fast[at + 2] + b.slow[at + 2],
                            ];
                            let peak = c[0].max(c[1]).max(c[2]);
                            let white = (peak - BLOOM_WHITE_FROM).max(0.0) * BLOOM_WHITE;
                            let o = (x - lo + n) * 3;
                            src[o] = c[0] + white;
                            src[o + 1] = c[1] + white;
                            src[o + 2] = c[2] + white;
                        }
                        let out = &mut b.hblur[j * stride + lo * 3..j * stride + hi * 3];
                        out.fill(0.0);
                        for (i, &w) in weights.iter().enumerate() {
                            // Output x reads source x + i - n, at index x - lo + i.
                            let inp = &src[i * 3..i * 3 + out.len()];
                            for (o, s) in out.iter_mut().zip(inp) {
                                *o += w * s;
                            }
                        }
                    }
                }
            });
        }

        // Pass 2: the vertical blur.
        let hblur = &self.hblur;
        let h_m = &self.h_m;
        let n = taps as i32;
        let bands: Vec<(usize, &mut [f32], &mut [Mask])> = self
            .bloom
            .chunks_mut(BAND * stride)
            .zip(self.bloom_m.chunks_mut(BAND))
            .enumerate()
            .map(|(i, (bloom, m))| (i * BAND, bloom, m))
            .collect();
        pool.each(bands, |(y0, bloom, bloom_m)| {
            for j in 0..bloom_m.len() {
                let y = (y0 + j) as i32;
                let mut m = 0;
                for i in (y - n).max(0)..(y + n + 1).min(th as i32) {
                    m |= h_m[i as usize];
                }
                for (x0, x1) in blocks.runs(bloom_m[j] & !m) {
                    bloom[j * stride + x0 * 3..j * stride + x1 * 3].fill(0.0);
                }
                bloom_m[j] = m;
                for (x0, x1) in blocks.runs(m) {
                    let out = &mut bloom[j * stride + x0 * 3..j * stride + x1 * 3];
                    out.fill(0.0);
                    for (k, &w) in weights.iter().enumerate() {
                        let sy = y + k as i32 - n;
                        if sy < 0 || sy >= th as i32 || h_m[sy as usize] & blocks.of(x0, x1) == 0 {
                            continue;
                        }
                        let row = &hblur[sy as usize * stride + x0 * 3..sy as usize * stride + x1 * 3];
                        for (o, s) in out.iter_mut().zip(row) {
                            *o += w * s;
                        }
                    }
                }
            }
        });
    }

    /// Writes the picture of the light (what the last `frame` left) into
    /// `image`, whose rows are `stride` pixels and which shows the tube's
    /// top left at (`ox`, `oy`) of its own pixels; only rows `rows` of the
    /// tube, and only where light is or was.
    pub fn composite(&mut self, image: &mut [u32], stride: usize, ox: usize, oy: usize, rows: std::ops::Range<usize>, pool: &Pool) {
        let (tw, th) = self.size;
        let tstride = tw * 3;
        let blocks = self.blocks;
        let rows = rows.start.min(th)..rows.end.min(th);
        let (beam, fast, slow, bloom) = (&self.beam, &self.fast, &self.slow, &self.bloom);
        let (beam_m, glow_m, bloom_m) = (&self.beam_m, &self.glow_m, &self.bloom_m);
        let srgb = &self.srgb;
        let first = (oy + rows.start) * stride;
        let image = &mut image[first..first + rows.len() * stride];
        let shown = &mut self.shown_m[rows.clone()];
        let bands: Vec<(usize, &mut [u32], &mut [Mask])> = image
            .chunks_mut(BAND * stride)
            .zip(shown.chunks_mut(BAND))
            .enumerate()
            .map(|(i, (img, shown))| (rows.start + i * BAND, img, shown))
            .collect();
        pool.each(bands, |(y0, img, shown)| {
            let mut light = vec![0.0f32; tstride];
            for j in 0..shown.len() {
                let y = y0 + j;
                let now = beam_m[y] | glow_m[y] | bloom_m[y];
                let out = &mut img[j * stride + ox..j * stride + ox + tw];
                // Black where there was light and is none now.
                for (x0, x1) in blocks.runs(shown[j] & !now) {
                    out[x0..x1].fill(0);
                }
                shown[j] = now;
                let at = y * tstride;
                for (x0, x1) in blocks.runs(now) {
                    // In three passes over the run, the first and last of
                    // which vectorise: the light, the spill (per pixel), and
                    // the tone curve to an index into the sRGB table.
                    let (lo, hi) = (at + x0 * 3, at + x1 * 3);
                    let c = &mut light[..hi - lo];
                    for (k, c) in c.iter_mut().enumerate() {
                        let i = lo + k;
                        *c = beam[i] + fast[i] + slow[i] + BLOOM * bloom[i];
                    }
                    for p in c.chunks_exact_mut(3) {
                        let spill = (p[0].max(p[1]).max(p[2]) - SPILL_FROM).max(0.0) * SPILL;
                        p[0] += spill;
                        p[1] += spill;
                        p[2] += spill;
                    }
                    for v in c.iter_mut() {
                        *v = tone(*v).min(1.0) * SRGB_STEPS as f32;
                    }
                    for (o, p) in out[x0..x1].iter_mut().zip(c.chunks_exact(3)) {
                        *o = pack(srgb[p[0] as usize], srgb[p[1] as usize], srgb[p[2] as usize]);
                    }
                }
            }
        });
    }

    /// Forgets what the picture showed in `rows` (the caller blackened them).
    pub fn forget_shown(&mut self, rows: std::ops::Range<usize>) {
        let end = rows.end.min(self.shown_m.len());
        self.shown_m[rows.start.min(end)..end].fill(0);
    }

    /// The glyph light: each pixel's glyph (or cursor) coverage times its
    /// foreground colour, linear; rows without glyphs are skipped.
    fn glyph_light(&mut self, l: &Light, pool: &Pool) {
        let (tw, _) = self.size;
        let stride = tw * 3;
        let grid = &l.grid;
        let shift = self.blocks.shift;
        let bands: Vec<(usize, &mut [f32], &mut [Mask])> = self
            .glyph
            .chunks_mut(BAND * stride)
            .zip(self.glyph_m.chunks_mut(BAND))
            .enumerate()
            .map(|(i, (g, m))| (i * BAND, g, m))
            .collect();
        pool.each(bands, |(y0, rows, masks)| {
            for j in 0..masks.len() {
                masks[j] = 0;
                let Some((row, gy)) = grid.row(y0 + j) else { continue };
                if !grid.row_has_glyphs(row) {
                    continue;
                }
                let out = &mut rows[j * stride..(j + 1) * stride];
                let mut m = 0;
                for x in 0..tw {
                    let o = &mut out[x * 3..x * 3 + 3];
                    let Some((col, gx)) = grid.col(x) else {
                        o.fill(0.0);
                        continue;
                    };
                    let (cov, fg) = grid.coverage(row, gy, col, gx);
                    if cov == 0 {
                        o.fill(0.0);
                        continue;
                    }
                    let k = cov as f32 / 255.0;
                    let c = &grid.lin[fg as usize];
                    o[0] = c[0] * k;
                    o[1] = c[1] * k;
                    o[2] = c[2] * k;
                    m |= 1 << (x as u32 >> shift);
                }
                masks[j] = m;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        let lin = linear_table();
        assert_eq!(lin[0], 0.0);
        assert!((lin[255] - 1.0).abs() < 1e-6);
        let srgb = srgb_table();
        assert_eq!(srgb[0], 0);
        assert_eq!(srgb[SRGB_STEPS], 255);
        // The table agrees with the exact conversion within one step.
        for i in 0..=255 {
            let v = lin[i];
            let got = srgb[(v * SRGB_STEPS as f32) as usize] as i32;
            assert!((got - i as i32).abs() <= 1, "{i}: {got}");
        }
    }

    #[test]
    fn blocks_and_runs() {
        let b = Blocks::new(1320);
        assert_eq!(b.shift, 4);
        assert_eq!(b.of(0, 1), 1);
        assert_eq!(b.of(15, 17), 0b11);
        assert_eq!(b.of(5, 5), 0);
        let runs: Vec<_> = b.runs(0b1101 | 1 << 82).collect();
        assert_eq!(runs, vec![(0, 16), (32, 64), (1312, 1320)]);
        assert_eq!(b.dilate(0b100, 1), 0b1110);
        assert_eq!(b.dilate(1 << 82, 2), 0b111 << 80);
        let wide = Blocks::new(3840);
        assert_eq!(wide.shift, 5);
        assert_eq!(wide.runs(Mask::MAX).collect::<Vec<_>>(), vec![(0, 3840)]);
    }
}

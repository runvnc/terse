//! The phosphor presentation (SPEC §vector.phosphor) on the CPU: the model of
//! `phosphor.rs` with its constants, computed as the GPU renderer's passes
//! compute it, in linear-light floats the size of the screen:
//!
//! 1. the text: each glyph and the cursor in its foreground colour, and the
//!    page's text and figures, times `GLYPH`;
//! 2. the beam: the shapes' Gaussian light (`shapes.rs`), added up;
//! 3. the afterglow: both components faded (× their factor, − `EPSILON`),
//!    then beam + text deposited with `max`, then the smear of moving
//!    shapes;
//! 4. the halo's pyramid: what it blurs (`max(beam + TEXT_HALO × text,
//!    afterglow)`, whitened above one stroke) averaged 4 × 4 into the
//!    first level (a quarter of the resolution), and each level from the
//!    one before down to 1/32; each of `HALO`'s Gaussians blurred on its level
//!    (`phosphor::halo_blurs`) and added, times its weight, to that level's
//!    sum; each level's sum then takes the one below it, sampled up
//!    bilinearly;
//! 5. `max(beam + text, afterglow)` plus the halo (the first level's sum, sampled
//!    up), tinted for a monochrome tube, spilled into white where far
//!    brighter than one stroke, through the tone curve, the glass, to sRGB.
//!
//! The differences from the GPU are of rounding: the GPU keeps the light
//! in 16-bit floats and converts to sRGB exactly where this uses a
//! 4,096-entry table.
//!
//! Work at full resolution is limited to where there is light: each row
//! keeps, per buffer, a bit mask of the blocks of columns where it may be
//! non-zero (and keeps it zero elsewhere), so a dark screen with a few
//! shapes costs those shapes and their halo, not the whole screen. The
//! pyramid's levels are small and computed whole.

use std::sync::atomic::{AtomicBool, Ordering};

use super::base::{IRect, Pool, pack};
use super::page::PageQuads;
use super::screen::Grid;
use super::shapes::{Geom, Mode, Shape, visit};
use crate::phosphor::{
    BLOOM_WHITE, BLOOM_WHITE_FROM, DEPOSIT, EPSILON, FAST_SHARE, GLYPH, HALO_FIRST, HALO_LEVELS, HaloBlur, Look, SPILL, SPILL_FROM,
    TEXT_HALO, halo_blurs, kernel, level_size, tinted, tone_scale, up, vignette,
};

/// Halo light below this is left out of the picture's masks: on its own
/// it shows as 0 in 8-bit sRGB.
const HALO_FLOOR: f32 = 1.2e-4;

/// Rows handed to a thread at a time (even, so a band holds whole rows of
/// the pyramid's first level).
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

/// The entry of the sRGB table for tone-mapped light `v` (0–1): the
/// nearest step.
#[inline]
fn index(v: f32) -> usize {
    step(v.clamp(0.0, 1.0) * SRGB_STEPS as f32)
}

/// `v` (0 to 2²²) rounded to the nearest integer by the float trick (adding
/// 1.5 × 2²³ leaves it in the low bits), cheaper than a saturating cast.
#[inline]
fn step(v: f32) -> usize {
    ((v + 12_582_912.0).to_bits() & 0x3F_FFFF) as usize
}

/// One afterglow deposit of a moving shape (`phosphor::deposits`): the
/// shape and what it adds to the fast and slow components per unit light.
pub struct Smear {
    pub shape: Shape,
    pub fast: f32,
    pub slow: f32,
}

/// Layers of light drawn elsewhere (terse-bus's drivers in light mode,
/// `DEVICES.md` 7.1): each pixel straight sRGB colour in its low 24 bits
/// and its coverage in its alpha, giving `linear(colour) × alpha / 255 ×
/// GLYPH` of light, as the terminal's glyphs give in [`Text::Cells`].
pub struct LightLayers<'a> {
    /// Each layer's pixels, rows of `width`.
    pub layers: Vec<&'a [u32]>,
    pub width: usize,
    pub height: usize,
    /// The layers' pixel that is the tube's (0, 0).
    pub origin: (usize, usize),
    /// Per layer, per row of the layer: the columns that may hold light
    /// (x0..x1; none if x0 ≥ x1).
    pub extents: Vec<&'a [(u32, u32)]>,
}

/// Where the text's light comes from.
pub enum Text<'a> {
    /// The terminal's cells and the page, prepared for the tube
    /// (`SoftPage::prepare` with the screen at the origin and the tube's
    /// size): the software renderer's own.
    Cells { grid: Grid<'a>, page: &'a PageQuads },
    /// Layers drawn as light by other processes.
    Layers(&'a LightLayers<'a>),
}

/// What a machine frame of light is made of.
pub struct Light<'a> {
    pub text: Text<'a>,
    pub shapes: &'a [Shape],
    pub smears: &'a [Smear],
    pub lin: &'a [f32; 256],
    /// Persistence 0–255 (0: no trails).
    pub persistence: u8,
    /// The tube's preset and glass.
    pub look: Look,
    /// Start the afterglow afresh.
    pub clear: bool,
    /// Advance the afterglow (else only the beam is redrawn).
    pub advance: bool,
    /// Rectangles (x0, y0, x1, y1 pixels of the tube) that give no light:
    /// the sources of the machine's Surfaces (SPEC §vector3d.presentation).
    pub excluded: &'a [(usize, usize, usize, usize)],
    /// How much of the look is shed (`phosphor::shed_level`): from
    /// `SHED_HALO` no halo, at `SHED_PLAIN` the lines drawn plain.
    pub shed: u8,
}

/// One level of the halo's pyramid: the light averaged down to it, a
/// blur's first pass, and its sum (its own blurs and the levels below).
struct Level {
    size: (usize, usize),
    down: Vec<f32>,
    tmp: Vec<f32>,
    sum: Vec<f32>,
}

/// One screen's tube: the light textures of the GPU renderer as floats,
/// linear RGB, three floats a texel.
pub struct Tube {
    pub size: (usize, usize),
    blocks: Blocks,
    beam: Vec<f32>,
    text: Vec<f32>,
    fast: Vec<f32>,
    slow: Vec<f32>,
    /// Per row: the blocks where each buffer may be non-zero (it is zero
    /// elsewhere), and those the last composite left non-black.
    beam_m: Vec<Mask>,
    text_m: Vec<Mask>,
    glow_m: Vec<Mask>,
    shown_m: Vec<Mask>,
    /// The pyramid, levels `HALO_FIRST` to `HALO_LEVELS`, the blurs on it,
    /// and per row of its first level the blocks (of the screen's columns)
    /// where its sum, the halo, shows.
    levels: Vec<Level>,
    blurs: Vec<(HaloBlur, Vec<f32>)>,
    halo_m: Vec<Mask>,
    /// Per row of the first level: the blocks (of the screen's columns)
    /// where the light averaged into it may be non-zero, and where a blur's
    /// first pass may be.
    down_m: Vec<Mask>,
    tmp_m: Vec<Mask>,
    /// For each column of the screen, the columns of the first level it
    /// samples and the second's weight.
    cols: Vec<(u32, u32, f32)>,
    /// The pyramid holds light (else it is all zero).
    halo_lit: bool,
    srgb: Vec<u8>,
}

/// Per-row slices of the tube's buffers for one band of rows.
struct Band<'a> {
    y0: usize,
    beam: &'a mut [f32],
    fast: &'a mut [f32],
    slow: &'a mut [f32],
    down: &'a mut [f32],
    down_m: &'a mut [Mask],
    beam_m: &'a mut [Mask],
    glow_m: &'a mut [Mask],
}

/// Runs `f(y, row)` on every row of `data` (rows `stride` floats), a band
/// of rows per thread, or all on this one if there is little to do: `work`
/// is about how many operations `f` does per float.
fn par_rows(pool: &Pool, data: &mut [f32], stride: usize, work: usize, f: impl Fn(usize, &mut [f32]) + Sync + Send) {
    if data.len() * work < 1 << 18 {
        for (y, row) in data.chunks_mut(stride).enumerate() {
            f(y, row);
        }
        return;
    }
    let bands: Vec<(usize, &mut [f32])> = data.chunks_mut(BAND * stride).enumerate().map(|(i, c)| (i * BAND, c)).collect();
    pool.each(bands, |(y0, chunk)| {
        for (j, row) in chunk.chunks_mut(stride).enumerate() {
            f(y0 + j, row);
        }
    });
}

/// The pyramid's first level is this many pixels of the screen across.
const FIRST: usize = 1 << HALO_FIRST;

impl Tube {
    pub fn new(size: (usize, usize)) -> Self {
        let n = size.0 * size.1 * 3;
        let rows = size.1;
        let blocks = Blocks::new(size.0);
        let levels: Vec<Level> = (HALO_FIRST..=HALO_LEVELS)
            .map(|k| {
                let s = level_size(size, k);
                let n = s.0 * s.1 * 3;
                Level { size: s, down: vec![0.0; n], tmp: vec![0.0; n], sum: vec![0.0; n] }
            })
            .collect();
        let blurs = halo_blurs(size.1 as f32).into_iter().map(|b| (b, kernel(&b))).collect();
        Tube {
            size,
            blocks,
            beam: vec![0.0; n],
            text: vec![0.0; n],
            fast: vec![0.0; n],
            slow: vec![0.0; n],
            beam_m: vec![0; rows],
            text_m: vec![0; rows],
            glow_m: vec![0; rows],
            shown_m: vec![blocks.all(); rows],
            halo_m: vec![0; levels[0].size.1],
            down_m: vec![0; levels[0].size.1],
            tmp_m: vec![0; levels[0].size.1],
            cols: (0..size.0).map(|x| up(x, levels[0].size.0, FIRST)).map(|(a, b, t)| (a as u32, b as u32, t)).collect(),
            levels,
            blurs,
            halo_lit: false,
            srgb: srgb_table(),
        }
    }

    /// One frame of light (or with `advance` false, the beam redrawn).
    pub fn frame(&mut self, l: &Light, pool: &Pool) {
        let (tw, _) = self.size;
        let stride = tw * 3;
        let blocks = self.blocks;
        self.text_light(l, pool);

        let trails = l.persistence > 0;
        let slow_decay = crate::phosphor::slow_decay(l.persistence);
        let fast_decay = l.look.preset.fast_decay();
        let geom = Geom::for_rect((0.0, 0.0, tw as f32, self.size.1 as f32));
        let shift = blocks.shift;
        let (lw, _) = self.levels[0].size;
        let lstride = lw * 3;

        // Pass 1: beam, afterglow and the halo's first level, band by band.
        let changed = AtomicBool::new(false);
        {
            let changed = &changed;
            let text = &self.text;
            let text_m = &self.text_m;
            let bands: Vec<Band> = self
                .beam
                .chunks_mut(BAND * stride)
                .zip(self.fast.chunks_mut(BAND * stride))
                .zip(self.slow.chunks_mut(BAND * stride))
                .zip(self.levels[0].down.chunks_mut(BAND / FIRST * lstride))
                .zip(self.down_m.chunks_mut(BAND / FIRST))
                .zip(self.beam_m.chunks_mut(BAND))
                .zip(self.glow_m.chunks_mut(BAND))
                .enumerate()
                .map(|(i, ((((((beam, fast), slow), down), down_m), beam_m), glow_m))| Band {
                    y0: i * BAND,
                    beam,
                    fast,
                    slow,
                    down,
                    down_m,
                    beam_m,
                    glow_m,
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
                // The shapes' beam.
                // Plain lines: each a flat stroke of its width at the
                // core's brightness, as the clean style covers it.
                let (mode, gain) = if l.shed >= crate::phosphor::SHED_PLAIN { (Mode::Cover, crate::phosphor::CORE) } else { (Mode::Light, 1.0) };
                for s in l.shapes {
                    let [r, g, bl, a] = s.rgba();
                    let k = a as f32 / 255.0 * gain;
                    let c = [l.lin[r as usize] * k, l.lin[g as usize] * k, l.lin[bl as usize] * k];
                    visit(s, &geom, mode, run, |x, y, v| {
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
                    let (kf, ks) = if l.clear { (0.0, 0.0) } else { (fast_decay, slow_decay) };
                    let eps = if l.clear { 0.0 } else { EPSILON };
                    for j in 0..rows {
                        let y = b.y0 + j;
                        let lit = b.beam_m[j] | text_m[y];
                        let m = b.glow_m[j] | lit;
                        for (x0, x1) in blocks.runs(m) {
                            let (lo, hi) = (j * stride + x0 * 3, j * stride + x1 * 3);
                            let (fast, slow) = (&mut b.fast[lo..hi], &mut b.slow[lo..hi]);
                            if !trails {
                                fast.fill(0.0);
                                slow.fill(0.0);
                                continue;
                            }
                            let beam = &b.beam[lo..hi];
                            let tx = &text[y * stride + x0 * 3..y * stride + x1 * 3];
                            for (((f, s), &bm), &t) in fast.iter_mut().zip(slow.iter_mut()).zip(beam).zip(tx) {
                                let v = bm + t;
                                *f = (*f * kf - eps).max(v * df);
                                *s = (*s * ks - eps).max(v * ds);
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
                            // (Without stopping early: it vectorises.)
                            let lit = |v: &[f32]| v.iter().fold(false, |a, &v| a | (v > 0.0));
                            if lit(&b.slow[lo..hi]) || lit(&b.fast[lo..hi]) {
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
                // The halo's first level: what it blurs, whitened, averaged
                // over FIRST × FIRST pixels; zero where no light is. Whether
                // it changed is noted: if not, nor does the halo.
                let mut any = false;
                let (mut src, mut row) = (Vec::new(), Vec::new());
                for jj in 0..b.down_m.len() {
                    let out = &mut b.down[jj * lstride..(jj + 1) * lstride];
                    let pair = (jj * FIRST)..(jj * FIRST + FIRST).min(rows);
                    let lit = pair.clone().fold(0, |m, j| m | b.beam_m[j] | b.glow_m[j] | text_m[b.y0 + j]);
                    for (x0, x1) in blocks.runs(b.down_m[jj] & !lit) {
                        let gone = &mut out[x0 / FIRST * 3..x1.div_ceil(FIRST) * 3];
                        if gone.iter().any(|&v| v != 0.0) {
                            changed.store(true, Ordering::Relaxed);
                        }
                        gone.fill(0.0);
                    }
                    b.down_m[jj] = lit;
                    for (x0, x1) in blocks.runs(lit) {
                        // Columns x0..x1 of the screen, x0 a multiple of FIRST
                        // (blocks are).
                        src.clear();
                        src.resize((x1 - x0) * 3, 0.0);
                        for j in pair.clone() {
                            let y = b.y0 + j;
                            let (lo, hi) = (j * stride + x0 * 3, j * stride + x1 * 3);
                            let (bm, fa, sl) = (&b.beam[lo..hi], &b.fast[lo..hi], &b.slow[lo..hi]);
                            let tx = &text[y * stride + x0 * 3..y * stride + x1 * 3];
                            row.clear();
                            row.extend(bm.iter().zip(tx).zip(fa.iter().zip(sl)).map(|((b, t), (f, s))| (b + TEXT_HALO * t).max(f + s)));
                            // What the machine's Surfaces show does not
                            // glow onto its glass.
                            for &(ex0, ey0, ex1, ey1) in l.excluded {
                                let (a, z) = (ex0.max(x0), ex1.min(x1));
                                if y >= ey0 && y < ey1 && a < z {
                                    row[(a - x0) * 3..(z - x0) * 3].fill(0.0);
                                }
                            }
                            for (acc, c) in src.chunks_exact_mut(3).zip(row.chunks_exact(3)) {
                                let white = (c[0].max(c[1]).max(c[2]) - BLOOM_WHITE_FROM).max(0.0) * BLOOM_WHITE;
                                acc[0] += c[0] + white;
                                acc[1] += c[1] + white;
                                acc[2] += c[2] + white;
                            }
                        }
                        // The mean of each FIRST × FIRST (fewer at an edge).
                        let rows_in = pair.len() as f32;
                        for (x, d) in out[x0 / FIRST * 3..x1.div_ceil(FIRST) * 3].chunks_exact_mut(3).enumerate() {
                            let cells = &src[x * FIRST * 3..((x + 1) * FIRST * 3).min(src.len())];
                            let k = 1.0 / (cells.len() / 3) as f32 / rows_in;
                            let mut acc = [0.0; 3];
                            for c in cells.chunks_exact(3) {
                                acc[0] += c[0];
                                acc[1] += c[1];
                                acc[2] += c[2];
                            }
                            let v = [acc[0] * k, acc[1] * k, acc[2] * k];
                            if d != v {
                                d.copy_from_slice(&v);
                                any = true;
                            }
                        }
                    }
                }
                if any {
                    changed.store(true, Ordering::Relaxed);
                }
            });
        }
        self.halo(changed.into_inner(), l.shed < crate::phosphor::SHED_HALO, pool);
    }

    /// The halo's pyramid from its first level (`frame`): the levels
    /// below, the blurs, the sums up from the smallest level, and where the
    /// halo shows; nothing if the first level is as it was (`changed`
    /// false).
    fn halo(&mut self, changed: bool, shown: bool, pool: &Pool) {
        let lit = shown && self.beam_m.iter().chain(&self.text_m).chain(&self.glow_m).any(|&m| m != 0);
        if (!lit && !self.halo_lit) || (!changed && lit == self.halo_lit) {
            return;
        }
        self.halo_lit = lit;
        if !lit {
            for level in &mut self.levels {
                level.sum.fill(0.0);
            }
            self.halo_m.fill(0);
            return;
        }
        // Each level the 2 × 2 mean of the one above (fewer at an odd edge).
        for k in 1..self.levels.len() {
            let (above, below) = self.levels.split_at_mut(k);
            let src = &above[k - 1];
            let dst = &mut below[0];
            let ((sw, sh), (dw, _)) = (src.size, dst.size);
            par_rows(pool, &mut dst.down, dw * 3, 4, |y, row| {
                let rows: &[usize] = if 2 * y + 1 < sh { &[2 * y, 2 * y + 1] } else { &[2 * y] };
                for (x, d) in row.chunks_exact_mut(3).enumerate() {
                    let cols: &[usize] = if 2 * x + 1 < sw { &[2 * x, 2 * x + 1] } else { &[2 * x] };
                    let k = 1.0 / (rows.len() * cols.len()) as f32;
                    let mut acc = [0.0; 3];
                    for &r in rows {
                        for &c in cols {
                            let p = &src.down[(r * sw + c) * 3..][..3];
                            acc[0] += p[0];
                            acc[1] += p[1];
                            acc[2] += p[2];
                        }
                    }
                    d.copy_from_slice(&[acc[0] * k, acc[1] * k, acc[2] * k]);
                }
            });
        }
        // From the smallest level up: the level below sampled up, then the
        // level's own blurs added.
        for k in (0..self.levels.len()).rev() {
            let (here, rest) = self.levels.split_at_mut(k + 1);
            let level = &mut here[k];
            let (w, h) = level.size;
            let stride = w * 3;
            match rest.first() {
                Some(lower) => {
                    let (lw, lh) = lower.size;
                    let ls = &lower.sum;
                    let cols: Vec<(usize, usize, f32)> = (0..w).map(|x| up(x, lw, 2)).collect();
                    par_rows(pool, &mut level.sum, stride, 6, |y, out| {
                        let (r0, r1, ty) = up(y, lh, 2);
                        let (a, z) = (&ls[r0 * lw * 3..][..lw * 3], &ls[r1 * lw * 3..][..lw * 3]);
                        for (o, &(c0, c1, tx)) in out.chunks_exact_mut(3).zip(&cols) {
                            for ch in 0..3 {
                                let top = a[c0 * 3 + ch] + (a[c1 * 3 + ch] - a[c0 * 3 + ch]) * tx;
                                let bottom = z[c0 * 3 + ch] + (z[c1 * 3 + ch] - z[c0 * 3 + ch]) * tx;
                                o[ch] = top + (bottom - top) * ty;
                            }
                        }
                    });
                }
                None => level.sum.fill(0.0),
            }
            // On the first level the blurs work only where light is (each
            // row's blocks, in the screen's columns: the level's are as many,
            // FIRST times narrower); the smaller levels are blurred whole.
            let all = (k > 0).then(|| vec![Mask::MAX; h]);
            let bblocks = if k == 0 { Blocks { shift: self.blocks.shift - HALO_FIRST as u32, width: w } } else { Blocks { shift: 7, width: w } };
            for (b, weights) in self.blurs.iter().filter(|(b, _)| b.level == k + HALO_FIRST) {
                let n = b.taps as usize;
                let reach = (n as u32).div_ceil(1 << bblocks.shift);
                let down_m: &[Mask] = all.as_deref().unwrap_or(&self.down_m);
                // Along x, zero beyond the edges and outside the mask.
                let down = &level.down;
                let tmp_m = &mut self.tmp_m;
                for (m, &d) in tmp_m.iter_mut().zip(down_m) {
                    *m = bblocks.dilate(d, reach);
                }
                let tmp_m = &self.tmp_m;
                let tmp_m: &[Mask] = if k == 0 { tmp_m } else { all.as_deref().unwrap() };
                par_rows(pool, &mut level.tmp, stride, weights.len(), |y, out| {
                    let src = &down[y * stride..(y + 1) * stride];
                    out.fill(0.0);
                    for (x0, x1) in bblocks.runs(tmp_m[y]) {
                        for (i, &wt) in weights.iter().enumerate() {
                            // Output x reads source x + i - n.
                            let lo = x0.max(n.saturating_sub(i));
                            let hi = (w + n).saturating_sub(i).min(x1);
                            if lo >= hi {
                                continue;
                            }
                            let o = &mut out[lo * 3..hi * 3];
                            let s = &src[(lo + i - n) * 3..(hi + i - n) * 3];
                            for (o, s) in o.iter_mut().zip(s) {
                                *o += wt * s;
                            }
                        }
                    }
                });
                // Along y, times the weight, into the sum.
                let tmp = &level.tmp;
                let weight = b.weight;
                par_rows(pool, &mut level.sum, stride, weights.len(), |y, out| {
                    let rows = y.saturating_sub(n)..(y + n + 1).min(h);
                    let m = rows.fold(0, |m, r| m | tmp_m[r]);
                    for (x0, x1) in bblocks.runs(m) {
                        for (i, &wt) in weights.iter().enumerate() {
                            let sy = y as isize + i as isize - n as isize;
                            if sy < 0 || sy >= h as isize || tmp_m[sy as usize] & bblocks.of(x0, x1) == 0 {
                                continue;
                            }
                            let s = &tmp[sy as usize * stride + x0 * 3..sy as usize * stride + x1 * 3];
                            let wt = wt * weight;
                            for (o, s) in out[x0 * 3..x1 * 3].iter_mut().zip(s) {
                                *o += wt * s;
                            }
                        }
                    }
                });
            }
        }
        // Where the first level's sum shows, in blocks of the screen's columns.
        let first = &self.levels[0];
        let stride = first.size.0 * 3;
        let blocks = self.blocks;
        let scan = |(rows, masks): (&[f32], &mut [Mask])| {
            for (row, m) in rows.chunks(stride).zip(masks.iter_mut()) {
                let mut bits: Mask = 0;
                for (x, p) in row.chunks_exact(3).enumerate() {
                    if p[0].max(p[1]).max(p[2]) > HALO_FLOOR {
                        // The screen's pixels that sample it (bilinearly).
                        let x0 = (x * FIRST).saturating_sub(FIRST / 2);
                        bits |= blocks.of(x0, (x * FIRST + FIRST + FIRST / 2).min(blocks.width));
                    }
                }
                *m = bits;
            }
        };
        if first.sum.len() < 1 << 16 {
            scan((&first.sum, &mut self.halo_m));
        } else {
            pool.each(first.sum.chunks(BAND * stride).zip(self.halo_m.chunks_mut(BAND)).collect(), scan);
        }
    }

    /// Writes the picture of the light (what the last `frame` left) into
    /// `image`, whose rows are `stride` pixels and which shows the tube's
    /// top left at (`ox`, `oy`) of its own pixels; only rows `rows` of the
    /// tube, and only where light is or was.
    #[allow(clippy::too_many_arguments)]
    pub fn composite(&mut self, image: &mut [u32], stride: usize, ox: usize, oy: usize, rows: std::ops::Range<usize>, look: Look, pool: &Pool) {
        let (tw, th) = self.size;
        let tstride = tw * 3;
        let blocks = self.blocks;
        let rows = rows.start.min(th)..rows.end.min(th);
        let (beam, text, fast, slow) = (&self.beam, &self.text, &self.fast, &self.slow);
        let (beam_m, text_m, glow_m, halo_m) = (&self.beam_m, &self.text_m, &self.glow_m, &self.halo_m);
        let halo_lit = self.halo_lit;
        let first = &self.levels[0];
        let ((hw, hh), halo) = (first.size, &first.sum);
        // Where the pixels of each FIRST sample the first level, from the
        // one under them: the pair after it or before it, and the weight.
        let steps: [(bool, f32); FIRST] = std::array::from_fn(|r| {
            let (_, b, t) = up(FIRST + r, 3, FIRST);
            (b == 2, t)
        });
        let cols = &self.cols;
        let hstride = hw * 3;
        let srgb = &self.srgb;
        let [tr, tg, tb, glass] = look.uniform();
        let tint = look.preset.tint().map(|_| [tr, tg, tb]);
        let first_row = (oy + rows.start) * stride;
        let image = &mut image[first_row..first_row + rows.len() * stride];
        let shown = &mut self.shown_m[rows.clone()];
        let bands: Vec<(usize, &mut [u32], &mut [Mask])> = image
            .chunks_mut(BAND * stride)
            .zip(shown.chunks_mut(BAND))
            .enumerate()
            .map(|(i, (img, shown))| (rows.start + i * BAND, img, shown))
            .collect();
        pool.each(bands, |(y0, img, shown)| {
            let mut light = vec![0.0f32; tstride];
            let mut glow = vec![0.0f32; tstride];
            let (mut vrow, mut srow) = (Vec::new(), Vec::new());
            for j in 0..shown.len() {
                let y = y0 + j;
                // The halo's rows for this row, weighted.
                let (r0, r1, fy) = up(y, hh, FIRST);
                let halo_here = if halo_lit { halo_m[r0] | halo_m[r1] } else { 0 };
                let now = beam_m[y] | text_m[y] | glow_m[y] | halo_here;
                let out = &mut img[j * stride + ox..j * stride + ox + tw];
                // Black where there was light and is none now.
                for (x0, x1) in blocks.runs(shown[j] & !now) {
                    out[x0..x1].fill(0);
                }
                shown[j] = now;
                let at = y * tstride;
                if halo_here != 0 {
                    // The first level's two rows for this row, mixed.
                    let (row0, row1) = (&halo[r0 * hstride..][..hstride], &halo[r1 * hstride..][..hstride]);
                    vrow.clear();
                    vrow.extend(row0.iter().zip(row1).map(|(a, b)| a + (b - a) * fy));
                }
                let srgb: &[u8; SRGB_STEPS + 1] = srgb[..].try_into().expect("the table");
                let to = |v: f32| srgb[index(v)];
                // Where only the halo is (far below the knee and the spill):
                // it alone, sampled up.
                let lit = beam_m[y] | text_m[y] | glow_m[y];
                let plain = tint.is_none() && glass == 0.0;
                let only_halo = if plain { now & !lit } else { 0 };
                if only_halo != 0 {
                    // The row in steps of the sRGB table (the halo is never
                    // below 0).
                    srow.clear();
                    srow.extend(vrow.iter().map(|v| v * SRGB_STEPS as f32));
                }
                let at_step = |v: f32| srgb[step(v).min(SRGB_STEPS)];
                for (x0, x1) in blocks.runs(only_halo) {
                    // x0 is a multiple of FIRST: each FIRST pixels lie around
                    // one column c of the first level, the first half between
                    // c - 1 and c, the rest between c and c + 1 (`steps`).
                    for (i, group) in out[x0..x1].chunks_mut(FIRST).enumerate() {
                        let c = x0 / FIRST + i;
                        let v = |k: usize| -> &[f32; 3] { srow[k * 3..k * 3 + 3].try_into().expect("three") };
                        let (before, here, after) = (v(c.saturating_sub(1)), v(c), v((c + 1).min(hw - 1)));
                        for (o, &(second, t)) in group.iter_mut().zip(&steps) {
                            let (a, b) = if second { (here, after) } else { (before, here) };
                            *o = pack(at_step(a[0] + (b[0] - a[0]) * t), at_step(a[1] + (b[1] - a[1]) * t), at_step(a[2] + (b[2] - a[2]) * t));
                        }
                    }
                }
                for (x0, x1) in blocks.runs(now & !only_halo) {
                    let (lo, hi) = (at + x0 * 3, at + x1 * 3);
                    let g = &mut glow[..hi - lo];
                    if halo_here & blocks.of(x0, x1) != 0 {
                        // Each pixel between its two columns of the first level.
                        for (p, &(ia, ib, t)) in g.chunks_exact_mut(3).zip(&cols[x0..x1]) {
                            let (va, vb) = (&vrow[ia as usize * 3..][..3], &vrow[ib as usize * 3..][..3]);
                            p[0] = va[0] + (vb[0] - va[0]) * t;
                            p[1] = va[1] + (vb[1] - va[1]) * t;
                            p[2] = va[2] + (vb[2] - va[2]) * t;
                        }
                    } else {
                        g.fill(0.0);
                    }
                    // The light, then per pixel the tint, the spill, the
                    // tone curve and the glass.
                    let c = &mut light[..hi - lo];
                    let (bm, tx, fa, sl) = (&beam[lo..hi], &text[lo..hi], &fast[lo..hi], &slow[lo..hi]);
                    for (((((c, b), t), f), s), g) in c.iter_mut().zip(bm).zip(tx).zip(fa).zip(sl).zip(g.iter()) {
                        *c = (b + t).max(f + s) + g;
                    }
                    for (k, (o, p)) in out[x0..x1].iter_mut().zip(c.chunks_exact(3)).enumerate() {
                        let mut v = [p[0], p[1], p[2]];
                        if let Some(t) = tint {
                            v = tinted(v, t);
                        }
                        let spill = (v[0].max(v[1]).max(v[2]) - SPILL_FROM).max(0.0) * SPILL;
                        let v = [v[0] + spill, v[1] + spill, v[2] + spill];
                        let mut scale = tone_scale(v[0].max(v[1]).max(v[2]));
                        if glass > 0.0 {
                            let gx = 2.0 * (x0 + k) as f32 / tw as f32 - 1.0;
                            let gy = 2.0 * y as f32 / th as f32 - 1.0;
                            scale *= vignette(gx, gy, glass);
                        }
                        *o = pack(to(v[0] * scale), to(v[1] * scale), to(v[2] * scale));
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

    /// The text's light: each pixel's glyph (or cursor) coverage times its
    /// foreground colour, linear, and the page's light over it.
    fn text_light(&mut self, l: &Light, pool: &Pool) {
        let (grid, page) = match &l.text {
            Text::Cells { grid, page } => (grid, *page),
            Text::Layers(layers) => return self.layer_light(layers, l.lin, pool),
        };
        let (tw, _) = self.size;
        let stride = tw * 3;
        let blocks = self.blocks;
        let shift = blocks.shift;
        let bands: Vec<(usize, &mut [f32], &mut [Mask])> = self
            .text
            .chunks_mut(BAND * stride)
            .zip(self.text_m.chunks_mut(BAND))
            .enumerate()
            .map(|(i, (g, m))| (i * BAND, g, m))
            .collect();
        pool.each(bands, |(y0, rows, masks)| {
            for j in 0..masks.len() {
                for (x0, x1) in blocks.runs(masks[j]) {
                    rows[j * stride + x0 * 3..j * stride + x1 * 3].fill(0.0);
                }
                masks[j] = 0;
                let Some((row, gy)) = grid.row(y0 + j) else { continue };
                if !grid.row_has_glyphs(row) {
                    continue;
                }
                let out = &mut rows[j * stride..(j + 1) * stride];
                let mut m = 0;
                for x in 0..tw {
                    let Some((col, gx)) = grid.col(x) else { continue };
                    let (cov, fg) = grid.coverage(row, gy, col, gx);
                    if cov == 0 {
                        continue;
                    }
                    let k = cov as f32 / 255.0 * GLYPH;
                    let c = &grid.lin[fg as usize];
                    let o = &mut out[x * 3..x * 3 + 3];
                    o[0] = c[0] * k;
                    o[1] = c[1] * k;
                    o[2] = c[2] * k;
                    m |= 1 << (x as u32 >> shift);
                }
                masks[j] = m;
            }
            let run = IRect::new(0, y0 as i32, tw as i32, (y0 + masks.len()) as i32);
            page.draw_light(rows, y0 as i32, tw, run, masks, shift, l.lin);
        });
    }

    /// The text's light from layers of light ([`LightLayers`]): each
    /// pixel's coverage times its colour, linear, added up.
    fn layer_light(&mut self, src: &LightLayers, lin: &[f32; 256], pool: &Pool) {
        let (tw, _) = self.size;
        let stride = tw * 3;
        let shift = self.blocks.shift;
        let blocks = self.blocks;
        let (ox, oy) = src.origin;
        let bands: Vec<(usize, &mut [f32], &mut [Mask])> = self
            .text
            .chunks_mut(BAND * stride)
            .zip(self.text_m.chunks_mut(BAND))
            .enumerate()
            .map(|(i, (g, m))| (i * BAND, g, m))
            .collect();
        pool.each(bands, |(y0, rows, masks)| {
            for j in 0..masks.len() {
                for (x0, x1) in blocks.runs(masks[j]) {
                    rows[j * stride + x0 * 3..j * stride + x1 * 3].fill(0.0);
                }
                masks[j] = 0;
                let ly = y0 + j + oy;
                if ly >= src.height {
                    continue;
                }
                let out = &mut rows[j * stride..(j + 1) * stride];
                let mut m = 0;
                for (px, ext) in src.layers.iter().zip(&src.extents) {
                    let (e0, e1) = ext[ly];
                    let (x0, x1) = ((e0 as usize).max(ox), (e1 as usize).min(ox + tw).min(src.width));
                    if x0 >= x1 {
                        continue;
                    }
                    let row = &px[ly * src.width..(ly + 1) * src.width];
                    for x in x0..x1 {
                        let p = row[x];
                        let a = p >> 24;
                        if a == 0 {
                            continue;
                        }
                        let k = a as f32 / 255.0 * GLYPH;
                        let t = x - ox;
                        let o = &mut out[t * 3..t * 3 + 3];
                        o[0] += lin[(p >> 16 & 255) as usize] * k;
                        o[1] += lin[(p >> 8 & 255) as usize] * k;
                        o[2] += lin[(p & 255) as usize] * k;
                        m |= 1 << (t as u32 >> shift);
                    }
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
            let got = srgb[(v * SRGB_STEPS as f32 + 0.5) as usize] as i32;
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


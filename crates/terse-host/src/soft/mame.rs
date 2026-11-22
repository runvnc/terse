// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// The arithmetic of MAME's vector CRT shaders (vs_beam.sc, fs_beam.sc,
// fs_decay.sc, fs_downsample.sc, fs_blur.sc, fs_composite.sc in
// src/osd/modules/render/bgfx/shaders/chains/vector-crt/) and of the passes
// of src/osd/modules/render/bgfx/vectorrenderer.cpp, commit
// 6f9a2f023e1a951ba34f289d6a4d214a9578676b, ported to the CPU in Rust for
// Tersenet, following the WGSL translation in terse-host/shaders/mame/ step
// for step. The originals are in third_party/mame-vector-crt/.
//
//! MAME's vector CRT look on the CPU, for the software renderer: the same
//! picture the GPU renderer (`mame::gpu`) draws, computed pass for pass as
//! the shaders compute it, in 32-bit floats where the GPU keeps 16:
//!
//! 1. the phosphor (the accumulation) decayed by exp(−dt/τ) (`fs_decay`);
//! 2. each beam added: a padded quad around the segment, at each pixel
//!    centre in it the Gaussian core and halo widened by the pixel's own
//!    variance, round caps (flat at a Tersenet butt end), the scan-order
//!    timing, the intensity (from end to end, Tersenet) (`vs_beam`,
//!    `fs_beam`);
//! 3. the bloom: the phosphor sampled down to half size (four bilinear
//!    samples, `fs_downsample`), two horizontal and vertical passes of
//!    the nine-tap Gaussian (`fs_blur`, clamped at the edges);
//! 4. the composite: phosphor and bloom (sampled up bilinearly), Tersenet's
//!    text light (glyphs drawn as light, not by the beam), exposure, the
//!    luminance tone map 1 − e^−L, gamma 1/2.2 (`fs_composite`), to 8 bits.
//!
//! Work is limited to where light is, as in `phosphor.rs`: per row a mask
//! of the blocks of columns that may hold light, for the phosphor, each
//! bloom target, the text and what the picture showed. A frame whose beams,
//! text and time are those of the frames before, once the phosphor has
//! reached its fixed point, is not drawn: the picture would be the same
//! (as `terse-dev-mame` does).

use super::base::{Pool, pack};
use super::phosphor::{BAND, Blocks, Mask, Text, text_light};
use crate::mame::beams::BeamInstance;
use crate::mame::params::{BLOOM_PASSES, Params};

pub use crate::mame::params::TEXT_GAIN;

/// Light (any channel of the phosphor) below this after the decay is
/// taken as none, so that masks shrink when light goes: it is far below
/// anything the composite shows (light whose luminance is under 10⁻⁴ is
/// black there).
const FLOOR: f32 = 2e-6;

/// The internal resolution's ceiling (as `terse-dev-mame`): a screen
/// taller than this has its phosphor and bloom drawn at this many lines,
/// sampled up in the composite.
pub const MAX_LINES: u32 = 960;

/// How the look is drawn at a shed level (PERFORMANCE.md R15): 0 the whole
/// look within [`MAX_LINES`]; 1 at most 720 lines; 2 at most 480; 3 that
/// without the bloom (as `terse-dev-mame`'s `TubeLook::at`).
pub fn lines_at(level: u8) -> (u32, bool) {
    match level {
        0 => (MAX_LINES, true),
        1 => (720, true),
        2 => (480, true),
        _ => (480, false),
    }
}

/// The phosphor's size for a screen of `size` pixels: the same, or scaled
/// down (the aspect kept) to `max_lines`.
pub fn internal_size(size: (usize, usize), max_lines: u32) -> (usize, usize) {
    let max = max_lines as usize;
    if size.1 <= max {
        return size;
    }
    ((size.0 * max).div_ceil(size.1).max(1), max.max(1))
}

/// Frames of the same beams after which the phosphor is at its fixed point
/// to within 2⁻¹⁶ of its light (as `terse-dev-mame`).
pub fn steady_frames(persistence: f32, dt: f64) -> u32 {
    let k = dt / persistence.max(0.001) as f64;
    if k <= 0.0 || !k.is_finite() {
        return u32::MAX;
    }
    ((16.0 * std::f64::consts::LN_2 / k).ceil() as u32).saturating_add(1)
}

/// e^x to about 2·10⁻⁵ relative (the GPU's `exp` is approximate too): 2^f
/// by its Taylor series on [0, 1), times a power of two.
#[inline(always)]
pub fn exp_fast(x: f32) -> f32 {
    let t = x.clamp(-87.0, 88.0) * std::f32::consts::LOG2_E;
    let i = t.floor();
    let f = t - i;
    let p = 1.0
        + f * (0.693_147_2 + f * (0.240_226_5 + f * (0.055_504_11 + f * (0.009_618_129 + f * (0.001_333_356 + f * 0.000_154_035_3)))));
    f32::from_bits(((i as i32 + 127) << 23) as u32) * p
}

/// A beam ready to draw: `vs_beam`'s quad and the per-segment parts of
/// `fs_beam`, in the phosphor's pixels.
#[derive(Clone, Copy)]
struct Prep {
    p0: [f32; 2],
    dir: [f32; 2],
    nrm: [f32; 2],
    len: f32,
    pad: f32,
    /// Rows and columns the quad may cover.
    rows: (i32, i32),
    cols: (i32, i32),
    rgb: [f32; 3],
    i0: f32,
    i1: f32,
    start: f32,
    duration: f32,
    butt: (bool, bool),
    sigma: f32,
    /// For an intensity the same at both ends: the Gaussians' factors.
    fixed: Option<Profile>,
}

/// `fs_beam`'s Gaussians for an intensity: −½/σ² and the scale of the core
/// and the halo (the halo's strength included), widened by the pixel.
#[derive(Clone, Copy)]
struct Profile {
    core_k: f32,
    core_scale: f32,
    halo_k: f32,
    halo_scale: f32,
}

/// `pixelAcross²/12`: across varies by one pixel per pixel (the quad is in
/// the target's pixels), so a pixel's variance across the beam is 1/12.
const PIXEL_VARIANCE: f32 = 1.0 / 12.0;

impl Profile {
    fn of(base_sigma: f32, intensity: f32, halo: f32) -> Profile {
        let response = intensity.clamp(0.0, 1.0).sqrt();
        let core_sigma = base_sigma * (1.0 + 0.12 * response);
        let halo_sigma = base_sigma * 3.5 * (1.0 + 0.25 * response);
        let halo_strength = halo * (0.6 + 0.4 * response);
        let fc = (core_sigma * core_sigma + PIXEL_VARIANCE).sqrt();
        let fh = (halo_sigma * halo_sigma + PIXEL_VARIANCE).sqrt();
        Profile { core_k: -0.5 / (fc * fc), core_scale: core_sigma / fc, halo_k: -0.5 / (fh * fh), halo_scale: halo_strength * halo_sigma / fh }
    }

    #[inline(always)]
    fn radial(&self, d2: f32) -> f32 {
        self.core_scale * exp_fast(d2 * self.core_k) + self.halo_scale * exp_fast(d2 * self.halo_k)
    }
}

fn prep(b: &BeamInstance, halo: f32) -> Prep {
    let p0 = [b.x0, b.y0];
    let d = [b.x1 - b.x0, b.y1 - b.y0];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let dir = if len > 0.0001 { [d[0] / len, d[1] / len] } else { [1.0, 0.0] };
    let nrm = [-dir[1], dir[0]];
    let sigma = b.sigma.max(0.01);
    let pad = (sigma * 6.0).max(1.0);
    let corners = [
        (p0[0] - dir[0] * pad + nrm[0] * pad, p0[1] - dir[1] * pad + nrm[1] * pad),
        (p0[0] - dir[0] * pad - nrm[0] * pad, p0[1] - dir[1] * pad - nrm[1] * pad),
        (b.x1 + dir[0] * pad + nrm[0] * pad, b.y1 + dir[1] * pad + nrm[1] * pad),
        (b.x1 + dir[0] * pad - nrm[0] * pad, b.y1 + dir[1] * pad - nrm[1] * pad),
    ];
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let fixed = (b.intensity == b.intensity_end).then(|| Profile::of(sigma, b.intensity.max(0.0), halo));
    Prep {
        p0,
        dir,
        nrm,
        len,
        pad,
        rows: ((y0 - 0.5).floor() as i32, (y1 + 0.5).ceil() as i32),
        cols: ((x0 - 0.5).floor() as i32, (x1 + 0.5).ceil() as i32),
        rgb: [b.red, b.green, b.blue],
        i0: b.intensity,
        i1: b.intensity_end,
        start: b.start,
        duration: b.duration,
        butt: (b.butt_start > 0.5, b.butt_end > 0.5),
        sigma,
        fixed,
    }
}

/// The interval of `x` where `lo ≤ k + m·x ≤ hi`, intersected with `iv`.
#[inline]
fn within(iv: (f32, f32), k: f32, m: f32, lo: f32, hi: f32) -> (f32, f32) {
    if m.abs() < 1e-12 {
        return if k >= lo && k <= hi { iv } else { (1.0, 0.0) };
    }
    let (a, b) = ((lo - k) / m, (hi - k) / m);
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    (iv.0.max(a), iv.1.min(b))
}

/// What the beam shader needs of the frame (`u_vector_params`).
#[derive(Clone, Copy)]
struct Frame {
    dt: f32,
    scan_persistence: f32,
    gain: f32,
    halo: f32,
}

/// Adds beam `p`'s light to rows `y0..y0 + rows` of `acc` (`stride`
/// floats a row, `width` pixels), marking the blocks it lit in `masks`.
#[allow(clippy::too_many_arguments)]
fn draw_beam(p: &Prep, f: &Frame, acc: &mut [f32], masks: &mut [Mask], y0: usize, width: usize, blocks: Blocks) {
    let rows = masks.len();
    let stride = width * 3;
    let ya = p.rows.0.max(y0 as i32);
    let yb = p.rows.1.min((y0 + rows) as i32);
    let pad = p.pad;
    for y in ya..yb {
        let cy = y as f32 + 0.5;
        let ry = cy - p.p0[1];
        // along = ra + dir.x · cx, across = rc + nrm.x · cx.
        let ra = ry * p.dir[1] - p.p0[0] * p.dir[0];
        let rc = ry * p.nrm[1] - p.p0[0] * p.nrm[0];
        let iv = within((f32::MIN, f32::MAX), ra, p.dir[0], -pad, p.len + pad);
        let iv = within(iv, rc, p.nrm[0], -pad, pad);
        if iv.0 > iv.1 {
            continue;
        }
        // Pixels whose centre (x + ½) is inside.
        let xa = ((iv.0 - 0.5).ceil() as i32).max(p.cols.0).max(0);
        let xb = ((iv.1 - 0.5).floor() as i32 + 1).min(p.cols.1).min(width as i32);
        if xa >= xb {
            continue;
        }
        let j = y as usize - y0;
        let row = &mut acc[j * stride..(j + 1) * stride];
        let inv_len = if p.len > 0.0001 { 1.0 / p.len } else { 0.0 };
        for x in xa..xb {
            let cx = x as f32 + 0.5;
            let along = ra + p.dir[0] * cx;
            let across = rc + p.nrm[0] * cx;
            if (p.butt.0 && along < 0.0) || (p.butt.1 && along > p.len) {
                continue;
            }
            let seg = if p.len > 0.0001 { (along * inv_len).clamp(0.0, 1.0) } else { 0.5 };
            let intensity = (p.i0 + (p.i1 - p.i0) * seg).max(0.0);
            let profile = match p.fixed {
                Some(pr) => pr,
                None => Profile::of(p.sigma, intensity, f.halo),
            };
            let past = (-along).max(along - p.len).max(0.0);
            let d2 = across * across + past * past;
            let radial = profile.radial(d2);
            let arrival = (p.start + p.duration * seg).clamp(0.0, 1.0);
            let age = (f.dt * (1.0 - arrival)).max(0.0);
            let temporal = exp_fast(-age / f.scan_persistence);
            let e = intensity * radial * temporal * f.gain;
            let o = &mut row[x as usize * 3..x as usize * 3 + 3];
            o[0] += p.rgb[0] * e;
            o[1] += p.rgb[1] * e;
            o[2] += p.rgb[2] * e;
        }
        masks[j] |= blocks.of(xa as usize, xb as usize);
    }
}

/// Where a texture of `n` texels is sampled bilinearly (clamped) for a
/// coordinate `s` in texels (centres at whole numbers): the two texels and
/// the second's weight.
#[inline]
fn bilinear(s: f32, n: usize) -> (u32, u32, f32) {
    let i0 = s.floor();
    let t = s - i0;
    let c = |v: f32| (v.max(0.0) as usize).min(n - 1) as u32;
    (c(i0), c(i0 + 1.0), t)
}

/// `fs_downsample`'s taps along one axis for each of `n_out` texels from
/// `n_in`: four bilinear samples, half a source texel either side of the
/// centre, averaged (each axis contributes ½ of the ¼).
fn down_taps(n_in: usize, n_out: usize) -> Vec<[(u32, f32); 4]> {
    (0..n_out)
        .map(|i| {
            let u = (i as f32 + 0.5) / n_out as f32;
            let off = 0.5 / n_in as f32;
            let mut taps = [(0, 0.0); 4];
            for (k, o) in [-off, off].into_iter().enumerate() {
                let (a, b, t) = bilinear((u + o) * n_in as f32 - 0.5, n_in);
                taps[2 * k] = (a, 0.5 * (1.0 - t));
                taps[2 * k + 1] = (b, 0.5 * t);
            }
            taps
        })
        .collect()
}

/// For each of `n_out` texels, where it samples a texture of `n_in`
/// (`textureSample` of the same coordinate).
fn up_taps(n_in: usize, n_out: usize) -> Vec<(u32, u32, f32)> {
    (0..n_out).map(|i| bilinear((i as f32 + 0.5) / n_out as f32 * n_in as f32 - 0.5, n_in)).collect()
}

/// `fs_blur`'s weights for a σ: w0..w4, already divided by their sum (the
/// ±k taps twice), and how many taps either side matter (the rest are
/// below 10⁻⁷ of the centre's).
fn blur_weights(sigma: f32) -> ([f32; 5], usize) {
    let sigma = sigma.max(0.001);
    let d = 2.0 * sigma * sigma;
    let w: [f32; 5] = std::array::from_fn(|k| (-((k * k) as f32) / d).exp());
    let norm = w[0] + 2.0 * (w[1] + w[2] + w[3] + w[4]);
    let taps = (1..5).rev().find(|&k| w[k] > 1e-7).unwrap_or(0);
    (w.map(|v| v / norm), taps)
}

/// `1/2.2` gamma to 8 bits, by the square root of the value (fine where
/// the curve is steep): entry `i` is `round(255 · (i/N)^(2/2.2))`.
const GAMMA_STEPS: usize = 4096;

fn gamma_table() -> Vec<u8> {
    (0..=GAMMA_STEPS).map(|i| ((i as f32 / GAMMA_STEPS as f32).powf(2.0 / 2.2) * 255.0 + 0.5) as u8).collect()
}

/// `fs_composite`'s tone map of HDR light (exposure applied), to 8-bit
/// (gamma-encoded) channels.
#[inline(always)]
fn tone(hdr: [f32; 3], gamma: &[u8; GAMMA_STEPS + 1]) -> u32 {
    let l = 0.2126 * hdr[0] + 0.7152 * hdr[1] + 0.0722 * hdr[2];
    if l <= 0.0001 {
        return 0;
    }
    let m = (1.0 - exp_fast(-l)) / l;
    let g = |c: f32| gamma[((c * m).clamp(0.0, 1.0).sqrt() * GAMMA_STEPS as f32 + 0.5) as usize];
    pack(g(hdr[0]), g(hdr[1]), g(hdr[2]))
}

/// The look's settings a tube is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub params: Params,
    /// The phosphor's resolution is at most this many lines.
    pub max_lines: u32,
    pub bloom: bool,
    /// The glass's darkening towards the edges (`fs_composite`'s vignette
    /// strength; MAME's default 0).
    pub vignette: f32,
    /// A monochrome tube's colour for the text's light (the beams' colours
    /// are tinted when they are made), or none.
    pub tint: Option<[f32; 3]>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { params: Params::default(), max_lines: MAX_LINES, bloom: true, vignette: 0.0, tint: None }
    }
}

/// One screen's phosphor, its bloom and its text, and the picture they
/// make.
pub struct Tube {
    /// The picture's size, and the phosphor's (internal) size.
    pub out: (usize, usize),
    pub size: (usize, usize),
    pub settings: Settings,
    blocks: Blocks,
    accum: Vec<f32>,
    acc_m: Vec<Mask>,
    half: (usize, usize),
    hblocks: Blocks,
    bloom: [Vec<f32>; 2],
    bloom_m: [Vec<Mask>; 2],
    /// The bloom holds light (else it is all zero).
    bloom_lit: bool,
    oblocks: Blocks,
    text: Vec<f32>,
    text_m: Vec<Mask>,
    shown_m: Vec<Mask>,
    reset: bool,
    /// The last frame's beams, time and text, and how many frames in a
    /// row they were the same.
    key: u64,
    same: u32,
    text_key: Option<u64>,
    down_cols: Vec<[(u32, f32); 4]>,
    down_rows: Vec<[(u32, f32); 4]>,
    bloom_cols: Vec<(u32, u32, f32)>,
    bloom_rows: Vec<(u32, u32, f32)>,
    acc_cols: Vec<(u32, u32, f32)>,
    acc_rows: Vec<(u32, u32, f32)>,
    gamma: Vec<u8>,
    /// Frames drawn and frames found steady (not drawn).
    pub drawn: u64,
    pub steady: u64,
}

/// What one frame is drawn from.
pub struct Light<'a> {
    /// The beams, in the phosphor's pixels (`mame::beams::instances` of
    /// lines made for [`Tube::size`]).
    pub beams: &'a [BeamInstance],
    /// The text's light, and a key that is the same when it is (`None`:
    /// always computed again).
    pub text: &'a Text<'a>,
    pub text_key: Option<u64>,
    pub lin: &'a [f32; 256],
    /// Emulated seconds since the last frame (MAME's frame interval).
    pub dt: f64,
    /// Rectangles of the picture (x0, y0, x1, y1) whose light does not
    /// bloom: the sources of the machine's Surfaces.
    pub excluded: &'a [(usize, usize, usize, usize)],
}

fn hash_beams(beams: &[BeamInstance], dt: f64, text: Option<u64>, excluded: &[(usize, usize, usize, usize)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for b in beams {
        b.bytes().hash(&mut h);
    }
    dt.to_bits().hash(&mut h);
    text.hash(&mut h);
    excluded.hash(&mut h);
    h.finish()
}

impl Tube {
    /// A tube for a picture of `out` pixels, its phosphor at most
    /// `settings.max_lines` high.
    pub fn new(out: (usize, usize), settings: Settings) -> Tube {
        let out = (out.0.max(1), out.1.max(1));
        let size = internal_size(out, settings.max_lines);
        let half = (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1));
        let blocks = Blocks::new(size.0);
        // The half-size blocks cover the same columns of the phosphor as its
        // own (one shift less), so a block's bit means the same place.
        let hblocks = Blocks { shift: blocks.shift - 1, width: half.0 };
        let oblocks = Blocks::new(out.0);
        let n = size.0 * size.1 * 3;
        let hn = half.0 * half.1 * 3;
        Tube {
            out,
            size,
            settings,
            blocks,
            accum: vec![0.0; n],
            acc_m: vec![0; size.1],
            half,
            hblocks,
            bloom: [vec![0.0; hn], vec![0.0; hn]],
            bloom_m: [vec![0; half.1], vec![0; half.1]],
            bloom_lit: false,
            oblocks,
            text: vec![0.0; out.0 * out.1 * 3],
            text_m: vec![0; out.1],
            shown_m: vec![oblocks.all(); out.1],
            reset: true,
            key: 0,
            same: 0,
            text_key: None,
            down_cols: down_taps(size.0, half.0),
            down_rows: down_taps(size.1, half.1),
            bloom_cols: up_taps(half.0, out.0),
            bloom_rows: up_taps(half.1, out.1),
            acc_cols: up_taps(size.0, out.0),
            acc_rows: up_taps(size.1, out.1),
            gamma: gamma_table(),
            drawn: 0,
            steady: 0,
        }
    }

    /// One frame of light: returns false if it was not drawn (the same
    /// beams, text and time as the frames before, the phosphor at its
    /// fixed point: the picture is as it was).
    pub fn frame(&mut self, l: &Light, pool: &Pool) -> bool {
        let p = self.settings.params;
        let text_same = l.text_key.is_some() && l.text_key == self.text_key;
        let key = hash_beams(l.beams, l.dt, l.text_key, l.excluded);
        if key == self.key && text_same && !self.reset {
            self.same = self.same.saturating_add(1);
            if self.same >= steady_frames(p.persistence, l.dt) {
                self.steady += 1;
                return false;
            }
        } else {
            self.same = 0;
            self.key = key;
        }
        self.drawn += 1;
        if !text_same {
            text_light(self.out, self.oblocks, &mut self.text, &mut self.text_m, l.text, l.lin, pool);
            self.text_key = l.text_key;
        }
        // MAME: "Avoid an unusually large elapsed time".
        let dt = l.dt.clamp(0.0, 0.100) as f32;
        let (w, _) = self.size;
        let stride = w * 3;
        let blocks = self.blocks;
        let decay = if self.reset { 0.0 } else { (-dt / p.persistence.max(0.001)).exp() };
        self.reset = false;
        let frame = Frame { dt, scan_persistence: p.persistence.max(dt * 10.0).max(0.001), gain: p.beam_intensity, halo: p.halo };
        let preps: Vec<Prep> = l.beams.iter().map(|b| prep(b, p.halo)).collect();
        // Decay, then the beams, a band of rows at a time.
        let bands: Vec<(usize, &mut [f32], &mut [Mask])> =
            self.accum.chunks_mut(BAND * stride).zip(self.acc_m.chunks_mut(BAND)).enumerate().map(|(i, (a, m))| (i * BAND, a, m)).collect();
        pool.each(bands, |(y0, acc, masks)| {
            for (j, m) in masks.iter_mut().enumerate() {
                if *m == 0 {
                    continue;
                }
                let row = &mut acc[j * stride..(j + 1) * stride];
                let mut keep = 0;
                for (x0, x1) in blocks.runs(*m) {
                    let part = &mut row[x0 * 3..x1 * 3];
                    if decay == 0.0 {
                        part.fill(0.0);
                        continue;
                    }
                    let mut lit = false;
                    for v in part.iter_mut() {
                        let d = *v * decay;
                        *v = if d < FLOOR { 0.0 } else { d };
                        lit |= *v > 0.0;
                    }
                    if lit {
                        keep |= blocks.of(x0, x1);
                    }
                }
                *m = keep;
            }
            let band_rows = (y0 as i32, (y0 + masks.len()) as i32);
            for pr in &preps {
                if pr.rows.1 <= band_rows.0 || pr.rows.0 >= band_rows.1 {
                    continue;
                }
                draw_beam(pr, &frame, acc, masks, y0, w, blocks);
            }
        });
        if self.settings.bloom {
            self.bloom(l.excluded, pool);
        } else if self.bloom_lit {
            for (b, m) in self.bloom.iter_mut().zip(&mut self.bloom_m) {
                b.fill(0.0);
                m.fill(0);
            }
            self.bloom_lit = false;
        }
        true
    }

    /// The bloom: the phosphor down to half size into the first target,
    /// then the blur passes (`vectorrenderer.cpp`'s `composite`).
    fn bloom(&mut self, excluded: &[(usize, usize, usize, usize)], pool: &Pool) {
        let p = self.settings.params;
        let lit = self.acc_m.iter().any(|&m| m != 0);
        if !lit && !self.bloom_lit {
            return;
        }
        self.bloom_lit = lit;
        let (w, h) = self.size;
        let (bw, bh) = self.half;
        let hb = self.hblocks;
        let hstride = bw * 3;
        let stride = w * 3;
        // The excluded rectangles in the phosphor's pixels.
        let (sx, sy) = (w as f32 / self.out.0 as f32, h as f32 / self.out.1 as f32);
        let excl: Vec<(usize, usize, usize, usize)> = excluded
            .iter()
            .map(|&(x0, y0, x1, y1)| ((x0 as f32 * sx) as usize, (y0 as f32 * sy) as usize, ((x1 as f32 * sx).ceil() as usize).min(w), ((y1 as f32 * sy).ceil() as usize).min(h)))
            .collect();
        // Downsample.
        {
            let (accum, acc_m, down_cols, down_rows) = (&self.accum, &self.acc_m, &self.down_cols, &self.down_rows);
            let excl = &excl;
            let [b0, _] = &mut self.bloom;
            let [m0, _] = &mut self.bloom_m;
            let bands: Vec<(usize, &mut [f32], &mut [Mask])> = b0.chunks_mut(BAND * hstride).zip(m0.chunks_mut(BAND)).enumerate().map(|(i, (b, m))| (i * BAND, b, m)).collect();
            pool.each(bands, |(y0, rows, masks)| {
                let mut tmp = vec![0.0f32; stride];
                for (j, m) in masks.iter_mut().enumerate() {
                    let y = y0 + j;
                    let taps = &down_rows[y];
                    let src: Mask = taps.iter().fold(0, |a, &(r, _)| a | acc_m[r as usize]);
                    let new = if src == 0 { 0 } else { hb.dilate(src, 1) };
                    let out = &mut rows[j * hstride..(j + 1) * hstride];
                    for (x0, x1) in hb.runs(*m & !new) {
                        out[x0 * 3..x1 * 3].fill(0.0);
                    }
                    *m = new;
                    for (x0, x1) in hb.runs(new) {
                        // The source columns these half columns read.
                        let (c0, c1) = (down_cols[x0][0].0 as usize, (down_cols[x1 - 1][3].0 as usize + 1).min(w));
                        // The rows' weighted sum, then the columns'.
                        tmp[c0 * 3..c1 * 3].fill(0.0);
                        for &(r, wt) in taps {
                            if wt == 0.0 || acc_m[r as usize] == 0 {
                                continue;
                            }
                            let r = r as usize;
                            let src = &accum[r * stride + c0 * 3..r * stride + c1 * 3];
                            let zero_x = excl.iter().filter(|e| r >= e.1 && r < e.3 && e.0 < c1 && e.2 > c0);
                            for (t, s) in tmp[c0 * 3..c1 * 3].iter_mut().zip(src) {
                                *t += wt * s;
                            }
                            // Light in a Surface's source does not bloom.
                            for e in zero_x {
                                let (a, z) = (e.0.max(c0), e.2.min(c1));
                                for x in a..z {
                                    for ch in 0..3 {
                                        tmp[x * 3 + ch] -= wt * accum[r * stride + x * 3 + ch];
                                    }
                                }
                            }
                        }
                        for x in x0..x1 {
                            let mut acc = [0.0f32; 3];
                            for &(c, wt) in &down_cols[x] {
                                let c = c as usize * 3;
                                acc[0] += wt * tmp[c];
                                acc[1] += wt * tmp[c + 1];
                                acc[2] += wt * tmp[c + 2];
                            }
                            out[x * 3..x * 3 + 3].copy_from_slice(&acc);
                        }
                    }
                }
            });
        }
        // "Scale the output-pixel bloom radius relative to a 1080-line reference."
        let bloom_scale = (h as f32 / 1080.0).clamp(0.25, 2.0);
        let radius = p.bloom_radius * bloom_scale;
        let pass_scale = 1.0 / (BLOOM_PASSES as f32).sqrt();
        let (wx, tx) = blur_weights(radius * bw as f32 / w as f32 * pass_scale);
        let (wy, ty) = blur_weights(radius * bh as f32 / h as f32 * pass_scale);
        for _ in 0..BLOOM_PASSES {
            // Horizontal: bloom 0 into bloom 1.
            {
                let [b0, b1] = &mut self.bloom;
                let [m0, m1] = &mut self.bloom_m;
                let (b0, m0) = (&*b0, &*m0);
                let bands: Vec<(usize, &mut [f32], &mut [Mask])> = b1.chunks_mut(BAND * hstride).zip(m1.chunks_mut(BAND)).enumerate().map(|(i, (b, m))| (i * BAND, b, m)).collect();
                pool.each(bands, |(y0, rows, masks)| {
                    for (j, m) in masks.iter_mut().enumerate() {
                        let y = y0 + j;
                        let src_m = m0[y];
                        let new = if src_m == 0 { 0 } else { hb.dilate(src_m, 1) };
                        let out = &mut rows[j * hstride..(j + 1) * hstride];
                        for (x0, x1) in hb.runs(*m & !new) {
                            out[x0 * 3..x1 * 3].fill(0.0);
                        }
                        *m = new;
                        let src = &b0[y * hstride..(y + 1) * hstride];
                        for (x0, x1) in hb.runs(new) {
                            for x in x0..x1 {
                                let mut acc = [0.0f32; 3];
                                for k in -(tx as i32)..=tx as i32 {
                                    let sx = (x as i32 + k).clamp(0, bw as i32 - 1) as usize * 3;
                                    let wt = wx[k.unsigned_abs() as usize];
                                    acc[0] += wt * src[sx];
                                    acc[1] += wt * src[sx + 1];
                                    acc[2] += wt * src[sx + 2];
                                }
                                out[x * 3..x * 3 + 3].copy_from_slice(&acc);
                            }
                        }
                    }
                });
            }
            // Vertical: bloom 1 into bloom 0.
            {
                let [b0, b1] = &mut self.bloom;
                let [m0, m1] = &mut self.bloom_m;
                let (b1, m1) = (&*b1, &*m1);
                let bands: Vec<(usize, &mut [f32], &mut [Mask])> = b0.chunks_mut(BAND * hstride).zip(m0.chunks_mut(BAND)).enumerate().map(|(i, (b, m))| (i * BAND, b, m)).collect();
                pool.each(bands, |(y0, rows, masks)| {
                    for (j, m) in masks.iter_mut().enumerate() {
                        let y = y0 + j;
                        let (ra, rb) = (y.saturating_sub(ty), (y + ty + 1).min(bh));
                        let mut new: Mask = (ra..rb).fold(0, |a, r| a | m1[r]);
                        // Clamped at the edges: the edge row stands for the rows beyond.
                        if y < ty {
                            new |= m1[0];
                        }
                        if y + ty >= bh {
                            new |= m1[bh - 1];
                        }
                        let out = &mut rows[j * hstride..(j + 1) * hstride];
                        for (x0, x1) in hb.runs(*m & !new) {
                            out[x0 * 3..x1 * 3].fill(0.0);
                        }
                        *m = new;
                        for (x0, x1) in hb.runs(new) {
                            let o = &mut out[x0 * 3..x1 * 3];
                            o.fill(0.0);
                            for k in -(ty as i32)..=ty as i32 {
                                let sy = (y as i32 + k).clamp(0, bh as i32 - 1) as usize;
                                if m1[sy] & hb.of(x0, x1) == 0 {
                                    continue;
                                }
                                let wt = wy[k.unsigned_abs() as usize];
                                let src = &b1[sy * hstride + x0 * 3..sy * hstride + x1 * 3];
                                for (o, s) in o.iter_mut().zip(src) {
                                    *o += wt * s;
                                }
                            }
                        }
                    }
                });
            }
        }
    }

    /// Writes the picture (what the last frame left) into `image`, whose
    /// rows are `stride` pixels and which shows the picture's top left at
    /// (`ox`, `oy`); only rows `rows`, and only where light is or was.
    #[allow(clippy::too_many_arguments)]
    pub fn composite(&mut self, image: &mut [u32], stride: usize, ox: usize, oy: usize, rows: std::ops::Range<usize>, pool: &Pool) {
        let (ow, oh) = self.out;
        let rows = rows.start.min(oh)..rows.end.min(oh);
        if rows.is_empty() {
            return;
        }
        let (w, h) = self.size;
        let (bw, _) = self.half;
        let same = (w, h) == (ow, oh);
        let s = self.settings;
        let strength = if s.bloom { s.params.bloom_strength } else { 0.0 };
        let exposure = s.params.exposure;
        let (blocks, hb, ob) = (self.blocks, self.hblocks, self.oblocks);
        let (accum, acc_m, text, text_m) = (&self.accum, &self.acc_m, &self.text, &self.text_m);
        let (bloom, bloom_m) = (&self.bloom[0], &self.bloom_m[0]);
        let bloom_lit = s.bloom && self.bloom_lit;
        let (acc_cols, acc_rows, bloom_cols, bloom_rows) = (&self.acc_cols, &self.acc_rows, &self.bloom_cols, &self.bloom_rows);
        let gamma: &[u8; GAMMA_STEPS + 1] = self.gamma[..].try_into().expect("the table");
        let (astride, hstride, tstride) = (w * 3, bw * 3, ow * 3);
        // A mask of the phosphor's or the bloom's blocks in the picture's.
        let scale_mask = |m: Mask, b: Blocks, n: usize| -> Mask {
            if same && b.shift == blocks.shift {
                return m;
            }
            let k = ow as f32 / n as f32;
            b.runs(m).fold(0, |a, (x0, x1)| a | ob.of(((x0 as f32 * k) as usize).saturating_sub(2), (((x1 as f32 * k).ceil() as usize) + 2).min(ow)))
        };
        let vignette = s.vignette;
        let tint = s.tint;
        let first = (oy + rows.start) * stride;
        let image = &mut image[first..first + rows.len() * stride];
        let shown = &mut self.shown_m[rows.clone()];
        let bands: Vec<(usize, &mut [u32], &mut [Mask])> =
            image.chunks_mut(BAND * stride).zip(shown.chunks_mut(BAND)).enumerate().map(|(i, (img, sh))| (rows.start + i * BAND, img, sh)).collect();
        pool.each(bands, |(y0, img, shown)| {
            for j in 0..shown.len() {
                let y = y0 + j;
                let (a0, a1, ay) = if same { (y as u32, y as u32, 0.0) } else { acc_rows[y] };
                let am = acc_m[a0 as usize] | acc_m[a1 as usize];
                let (b0, b1, by) = bloom_rows[y];
                let bm = if bloom_lit { bloom_m[b0 as usize] | bloom_m[b1 as usize] } else { 0 };
                let now = scale_mask(am, blocks, w) | scale_mask(bm, hb, bw) | text_m[y];
                let out = &mut img[j * stride + ox..j * stride + ox + ow];
                for (x0, x1) in ob.runs(shown[j] & !now) {
                    out[x0..x1].fill(0);
                }
                shown[j] = now;
                if now == 0 {
                    continue;
                }
                let (ar0, ar1) = (&accum[a0 as usize * astride..][..astride], &accum[a1 as usize * astride..][..astride]);
                let (br0, br1) = (&bloom[b0 as usize * hstride..][..hstride], &bloom[b1 as usize * hstride..][..hstride]);
                let trow = &text[y * tstride..(y + 1) * tstride];
                let gy = 2.0 * (y as f32 + 0.5) / oh as f32 - 1.0;
                for (x0, x1) in ob.runs(now) {
                    for x in x0..x1 {
                        let mut c = if same {
                            [ar0[x * 3], ar0[x * 3 + 1], ar0[x * 3 + 2]]
                        } else {
                            let (c0, c1, t) = acc_cols[x];
                            let (c0, c1) = (c0 as usize * 3, c1 as usize * 3);
                            std::array::from_fn(|k| {
                                let top = ar0[c0 + k] + (ar0[c1 + k] - ar0[c0 + k]) * t;
                                let bottom = ar1[c0 + k] + (ar1[c1 + k] - ar1[c0 + k]) * t;
                                top + (bottom - top) * ay
                            })
                        };
                        if bm != 0 {
                            let (c0, c1, t) = bloom_cols[x];
                            let (c0, c1) = (c0 as usize * 3, c1 as usize * 3);
                            for k in 0..3 {
                                let top = br0[c0 + k] + (br0[c1 + k] - br0[c0 + k]) * t;
                                let bottom = br1[c0 + k] + (br1[c1 + k] - br1[c0 + k]) * t;
                                c[k] += (top + (bottom - top) * by) * strength;
                            }
                        }
                        let t = &trow[x * 3..x * 3 + 3];
                        let t = match tint {
                            Some(tn) => crate::phosphor::tinted([t[0], t[1], t[2]], tn),
                            None => [t[0], t[1], t[2]],
                        };
                        let hdr = [(c[0] + t[0] * TEXT_GAIN) * exposure, (c[1] + t[1] * TEXT_GAIN) * exposure, (c[2] + t[2] * TEXT_GAIN) * exposure];
                        if vignette > 0.0 {
                            // fs_composite: mix(1, clamp(16 · u(1 − u) · v(1 − v)), w), on
                            // the tone-mapped light; applied before it here only
                            // to its luminance's share, so after.
                            let u = (x as f32 + 0.5) / ow as f32;
                            let v = (gy + 1.0) * 0.5;
                            let g = (16.0 * u * (1.0 - u) * v * (1.0 - v)).clamp(0.0, 1.0);
                            let f = 1.0 + (g - 1.0) * vignette;
                            let px = tone(hdr, gamma);
                            let sc = |c: u32| ((c as f32 * f) + 0.5) as u8;
                            out[x] = pack(sc(px >> 16 & 255), sc(px >> 8 & 255), sc(px & 255));
                            continue;
                        }
                        out[x] = tone(hdr, gamma);
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

    /// Whether the phosphor or the text holds any light.
    pub fn lit(&self) -> bool {
        self.acc_m.iter().chain(&self.text_m).any(|&m| m != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mame::beams::{Line, Screen, instances};

    #[test]
    fn exp_is_close() {
        for i in -1740..200 {
            let x = i as f32 * 0.05;
            let (a, b) = (exp_fast(x), x.exp());
            assert!((a - b).abs() <= 3e-5 * b + 1e-38, "{x}: {a} {b}");
        }
        assert_eq!(exp_fast(-200.0), exp_fast(-87.0));
    }

    #[test]
    fn taps_and_weights() {
        // An even size: the four samples are the 2 × 2 texels.
        let t = down_taps(8, 4);
        assert_eq!(t[1], [(2, 0.5), (3, 0.0), (3, 0.5), (4, 0.0)]);
        let (w, taps) = blur_weights(0.33);
        assert!((w[0] + 2.0 * w[1..].iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(taps <= 2, "{taps}");
        assert_eq!(blur_weights(3.0).1, 4);
        assert_eq!(up_taps(4, 8)[0], (0, 0, 0.75));
        assert_eq!(internal_size((1920, 1440), 960), (1280, 960));
        assert_eq!(internal_size((1333, 1000), 960), (1280, 960));
        assert_eq!(steady_frames(0.002, 1.0 / 60.0), 3);
    }

    /// The beam at its centre, by the shader's formula, against a pixel of
    /// a drawn line.
    #[test]
    fn a_beam_is_fs_beam() {
        let s = Screen { size: (640, 480) };
        // A horizontal line through pixel centres (y = 10.5 units).
        let l = s.line(16.0 * 10.0, 16.0 * 10.5, 16.0 * 50.0, 16.0 * 10.5, 16.0, [255, 255, 255, 255], 1.0);
        let i = instances(&[l], 0.75);
        let mut t = Tube::new((640, 480), Settings { bloom: false, ..Settings::default() });
        let pool = Pool::new(1);
        let grid = Text::Layers(&super::super::phosphor::LightLayers { layers: vec![], width: 640, height: 480, origin: (0, 0), extents: vec![] });
        let lin = super::super::phosphor::linear_table();
        assert!(t.frame(&Light { beams: &i, text: &grid, text_key: Some(0), lin: &lin, dt: 1.0 / 60.0, excluded: &[] }, &pool));
        // The middle of the line: across 0, half way through the scan.
        let at = (10 * 640 + 30) * 3;
        let sigma = 0.75 * 0.085;
        let core = sigma * 1.12;
        let halo = sigma * 3.5 * 1.25;
        let fc = (core * core + 1.0 / 12.0f32).sqrt();
        let fh = (halo * halo + 1.0 / 12.0f32).sqrt();
        let radial = core / fc + 0.04 * halo / fh;
        let temporal = (-(1.0 / 60.0) * (1.0 - 20.5 / 40.0f32) / (10.0 / 60.0)).exp();
        let want = radial * temporal * 4.0;
        assert!((t.accum[at] - want).abs() < 1e-3 * want, "{} {want}", t.accum[at]);
        // One row off: across 1 pixel.
        let off = t.accum[at + 640 * 3];
        let want_off = (core / fc * (-0.5 / (fc * fc)).exp() + 0.04 * halo / fh * (-0.5 / (fh * fh)).exp()) * temporal * 4.0;
        assert!((off - want_off).abs() < 1e-3 * want_off.max(1e-6) + 1e-7, "{off} {want_off}");
        // The same frame again is drawn until the phosphor is steady, then not.
        let light = Light { beams: &i, text: &grid, text_key: Some(0), lin: &lin, dt: 1.0 / 60.0, excluded: &[] };
        let drawn: Vec<bool> = (0..5).map(|_| t.frame(&light, &pool)).collect();
        assert_eq!(drawn, [true, true, false, false, false]);
    }

    /// A segment that fades: its light follows its intensity along it; a
    /// butt end has no cap.
    #[test]
    fn fades_and_butt_ends() {
        let s = Screen { size: (640, 480) };
        let mut l: Line = s.line(16.0 * 10.0, 16.0 * 20.5, 16.0 * 110.0, 16.0 * 20.5, 16.0, [255, 255, 255, 255], 1.0);
        l.intensity_end = 0.0;
        l.butt = 2;
        let i = instances(&[l], 0.75);
        let mut t = Tube::new((640, 480), Settings { bloom: false, ..Settings::default() });
        let pool = Pool::new(2);
        let grid = Text::Layers(&super::super::phosphor::LightLayers { layers: vec![], width: 640, height: 480, origin: (0, 0), extents: vec![] });
        let lin = super::super::phosphor::linear_table();
        t.frame(&Light { beams: &i, text: &grid, text_key: Some(0), lin: &lin, dt: 1.0 / 60.0, excluded: &[] }, &pool);
        let v = |x: usize| t.accum[(20 * 640 + x) * 3];
        assert!(v(20) > v(60) && v(60) > v(100) && v(100) > 0.0, "{} {} {}", v(20), v(60), v(100));
        // The start's round cap; nothing past the butt end.
        assert!(v(9) > 0.0 && v(111) == 0.0, "{} {}", v(9), v(111));
    }
}

//! The phosphor presentation (SPEC 10.6): every tunable constant in one
//! place, the CPU side of the afterglow (motion sub-steps), and the WGSL
//! prelude that hands the constants to the shaders.
//!
//! The whole screen is one tube. Each machine frame has two kinds of light,
//! both linear RGB in float textures the size of the screen:
//!
//! - the *beam*: this frame's glyphs, cursor and vector shapes, drawn fresh
//!   at full brightness (never accumulated);
//! - the *afterglow*, in two parts with their own decay: a fast component
//!   that falls to about 10% within three frames, and a slow, dim tail whose
//!   per-frame factor is persistence / 256. Each frame both are multiplied
//!   by their factor, minus [`EPSILON`] so they really reach black, and then
//!   the summed beam is deposited into them: `afterglow = max(afterglow ×
//!   decay, beam × deposit)`. Depositing with `max` rather than `+` is the
//!   normalisation: a shape that stays put holds its afterglow at exactly
//!   `beam × DEPOSIT`, so it shows at `beam × (1 + DEPOSIT)` = 1.2 × and never
//!   brightens over time, while a trail left behind starts at `DEPOSIT` of
//!   the beam and fades from there. The beam is the sum of every stroke and
//!   glyph, so a line drawn twice leaves a trail twice as bright.
//!
//! The beam is additive: each stroke adds a sharp core ([`CORE_WIDTH`] of
//! its width) and a tight halo, a little more at its end points ([`DWELL`]),
//! so repeated and crossing lines add up. Displayed light is `beam +
//! afterglow + bloom`, where the bloom is all of that light blurred
//! ([`BLOOM`]), through a tone curve that is linear up to [`TONE_KNEE`] and
//! then has a long shoulder towards 1, leaving headroom for repetitions.
//! Only light above [`SPILL_FROM`] (a line drawn three or more times, bright
//! crossings) spills into the other channels and turns white-hot.
//!
//! A moving shape would leave a row of separate copies, one per frame. When
//! a frame's list has the same opcode sequence as the previous frame's
//! (the same shapes, only moved), the afterglow also takes deposits from
//! [`SUBSTEPS`] (more for a fast shape, up to [`MAX_SUBSTEPS`]) positions
//! interpolated between the two frames, each already decayed by the
//! fraction of a frame since it was "drawn", so the trail is a continuous
//! smear. The beam itself is only ever at the current position.

use terse_core::Primitive;

/// Width of the bright core as a share of the stated line width. At the
/// default width of 1 screen unit the core is 0.6 units; it only gets past
/// 1.2 units when the width asks for more than 2.
pub const CORE_WIDTH: f32 = 0.6;
/// Brightness of the core (linear light, times the colour and opacity).
/// Beam light adds up linearly before the tone curve, so a line drawn twice
/// (or two lines crossing) is twice the light: see [`TONE_KNEE`].
pub const CORE: f32 = 1.2;
/// Peak brightness of each stroke's own tight halo, at the core's edge,
/// relative to the core. (The wider glow is [`BLOOM`], over the whole screen.)
pub const HALO: f32 = 0.14;
/// The halo's exponential fall-off length, in sixteenths of a screen unit.
pub const HALO_FALLOFF: f32 = 7.0;
/// How far past the core's edge the halo reaches (it is smoothed to zero
/// over the outer half), in sixteenths: 1.5 screen units.
pub const HALO_REACH: f32 = 24.0;

/// Vertex dwell: the beam lingers where a stroke starts and ends, so the
/// light at an end point is 1 + DWELL times the stroke's. Where several
/// different lines meet (a polygon's corner), their ends share it: each
/// gives (1 + DWELL) / n there, so a corner is 1 + DWELL times one line, not
/// n times. Identical lines drawn again count as one line for this, so a
/// line drawn twice keeps twice the light at its corners too.
pub const DWELL: f32 = 0.5;
/// Radius of the dwell spot (a Gaussian) as a share of the core's half
/// width, and its least radius in pixels so it shows at any size.
pub const DWELL_SIZE: f32 = 1.6;
pub const DWELL_MIN_PX: f32 = 0.7;

/// Brightness of glyphs and the cursor (times the foreground colour).
pub const GLYPH: f32 = 1.1;
/// The tight halo around glyphs, like a stroke's [`HALO`]: the average of
/// eight samples at [`GLYPH_BLOOM_RADIUS`] screen units around each pixel,
/// times this. Glyphs also get the screen's [`BLOOM`].
pub const GLYPH_BLOOM: f32 = 0.12;
pub const GLYPH_BLOOM_RADIUS: f32 = 1.0;

/// The bloom: all light on the screen (beam and afterglow, glyphs and
/// shapes alike) blurred by a Gaussian of [`BLOOM_SIGMA`] screen units and
/// added back times this. It is linear in the light, so a brighter or
/// overdrawn line has a brighter bloom that stays visible further out.
pub const BLOOM: f32 = 0.55;
pub const BLOOM_SIGMA: f32 = 1.4;
/// Most taps on each side of a blur pass (a very large window blurs less).
pub const BLOOM_MAX_TAPS: f32 = 40.0;
/// Light above this (in the brightest channel; one static stroke's core is
/// [`CORE`] × (1 + [`DEPOSIT`]) = 1.44) whitens the bloom: the excess times
/// [`BLOOM_WHITE`] is added to every channel before blurring, so a doubled
/// line's glow is a little whiter than a single one's.
pub const BLOOM_WHITE_FROM: f32 = 1.0;
pub const BLOOM_WHITE: f32 = 0.35;

/// Share of the beam the afterglow keeps (both components together). It is
/// taken from the summed beam, so an overdrawn line leaves a brighter trail.
pub const DEPOSIT: f32 = 0.2;
/// The fast component's share of the deposit; the slow tail has the rest.
pub const FAST_SHARE: f32 = 0.6;
/// Per-frame factor of the fast component: 0.46³ ≈ 0.1.
pub const FAST_DECAY: f32 = 0.46;
/// Subtracted from both components every frame, after the decay, so tiny
/// values reach zero instead of lingering as a ghost.
pub const EPSILON: f32 = 0.0015;

/// Interpolated positions a moving shape deposits its afterglow from (the
/// last is the current position): at least [`SUBSTEPS`], and one per
/// [`SMEAR_SPACING`] sixteenths moved, up to [`MAX_SUBSTEPS`] (only
/// [`SUBSTEPS`] if the list is so long that the deposits would not fit
/// [`DEPOSIT_BUDGET`]).
pub const SUBSTEPS: u32 = 4;
pub const MAX_SUBSTEPS: u32 = 16;
pub const SMEAR_SPACING: i32 = 16;
/// Most deposit instances in one frame.
pub const DEPOSIT_BUDGET: usize = crate::display::MAX_SHAPES * SUBSTEPS as usize;
/// A shape that jumped further than this between frames (in sixteenths of
/// a screen unit, on either axis: 40 units) is not smeared; it teleported.
pub const MAX_SMEAR: i32 = 40 * 16;

/// Tone curve: linear light up to the knee, then a long shoulder,
/// `knee + (1 - knee) × u / (1 + u)` with `u = (x - knee) / (1 - knee)`,
/// which approaches 1 slowly. One static white stroke (about 1.44) shows
/// at about 225 in sRGB, two at 240, four at 247: each repetition is
/// brighter, and its bloom (linear) much more so.
pub const TONE_KNEE: f32 = 0.35;
/// Light above this (in the brightest channel) spills into all channels,
/// times [`SPILL`]: a saturated colour turns white only from about three
/// times one static stroke's light (crossings of bright lines, a line drawn
/// three times or more), not at a single repetition (2 × 1.44 = 2.88).
pub const SPILL_FROM: f32 = 3.0;
pub const SPILL: f32 = 0.5;

/// The slow component's per-frame factor for a persistence (SPEC 10.6).
pub fn slow_decay(persistence: u8) -> f32 {
    persistence as f32 / 256.0
}

/// The constants as WGSL declarations, prepended to the shaders.
#[cfg(feature = "gpu")]
pub fn wgsl() -> String {
    let consts = [
        ("CORE_WIDTH", CORE_WIDTH),
        ("CORE", CORE),
        ("HALO", HALO),
        ("HALO_FALLOFF", HALO_FALLOFF),
        ("HALO_REACH", HALO_REACH),
        ("DWELL", DWELL),
        ("DWELL_SIZE", DWELL_SIZE),
        ("DWELL_MIN_PX", DWELL_MIN_PX),
        ("GLYPH", GLYPH),
        ("GLYPH_BLOOM", GLYPH_BLOOM),
        ("GLYPH_BLOOM_RADIUS", GLYPH_BLOOM_RADIUS),
        ("BLOOM", BLOOM),
        ("BLOOM_SIGMA", BLOOM_SIGMA),
        ("BLOOM_MAX_TAPS", BLOOM_MAX_TAPS),
        ("BLOOM_WHITE_FROM", BLOOM_WHITE_FROM),
        ("BLOOM_WHITE", BLOOM_WHITE),
        ("DEPOSIT", DEPOSIT),
        ("FAST_SHARE", FAST_SHARE),
        ("EPSILON", EPSILON),
        ("TONE_KNEE", TONE_KNEE),
        ("SPILL_FROM", SPILL_FROM),
        ("SPILL", SPILL),
    ];
    consts.iter().map(|(name, v)| format!("const {name}: f32 = {v:?};\n")).collect()
}

/// Tone curve (the shaders have the same; this one is for tests).
#[cfg(test)]
pub fn tone(x: f32) -> f32 {
    if x <= TONE_KNEE {
        x
    } else {
        let u = (x - TONE_KNEE) / (1.0 - TONE_KNEE);
        TONE_KNEE + (1.0 - TONE_KNEE) * u / (1.0 + u)
    }
}

/// The opcodes of a display list in order (SPEC 10.3), operands skipped:
/// two lists with the same sequence draw the same shapes, maybe moved.
pub fn opcodes(list: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&op) = list.get(at) {
        let n = match op {
            0x01 | 0x03 | 0x04 | 0x05 | 0x08 => 4,
            0x02 => 1,
            0x06 | 0x07 => 2,
            0x09 => 8,
            _ => break,
        };
        if at + 1 + n > list.len() {
            break;
        }
        out.push(op);
        at += 1 + n;
    }
    out
}

/// One afterglow deposit of a moving shape at an earlier position: the
/// shape (a line or circle), the index of the shape it comes from, and the
/// light it adds to the fast and slow components, per unit of beam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deposit {
    pub shape: Primitive,
    pub index: usize,
    pub fast: f32,
    pub slow: f32,
}

fn lerp(a: i16, b: i16, t: f32) -> i16 {
    (a as f32 + (b as f32 - a as f32) * t).round() as i16
}

/// How far `cur` moved from `prev` (sixteenths, the largest change of any
/// coordinate), or `None` if they are not the same kind of shape.
fn moved(prev: &Primitive, cur: &Primitive) -> Option<i32> {
    let d = |pairs: &[(i32, i32)]| pairs.iter().map(|&(a, b)| (a - b).abs()).max().unwrap_or(0);
    match (*prev, *cur) {
        (Primitive::Line { x0: a0, y0: b0, x1: a1, y1: b1, .. }, Primitive::Line { x0, y0, x1, y1, .. }) => Some(d(&[
            (a0 as i32, x0 as i32),
            (b0 as i32, y0 as i32),
            (a1 as i32, x1 as i32),
            (b1 as i32, y1 as i32),
        ])),
        (Primitive::Circle { x: a, y: b, radius: r, .. }, Primitive::Circle { x, y, radius, .. }) => {
            Some(d(&[(a as i32, x as i32), (b as i32, y as i32), (r as i32, radius as i32)]))
        }
        (Primitive::Beam { x0: a0, y0: b0, x1: a1, y1: b1, .. }, Primitive::Beam { x0, y0, x1, y1, .. }) => Some(d(&[
            (a0 as i32, x0 as i32),
            (b0 as i32, y0 as i32),
            (a1 as i32, x1 as i32),
            (b1 as i32, y1 as i32),
        ])),
        _ => None,
    }
}

/// `cur` moved back towards `prev` to time `t` (0 = `prev`, 1 = `cur`), or
/// `None` if they are not the same kind of shape or it jumped too far.
fn between(prev: &Primitive, cur: &Primitive, t: f32) -> Option<Primitive> {
    let near = |pairs: &[(i16, i16)]| pairs.iter().all(|&(a, b)| (a as i32 - b as i32).abs() <= MAX_SMEAR);
    match (*prev, *cur) {
        (Primitive::Line { x0: a0, y0: b0, x1: a1, y1: b1, .. }, Primitive::Line { x0, y0, x1, y1, width, rgba }) => {
            near(&[(a0, x0), (b0, y0), (a1, x1), (b1, y1)]).then(|| Primitive::Line {
                x0: lerp(a0, x0, t),
                y0: lerp(b0, y0, t),
                x1: lerp(a1, x1, t),
                y1: lerp(b1, y1, t),
                width,
                rgba,
            })
        }
        (Primitive::Circle { x: a, y: b, radius: r, .. }, Primitive::Circle { x, y, radius, width, rgba }) => {
            near(&[(a, x), (b, y)]).then(|| Primitive::Circle {
                x: lerp(a, x, t),
                y: lerp(b, y, t),
                radius: (r as f32 + (radius as f32 - r as f32) * t).round() as u16,
                width,
                rgba,
            })
        }
        (Primitive::Beam { x0: a0, y0: b0, x1: a1, y1: b1, .. }, Primitive::Beam { x0, y0, x1, y1, width, rgba, fade, butt }) => {
            near(&[(a0, x0), (b0, y0), (a1, x1), (b1, y1)]).then(|| Primitive::Beam {
                x0: lerp(a0, x0, t),
                y0: lerp(b0, y0, t),
                x1: lerp(a1, x1, t),
                y1: lerp(b1, y1, t),
                width,
                rgba,
                fade,
                butt,
            })
        }
        _ => None,
    }
}

/// The afterglow deposits of one machine frame besides the current beam
/// (which the renderer deposits whole, summed): when `smear` (same opcodes
/// as the last frame, so `prev` lines up with `cur`), each moving shape at
/// the [`SUBSTEPS`] − 1 or more positions from just after `prev` up to (not
/// including) `cur`, already decayed for the part of a frame since.
/// `slow_decay` is the slow component's per-frame factor.
pub fn deposits(prev: &[Primitive], cur: &[Primitive], smear: bool, slow_decay: f32, out: &mut Vec<Deposit>) {
    out.clear();
    let smear = smear && prev.len() == cur.len() && prev != cur;
    if !smear {
        return;
    }
    let (fast, slow) = (DEPOSIT * FAST_SHARE, DEPOSIT * (1.0 - FAST_SHARE));
    let most = if cur.len() * MAX_SUBSTEPS as usize <= DEPOSIT_BUDGET { MAX_SUBSTEPS } else { SUBSTEPS };
    for (i, shape) in cur.iter().enumerate() {
        if let Some(d) = moved(&prev[i], shape).filter(|&d| d > 0) {
            let steps = ((d as u32).div_ceil(SMEAR_SPACING as u32)).clamp(SUBSTEPS, most);
            for s in 1..steps {
                let t = s as f32 / steps as f32;
                let Some(p) = between(&prev[i], shape, t) else { break };
                let age = 1.0 - t;
                out.push(Deposit { shape: p, index: i, fast: fast * FAST_DECAY.powf(age), slow: slow * slow_decay.powf(age) });
            }
        }
    }
}

/// The vertex dwell of each shape ([`DWELL`]): for a line (or a 3D
/// segment), how many different lines end at its start (bits 0-3) and at
/// its end (bits 4-7), itself included, at most 15; 0 for anything else.
/// Lines with the same two end points count once, whatever their colour or
/// width; a line of length 0 is one end. A segment's butt end (where a
/// circle's chords join, or where a face cut it) has no dwell: it is not
/// counted and its 4 bits are 0, and a segment both of whose ends are butt
/// ends gets 0.
pub fn dwell_ends(shapes: &[Primitive], out: &mut Vec<u32>) {
    use std::collections::{HashMap, HashSet};
    use terse_core::vector3::{BUTT_END, BUTT_START};
    out.clear();
    // The two ends and whether each has dwell.
    let ends = |p: &Primitive| match *p {
        Primitive::Line { x0, y0, x1, y1, .. } => Some(((x0, y0), (x1, y1), true, true)),
        Primitive::Beam { x0, y0, x1, y1, butt, .. } if butt & (BUTT_START | BUTT_END) != BUTT_START | BUTT_END => {
            Some(((x0, y0), (x1, y1), butt & BUTT_START == 0, butt & BUTT_END == 0))
        }
        _ => None,
    };
    let mut lines = HashSet::new();
    let mut at: HashMap<(i16, i16), u32> = HashMap::new();
    for (a, b, da, db) in shapes.iter().filter_map(ends) {
        if lines.insert(if a <= b { (a, b, da, db) } else { (b, a, db, da) }) {
            if da {
                *at.entry(a).or_default() += 1;
            }
            if db && (b != a || !da) {
                *at.entry(b).or_default() += 1;
            }
        }
    }
    out.extend(shapes.iter().map(|p| match ends(p) {
        Some((a, b, da, db)) => (if da { at[&a].min(15) } else { 0 }) | (if db { at[&b].min(15) } else { 0 }) << 4,
        None => 0,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: [u8; 4] = [255; 4];

    fn line(x: i16) -> Primitive {
        Primitive::Line { x0: x, y0: 0, x1: x + 16, y1: 0, width: 16, rgba: C }
    }

    #[test]
    fn constants_meet_the_brief() {
        // Fast component: about 10% after three frames.
        assert!((FAST_DECAY.powi(3) - 0.1).abs() < 0.01);
        // A static line shows at no more than 1.2 x its beam (max deposit).
        assert!(1.0 + DEPOSIT <= 1.2 + 1e-6);
        // A line drawn twice stays below the spill point (the halo is
        // outside the core, at most HALO of it); three times goes past it.
        let one = CORE * (1.0 + DEPOSIT);
        assert!(2.0 * one < SPILL_FROM && 3.0 * one > SPILL_FROM);
        // Default width 16 sixteenths: core under 1.2 units.
        assert!(CORE_WIDTH * 32.0 <= 1.2 * 16.0 + 1e-3);
        assert!(HALO_REACH <= 1.5 * 16.0);
        // Vertex dwell is subtle.
        assert!((0.3..=0.5).contains(&DWELL));
    }

    /// Repetition counts: 1, 2, 3 and 4 times one static stroke are each
    /// brighter after the tone curve (in sRGB, 8 bits: 226, 239, 244, 247
    /// for white; the bloom, which stays below the knee, grows linearly).
    #[test]
    fn repetitions_have_headroom() {
        let srgb = |c: f32| 255.0 * if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        let one = CORE * (1.0 + DEPOSIT);
        let shown: Vec<f32> = (1..=4).map(|n| srgb(tone(n as f32 * one))).collect();
        assert!(shown[0] > 215.0 && shown[0] < 235.0, "{shown:?}");
        assert!(shown.windows(2).all(|w| w[1] - w[0] >= 2.5), "{shown:?}");
        // A mid-brightness channel (half of full) gains a lot from a repeat.
        let half = |n: f32| srgb(tone(n * 0.5 * one));
        assert!(half(2.0) - half(1.0) > 20.0);
    }

    #[test]
    fn tone_curve_is_smooth_and_bounded() {
        assert_eq!(tone(0.0), 0.0);
        assert_eq!(tone(0.3), 0.3);
        let mut last = 0.0;
        for i in 1..1000 {
            let v = tone(i as f32 * 0.01);
            assert!(v > last && v < 1.0, "{i}");
            last = v;
        }
        // Continuous at the knee, with slope 1.
        assert!((tone(TONE_KNEE + 1e-4) - TONE_KNEE - 1e-4).abs() < 2e-5);
    }

    #[test]
    fn dwell_counts_distinct_lines_at_each_end() {
        let l = |x0, y0, x1, y1| Primitive::Line { x0, y0, x1, y1, width: 16, rgba: C };
        // A triangle drawn twice, a lone line, a dot and a circle.
        let tri = [l(0, 0, 100, 0), l(100, 0, 50, 80), l(50, 80, 0, 0)];
        let mut shapes: Vec<Primitive> = tri.iter().chain(&tri).copied().collect();
        shapes.push(l(0, 0, 0, 0));
        shapes.push(l(500, 500, 600, 500));
        shapes.push(Primitive::Circle { x: 0, y: 0, radius: 9, width: 16, rgba: C });
        // The same line reversed counts as the same line.
        shapes.push(l(100, 0, 0, 0));
        let mut out = Vec::new();
        dwell_ends(&shapes, &mut out);
        // (0, 0): two triangle sides and the dot.
        assert_eq!(out[0], 3 | 2 << 4);
        assert_eq!(out[3], out[0]);
        assert_eq!(out[1], 2 | 2 << 4);
        assert_eq!(out[6], 3 | 3 << 4);
        assert_eq!(out[7], 1 | 1 << 4);
        assert_eq!(out[8], 0);
        assert_eq!(out[9], 2 | 3 << 4);
        // 3D segments: an end where a face cut the segment (a butt end) is
        // not counted and has no dwell; the other end keeps its own. A
        // circle's chords (both ends butt) get none.
        let b = |x0, y0, x1, y1, butt| Primitive::Beam { x0, y0, x1, y1, width: 16, rgba: C, fade: [255, 255], butt };
        let shapes = [b(0, 0, 100, 0, 0), b(0, 0, 0, 100, 2), b(100, 0, 50, 50, 1), b(0, 0, 9, 9, 3)];
        dwell_ends(&shapes, &mut out);
        assert_eq!(out, [2 | 1 << 4, 2, 1 << 4, 0]);
    }

    #[test]
    fn opcode_sequences() {
        let a = [0x04, 1, 0, 2, 0, 0x05, 3, 0, 4, 0, 0x06, 9, 0, 0x00, 0x05];
        let b = [0x04, 7, 0, 7, 0, 0x05, 8, 0, 8, 0, 0x06, 1, 0];
        assert_eq!(opcodes(&a), vec![0x04, 0x05, 0x06]);
        assert_eq!(opcodes(&a), opcodes(&b));
        // Cut-off operands end the list.
        assert_eq!(opcodes(&[0x05, 1, 2]), Vec::<u8>::new());
    }

    #[test]
    fn moving_shapes_smear() {
        let mut out = Vec::new();
        // The earlier positions only: the current one is in the beam.
        deposits(&[line(0)], &[line(64)], true, 0.5, &mut out);
        assert_eq!(out.len(), SUBSTEPS as usize - 1);
        // Faster: more steps, up to the most.
        deposits(&[line(0)], &[line(10 * SMEAR_SPACING as i16)], true, 0.5, &mut out);
        assert_eq!(out.len(), 9);
        deposits(&[line(0)], &[line(MAX_SMEAR as i16)], true, 0.5, &mut out);
        assert_eq!(out.len(), MAX_SUBSTEPS as usize - 1);
        deposits(&[line(0), line(9)], &[line(64), line(9)], true, 0.5, &mut out);
        let xs: Vec<i16> = out
            .iter()
            .map(|d| match d.shape {
                Primitive::Line { x0, .. } => x0,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(xs, vec![16, 32, 48]);
        assert!(out.iter().all(|d| d.index == 0));
        // Older positions have decayed more, all below the full deposit.
        assert!(out.windows(2).all(|w| w[0].fast < w[1].fast && w[0].slow < w[1].slow));
        assert!(out[2].fast < DEPOSIT * FAST_SHARE);
        // A different structure, a teleport or no movement: nothing extra.
        deposits(&[line(0)], &[line(64)], false, 0.5, &mut out);
        assert!(out.is_empty());
        deposits(&[line(0)], &[line(4000)], true, 0.5, &mut out);
        assert!(out.is_empty());
        deposits(&[line(64)], &[line(64)], true, 0.5, &mut out);
        assert!(out.is_empty());
    }
}

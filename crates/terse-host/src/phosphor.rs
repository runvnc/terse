//! The phosphor presentation (SPEC 8.7): every tunable constant in one
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
//!   the beam is deposited into them: `afterglow = max(afterglow × decay,
//!   beam × deposit)`. Depositing with `max` rather than `+` is the
//!   normalisation: a shape that stays put holds its afterglow at exactly
//!   `beam × DEPOSIT`, so it shows at `beam × (1 + DEPOSIT)` = 1.2 × and never
//!   brightens over time, while a trail left behind starts at `DEPOSIT` of
//!   the beam and fades from there.
//!
//! Displayed light is `beam + afterglow`, through a tone curve that is
//! linear up to [`TONE_KNEE`] and rolls off softly towards 1 above it, so a
//! bright trail fades smoothly instead of sitting clamped at white and then
//! vanishing. Only light above [`SPILL_FROM`] (crossings and pivots, where
//! two beams add up) spills into the other channels and turns white-hot.
//!
//! A moving shape would leave a row of separate copies, one per frame. When
//! a frame's list has the same opcode sequence as the previous frame's
//! (the same shapes, only moved), the afterglow is deposited from
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
pub const CORE: f32 = 1.0;
/// Peak brightness of the halo, at the core's edge, relative to the core.
pub const HALO: f32 = 0.12;
/// The halo's exponential fall-off length, in sixteenths of a screen unit.
pub const HALO_FALLOFF: f32 = 7.0;
/// How far past the core's edge the halo reaches (it is smoothed to zero
/// over the outer half), in sixteenths: 1.5 screen units.
pub const HALO_REACH: f32 = 24.0;

/// Brightness of glyphs and the cursor (times the foreground colour).
pub const GLYPH: f32 = 1.0;
/// The faint bloom around glyphs: the average of eight samples at
/// [`GLYPH_BLOOM_RADIUS`] screen units around each pixel, times this.
pub const GLYPH_BLOOM: f32 = 0.16;
pub const GLYPH_BLOOM_RADIUS: f32 = 1.0;

/// Share of the beam the afterglow keeps (both components together).
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
pub const DEPOSIT_BUDGET: usize = crate::vectors::MAX_PRIMITIVES * SUBSTEPS as usize;
/// A shape that jumped further than this between frames (in sixteenths of
/// a screen unit, on either axis: 40 units) is not smeared; it teleported.
pub const MAX_SMEAR: i32 = 40 * 16;

/// Tone curve: linear light up to the knee, then `knee + (1 - knee) ×
/// (1 - exp(-(x - knee) / (1 - knee)))`, which approaches 1.
pub const TONE_KNEE: f32 = 0.6;
/// Light above this (in the brightest channel) spills into all channels,
/// times [`SPILL`]: only where beams add up, not along an ordinary line
/// (whose beam plus afterglow is at most 1.2).
pub const SPILL_FROM: f32 = 1.5;
pub const SPILL: f32 = 0.5;

/// The slow component's per-frame factor for a persistence (SPEC 8.7).
pub fn slow_decay(persistence: u8) -> f32 {
    persistence as f32 / 256.0
}

/// The constants as WGSL declarations, prepended to the shaders.
pub fn wgsl() -> String {
    let consts = [
        ("CORE_WIDTH", CORE_WIDTH),
        ("CORE", CORE),
        ("HALO", HALO),
        ("HALO_FALLOFF", HALO_FALLOFF),
        ("HALO_REACH", HALO_REACH),
        ("GLYPH", GLYPH),
        ("GLYPH_BLOOM", GLYPH_BLOOM),
        ("GLYPH_BLOOM_RADIUS", GLYPH_BLOOM_RADIUS),
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
    if x <= TONE_KNEE { x } else { TONE_KNEE + (1.0 - TONE_KNEE) * (1.0 - (-(x - TONE_KNEE) / (1.0 - TONE_KNEE)).exp()) }
}

/// The opcodes of a display list in order (SPEC 8.3), operands skipped:
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

/// One afterglow deposit: a shape (a line or circle) and the light it adds
/// to the fast and slow components, per unit of beam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deposit {
    pub shape: Primitive,
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
        _ => None,
    }
}

/// The afterglow deposits of one machine frame: `cur` (the outlines drawn
/// now) once at full deposit, or, when `smear` (same opcodes as the last
/// frame, so `prev` lines up with `cur`), each shape at [`SUBSTEPS`] or
/// more positions from just after `prev` to `cur`, the earlier ones already
/// decayed for the part of a frame since. `slow_decay` is the slow
/// component's per-frame factor.
pub fn deposits(prev: &[Primitive], cur: &[Primitive], smear: bool, slow_decay: f32, out: &mut Vec<Deposit>) {
    out.clear();
    let (fast, slow) = (DEPOSIT * FAST_SHARE, DEPOSIT * (1.0 - FAST_SHARE));
    let smear = smear && prev.len() == cur.len() && prev != cur;
    let most = if cur.len() * MAX_SUBSTEPS as usize <= DEPOSIT_BUDGET { MAX_SUBSTEPS } else { SUBSTEPS };
    for (i, shape) in cur.iter().enumerate() {
        if smear && let Some(d) = moved(&prev[i], shape).filter(|&d| d > 0) {
            let steps = ((d as u32).div_ceil(SMEAR_SPACING as u32)).clamp(SUBSTEPS, most);
            for s in 1..steps {
                let t = s as f32 / steps as f32;
                let Some(p) = between(&prev[i], shape, t) else { break };
                let age = 1.0 - t;
                out.push(Deposit { shape: p, fast: fast * FAST_DECAY.powf(age), slow: slow * slow_decay.powf(age) });
            }
        }
        out.push(Deposit { shape: *shape, fast, slow });
    }
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
        // Below the spill point, even with a halo on top.
        assert!(CORE * (1.0 + DEPOSIT) + HALO < SPILL_FROM);
        // Default width 16 sixteenths: core under 1.2 units.
        assert!(CORE_WIDTH * 32.0 <= 1.2 * 16.0 + 1e-3);
        assert!(HALO_REACH <= 1.5 * 16.0);
    }

    #[test]
    fn tone_curve_is_smooth_and_bounded() {
        assert_eq!(tone(0.0), 0.0);
        assert_eq!(tone(0.3), 0.3);
        let mut last = 0.0;
        for i in 1..400 {
            let v = tone(i as f32 * 0.01);
            assert!(v > last && v < 1.0, "{i}");
            last = v;
        }
        // Continuous at the knee.
        assert!((tone(TONE_KNEE + 1e-4) - TONE_KNEE).abs() < 2e-4);
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
        deposits(&[line(0)], &[line(64)], true, 0.5, &mut out);
        assert_eq!(out.len(), SUBSTEPS as usize);
        // Faster: more steps, up to the most.
        deposits(&[line(0)], &[line(10 * SMEAR_SPACING as i16)], true, 0.5, &mut out);
        assert_eq!(out.len(), 10);
        deposits(&[line(0)], &[line(MAX_SMEAR as i16)], true, 0.5, &mut out);
        assert_eq!(out.len(), MAX_SUBSTEPS as usize);
        deposits(&[line(0)], &[line(64)], true, 0.5, &mut out);
        let xs: Vec<i16> = out
            .iter()
            .map(|d| match d.shape {
                Primitive::Line { x0, .. } => x0,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(xs, vec![16, 32, 48, 64]);
        // Older positions have decayed; the current one has the full deposit.
        assert!(out.windows(2).all(|w| w[0].fast < w[1].fast && w[0].slow < w[1].slow));
        assert_eq!(out[3].fast, DEPOSIT * FAST_SHARE);
        // A different structure, a teleport or no movement: deposited once.
        deposits(&[line(0)], &[line(64)], false, 0.5, &mut out);
        assert_eq!(out.len(), 1);
        deposits(&[line(0)], &[line(4000)], true, 0.5, &mut out);
        assert_eq!(out.len(), 1);
        deposits(&[line(64)], &[line(64)], true, 0.5, &mut out);
        assert_eq!(out.len(), 1);
    }
}

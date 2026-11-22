//! The phosphor presentation (SPEC §vector.phosphor): every tunable constant in one
//! place, the tube presets, the CPU side of the afterglow (motion
//! sub-steps), and the WGSL prelude that hands the constants to the
//! shaders. The model imitates a vector monitor or oscilloscope: a thin,
//! sharp, nearly white-hot core, brightness that adds where the beam
//! passes more than once, a very faint and very wide halo from the glass,
//! a short afterglow, and a truly black background.
//!
//! The whole screen is one tube. Each machine frame has three kinds of
//! light, all linear RGB in float textures the size of the screen:
//!
//! - the *beam*: this frame's vector shapes, drawn fresh (never
//!   accumulated). Each line is the light of a Gaussian spot of standard
//!   deviation σ moved along it at constant speed: across the line a
//!   Gaussian, along it the difference of two error functions, so lines
//!   that meet end to end join seamlessly and a lone end is a soft, round
//!   cap of half the light ([`beam_light`]). σ is in *pixels* ([`sigma`]):
//!   the core is about 1.2 pixels wide at half maximum at any resolution,
//!   so the picture sharpens as the resolution grows. Its peak is [`CORE`]
//!   (times colour and opacity). Every distinct end point gets a dot of
//!   extra light where the beam dwells ([`DWELL`]), shared between the
//!   lines that meet there. Light adds: a line drawn twice is twice the
//!   light.
//! - the *text*: the terminal's glyphs and cursor and the page's text and
//!   figures, in their own antialiasing and exactly their colours
//!   ([`GLYPH`] is 1). Text is there to be read: it gets no narrow glow and
//!   no motion smear, and adds to the wide halo at only [`TEXT_HALO`].
//! - the *afterglow*, in two parts with their own decay: a fast component
//!   whose time constant is the preset's (`Preset::fast_tau`, per second;
//!   the factor per machine frame is `exp(−Δt/τ)`), and a slow, dim tail
//!   whose per-frame factor is persistence / 256 (SPEC §vector.phosphor). Each
//!   frame both are multiplied by their factor, minus [`EPSILON`] so they
//!   really reach black, and then beam and text are deposited into them:
//!   `afterglow = max(afterglow × decay, (beam + text) × deposit)`.
//!
//! What is shown is `max(beam + text, afterglow) + halo`: something that
//! stays put shows exactly its own light (its afterglow, [`DEPOSIT`] of it,
//! is below it), while where it was a moment ago its afterglow fades.
//!
//! The halo ([`HALO`]) is a sum of a few wide, faint Gaussians computed on
//! a pyramid of the light at ½, ¼ … 1/32 resolution ([`halo_blurs`]):
//! cheap, and wide (up to a tenth of the screen) but faint, so the glow
//! never fills the space between strokes. Light above [`BLOOM_WHITE_FROM`]
//! makes the halo whiter.
//!
//! The tone curve leaves light below [`TONE_KNEE`] untouched (one stroke's
//! core, [`CORE`], is just above it) and rolls off above, scaling the
//! colour by the curve of its brightest channel so the hue is kept. Light
//! above [`SPILL_FROM`] (a line drawn three times, bright crossings) first
//! spills into the other channels and turns white-hot. Then sRGB.
//!
//! A moving shape would leave a row of separate copies, one per frame. When
//! a frame's list has the same opcode sequence as the previous frame's
//! (the same shapes, only moved), the afterglow also takes deposits from
//! [`SUBSTEPS`] (more for a fast shape, up to [`MAX_SUBSTEPS`]) positions
//! interpolated between the two frames, each already decayed by the
//! fraction of a frame since it was "drawn", at [`SMEAR`] of the deposit,
//! so the trail is a continuous smear. The beam itself is only ever at the
//! current position.
//!
//! Everything here is presentation: nothing is read back into a machine,
//! and the afterglow advances once per machine frame, never by the clock,
//! so `--screenshot` pictures are the same on every run.

use terse_core::Primitive;

/// Seconds per machine frame (SPEC: 60 frames per second, nominal).
pub const FRAME_SECONDS: f32 = 1.0 / 60.0;

/// Peak brightness of a stroke's core (linear light, times the colour and
/// opacity). One static white stroke shows at about 236 in sRGB.
pub const CORE: f32 = 0.85;
/// The beam spot's standard deviation, in pixels, at the default width of
/// one screen unit (and below it): 0.5 px is a core about 1.18 px wide at
/// half its peak.
pub const SIGMA_MIN_PX: f32 = 0.5;
/// How σ grows with a width beyond the default, per pixel of extra width:
/// σ = max([`SIGMA_MIN_PX`], 0.42 × (width − 1 unit) in pixels).
pub const SIGMA_PER_WIDTH: f32 = 0.42;
/// The default line width, in sixteenths of a screen unit.
pub const DEFAULT_WIDTH: f32 = 16.0;
/// How far from a stroke its core is drawn, in σ (the Gaussian is 4·10⁻⁵
/// of its peak there: nothing that shows after sRGB).
pub const BEAM_REACH: f32 = 4.5;

/// Vertex dwell: each distinct end point of the lines (a lone end, a
/// polygon's corner, a dot) gets a round spot of extra light, the energy of
/// DWELL × 2σ of line, about 0.8 × DWELL of the core's peak at its centre;
/// where n different lines meet each gives 1/n of it. Identical lines drawn
/// again count as one line for this, so a line drawn twice keeps twice the
/// light at its corners too.
pub const DWELL: f32 = 0.5;

/// Brightness of glyphs, the cursor and the page (times their colour): 1,
/// their colours exactly.
pub const GLYPH: f32 = 1.0;
/// The share of the text's light that feeds the halo.
pub const TEXT_HALO: f32 = 0.25;

/// The halo: Gaussians of σ (a fraction of the screen's height) added to
/// the light times their weight (each blur keeps the light's total, so a
/// weight is the share of the light spread that far). Wide and faint: next
/// to a one-pixel line the narrowest adds well under a tenth of the core.
pub const HALO: [(f32, f32); 4] = [(0.004, 0.06), (0.012, 0.08), (0.03, 0.07), (0.08, 0.05)];
/// The halo's pyramid: levels at 1/2^HALO_FIRST … 1/2^HALO_LEVELS of the
/// screen (¼ to 1/32). The light is averaged straight down to the first,
/// and the halo is sampled up from it bilinearly.
pub const HALO_FIRST: usize = 2;
pub const HALO_LEVELS: usize = 5;
/// Each Gaussian is computed at the coarsest level where its σ is still at
/// least this many of the level's pixels.
pub const HALO_LEVEL_SIGMA: f32 = 1.0;
/// Its taps reach this many σ either side; at most [`HALO_MAX_TAPS`].
pub const HALO_TAP_REACH: f32 = 3.5;
pub const HALO_MAX_TAPS: i32 = 24;
/// Light above this (in the brightest channel; one stroke's core is
/// [`CORE`]) whitens the halo: the excess times [`BLOOM_WHITE`] is added to
/// every channel of what the halo blurs, so a doubled line's glow is a
/// little whiter than a single one's.
pub const BLOOM_WHITE_FROM: f32 = 1.0;
pub const BLOOM_WHITE: f32 = 0.35;

/// Share of beam and text the afterglow keeps (both components together).
pub const DEPOSIT: f32 = 0.2;
/// The fast component's share of the deposit; the slow tail has the rest.
pub const FAST_SHARE: f32 = 0.6;
/// The deposits of a moving shape's earlier positions, as a share of
/// [`DEPOSIT`].
pub const SMEAR: f32 = 0.5;
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
/// The most ink (SPEC §vector.ink-cap) the shapes of one frame may draw
/// and still be smeared: a quarter of the screen, above what the bundled
/// programs draw (holo's city, about 51,000), so that the look that
/// multiplies the work of drawing is shed when a list is heavy.
pub const SMEAR_INK_MAX: u64 = 640 * 480 / 4;
/// Above this much ink (a third of a screen) a frame's light also
/// has no halo, above [`PLAIN_INK_MAX`] (two fifths) its lines are drawn
/// plain: flat strokes of their width, as the clean style draws them, in
/// their colours on the black screen (SPEC §vector.phosphor: a host that
/// draws plain lines conforms). The beam's Gaussian, whose skirt reaches
/// [`BEAM_REACH`] σ beyond a wide line, is most of the look's cost.
pub const HALO_INK_MAX: u64 = 640 * 480 / 3;
pub const PLAIN_INK_MAX: u64 = 640 * 480 * 2 / 5;

/// How much of the phosphor look a frame sheds (presentation only, never
/// machine state): 0 the whole look, [`SHED_SMEAR`] no smear,
/// [`SHED_HALO`] no halo either, [`SHED_PLAIN`] plain lines. Decided by
/// the ink of the frame's shapes (SPEC §vector.ink-cap, as the caps count
/// it: the same picture on every run), and never below `look.floor`.
pub const SHED_SMEAR: u8 = 1;
pub const SHED_HALO: u8 = 2;
pub const SHED_PLAIN: u8 = 3;

/// A window's governor of the look (presentation only, never machine
/// state; off for `--screenshot`, `--bench-render` and `--no-shed`): after
/// [`LATE_FRAMES`] frames in a row that took longer than a frame to draw
/// and present, the least shedding level ([`Look::floor`]) goes up one;
/// after [`GOOD_FRAMES`] in a row that took less than half a frame, down
/// one. The gap between the two keeps it from flickering.
#[derive(Clone, Copy, Debug, Default)]
pub struct Governor {
    pub floor: u8,
    late: u32,
    good: u32,
}

pub const LATE_FRAMES: u32 = 3;
pub const GOOD_FRAMES: u32 = 120;

impl Governor {
    /// A governor whose least level is `floor` (the hardware check's, R16).
    pub fn at(floor: u8) -> Governor {
        Governor { floor: floor.min(SHED_PLAIN), late: 0, good: 0 }
    }

    /// Whether a level is left to shed.
    pub fn can_shed(&self) -> bool {
        self.floor < SHED_PLAIN
    }

    /// One level more at once: the rule's first step for a frame whose
    /// drawing was blamed (`PERFORMANCE.md` R15). Returns the new floor.
    pub fn shed(&mut self) -> Option<u8> {
        if !self.can_shed() {
            return None;
        }
        self.late = 0;
        self.good = 0;
        self.floor += 1;
        Some(self.floor)
    }

    /// Takes how long a frame took to draw and present; returns the new
    /// floor if it changed.
    pub fn presented(&mut self, took: std::time::Duration) -> Option<u8> {
        let frame = FRAME_SECONDS;
        let t = took.as_secs_f32();
        if t > frame {
            self.good = 0;
            self.late += 1;
            if self.late >= LATE_FRAMES && self.floor < SHED_PLAIN {
                self.late = 0;
                self.floor += 1;
                return Some(self.floor);
            }
        } else {
            self.late = 0;
            if t < frame / 2.0 {
                self.good += 1;
                if self.good >= GOOD_FRAMES && self.floor > 0 {
                    self.good = 0;
                    self.floor -= 1;
                    return Some(self.floor);
                }
            } else {
                self.good = 0;
            }
        }
        None
    }
}

pub fn shed_level(look: &Look, shapes: &[Primitive]) -> u8 {
    if look.no_shed {
        return 0;
    }
    let ink: u64 = shapes.iter().map(terse_core::vector::stroke_ink).sum();
    let by_ink = match ink {
        i if i > PLAIN_INK_MAX => SHED_PLAIN,
        i if i > HALO_INK_MAX => SHED_HALO,
        i if i > SMEAR_INK_MAX => SHED_SMEAR,
        _ => 0,
    };
    by_ink.max(look.floor.min(SHED_PLAIN))
}
/// Most deposit instances in one frame.
pub const DEPOSIT_BUDGET: usize = crate::display::MAX_SHAPES * SUBSTEPS as usize;
/// A shape that jumped further than this between frames (in sixteenths of
/// a screen unit, on either axis: 40 units) is not smeared; it teleported.
pub const MAX_SMEAR: i32 = 40 * 16;

/// Tone curve: light up to the knee is shown as it is; above, the
/// brightest channel `m` goes to `knee + (1 − knee) × u / (1 + u)` with
/// `u = (m − knee) / (1 − knee)`, approaching 1, and the other channels are
/// scaled with it (the hue is kept).
pub const TONE_KNEE: f32 = 0.8;
/// Light above this (in the brightest channel) spills into all channels,
/// times [`SPILL`]: a saturated colour turns white only from about two and
/// a half times one stroke's light (a line drawn three times, bright
/// crossings), not at a single repetition.
pub const SPILL_FROM: f32 = 2.1;
pub const SPILL: f32 = 0.5;

/// The glass (`--vignette`): light falls off towards the corners by up to
/// this share (as the fourth power of the distance from the centre).
pub const VIGNETTE: f32 = 0.3;

/// A tube to imitate (`--tube`): its colour and its fast afterglow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preset {
    /// A colour XY monitor (Tempest, Star Wars): each line in its colour.
    #[default]
    Xy,
    /// A green P31 oscilloscope tube: very short fast decay.
    P31,
    /// An amber monochrome tube.
    Amber,
    /// The Vectrex: a white tube, colours shown as brightness. (Its
    /// coloured overlays are not drawn: `Look::overlay` is where one would go.)
    Vectrex,
}

impl Preset {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "xy" | "colour" | "color" => Some(Preset::Xy),
            "p31" | "green" => Some(Preset::P31),
            "amber" => Some(Preset::Amber),
            "vectrex" | "white" => Some(Preset::Vectrex),
            _ => None,
        }
    }

    /// The phosphor's colour (linear) on a monochrome tube; `None`: colour.
    pub fn tint(self) -> Option<[f32; 3]> {
        match self {
            Preset::Xy => None,
            Preset::P31 => Some([0.35, 1.0, 0.45]),
            Preset::Amber => Some([1.0, 0.55, 0.08]),
            Preset::Vectrex => Some([0.9, 0.95, 1.0]),
        }
    }

    /// Time constant of the fast afterglow, in seconds.
    pub fn fast_tau(self) -> f32 {
        match self {
            Preset::Xy => 0.02,
            Preset::P31 => 0.003,
            Preset::Amber => 0.05,
            Preset::Vectrex => 0.015,
        }
    }

    /// Per-frame factor of the fast afterglow.
    pub fn fast_decay(self) -> f32 {
        (-FRAME_SECONDS / self.fast_tau()).exp()
    }
}

/// How the host draws the phosphor style: the preset and the glass.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Look {
    pub preset: Preset,
    /// Darken towards the corners ([`VIGNETTE`]).
    pub vignette: bool,
    /// `--no-shed`: the whole look whatever a frame draws ([`shed_level`]).
    pub no_shed: bool,
    /// The least shedding level, set by a window's governor when frames
    /// run long ([`shed_level`]); 0 for screenshots and benchmarks.
    pub floor: u8,
    // An overlay (the Vectrex's coloured plastic, an RGBA image multiplied
    // over the light before the glass) would go here; none is implemented.
}

impl Look {
    /// The uniform the composites take: the tint (r, g, b; all 0 for
    /// colour) and the vignette's strength.
    pub fn uniform(&self) -> [f32; 4] {
        let [r, g, b] = self.preset.tint().unwrap_or([0.0; 3]);
        [r, g, b, if self.vignette { VIGNETTE } else { 0.0 }]
    }
}

/// σ of the beam spot in pixels for a stated width (sixteenths), where a
/// pixel is `px` sixteenths.
pub fn sigma(width: f32, px: f32) -> f32 {
    SIGMA_MIN_PX.max(SIGMA_PER_WIDTH * (width - DEFAULT_WIDTH).max(0.0) / px)
}

/// Peak of a stroke's core for a stated width (sixteenths): [`CORE`], less
/// for a line thinner than the default.
pub fn peak(width: f32) -> f32 {
    CORE * (width / DEFAULT_WIDTH).min(1.0)
}

/// The error function (Abramowitz and Stegun 7.1.26, error below 1.5·10⁻⁷).
#[inline]
pub fn erf(x: f32) -> f32 {
    let a = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * a);
    let p = ((((1.061_405_4 * t - 1.453_152_1) * t + 1.421_413_8) * t - 0.284_496_74) * t + 0.254_829_6) * t;
    let y = 1.0 - p * (-a * a).exp();
    if x < 0.0 { -y } else { y }
}

/// 2/√(2π): the centre of a dot whose energy is that of 2σ of line, as a
/// share of the line's peak.
pub const DOT: f32 = 0.797_884_6;

/// The light (before colour, times the peak) of a segment of length `len`
/// at a point `along` it from its start and `across` from its axis, all in
/// units of σ: a Gaussian across, the difference of two error functions
/// along; 1 in the middle of a long line, ½ at a lone end. A segment of
/// length 0 is a dot with the energy of 2σ of line.
#[inline]
#[cfg_attr(not(test), allow(dead_code))] // The model: the GPU computes it, the CPU from tables (tested against it).
pub fn beam_light(along: f32, across: f32, len: f32) -> f32 {
    let g = (-0.5 * across * across).exp();
    if len <= 1e-4 {
        return DOT * g * (-0.5 * along * along).exp();
    }
    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
    // Further than 5.5σ inside both ends the two error functions are ±1
    // to within 10⁻¹⁴.
    if along > 5.5 && along < len - 5.5 {
        return g;
    }
    g * 0.5 * (erf(along * S) - erf((along - len) * S))
}

/// The halo's blurs at a screen `height` pixels high: for each of [`HALO`],
/// the pyramid level it is computed at (1 is half resolution), its σ in
/// that level's pixels, its taps either side and its weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HaloBlur {
    pub level: usize,
    pub sigma: f32,
    pub taps: i32,
    pub weight: f32,
}

pub fn halo_blurs(height: f32) -> Vec<HaloBlur> {
    HALO.iter()
        .map(|&(frac, weight)| {
            let s = frac * height;
            let level = ((s / HALO_LEVEL_SIGMA).log2().floor() as i32).clamp(HALO_FIRST as i32, HALO_LEVELS as i32) as usize;
            let scale = (1 << level) as f32;
            let sigma = (s * s - pyramid_spread(level)).max(0.0).sqrt() / scale;
            let taps = if sigma < 0.3 { 0 } else { ((HALO_TAP_REACH * sigma).ceil() as i32).min(HALO_MAX_TAPS) };
            HaloBlur { level, sigma, taps, weight }
        })
        .collect()
}

/// The variance (pixels² of the screen) the pyramid itself adds to what is
/// blurred on level `k`: averaging 2^k × 2^k pixels, the bilinear steps up
/// to the first level, and the bilinear step from it to the screen.
pub fn pyramid_spread(k: usize) -> f32 {
    let four = |j: usize| (1u64 << (2 * j)) as f32;
    let average = (four(k) - 1.0) / 12.0;
    let steps: f32 = (HALO_FIRST + 1..=k).map(|j| 0.1875 * four(j)).sum();
    (average + steps + up_spread(1 << HALO_FIRST)).max(0.0)
}

/// The variance (pixels²) of sampling up bilinearly by `factor`, averaged
/// over the pixels.
fn up_spread(factor: usize) -> f32 {
    let f = factor as f32;
    let mean: f32 = (0..factor)
        .map(|r| {
            let t = ((r as f32 + 0.5) / f - 0.5).rem_euclid(1.0);
            t * (1.0 - t)
        })
        .sum::<f32>()
        / f;
    mean * f * f
}

/// Where pixel `i` samples a texture `factor` times smaller (`n` pixels):
/// the two pixels and the second's weight, edges repeated.
#[inline]
pub fn up(i: usize, n: usize, factor: usize) -> (usize, usize, f32) {
    let f = (i as f32 + 0.5) / factor as f32 - 0.5;
    let i0 = f.floor();
    let t = f - i0;
    let c = |v: f32| (v.max(0.0) as usize).min(n - 1);
    (c(i0), c(i0 + 1.0), t)
}

/// The normalised weights of a blur's taps, from −taps to taps.
pub fn kernel(b: &HaloBlur) -> Vec<f32> {
    if b.taps == 0 {
        return vec![1.0];
    }
    let w: Vec<f32> = (-b.taps..=b.taps).map(|i| (-((i * i) as f32) / (2.0 * b.sigma * b.sigma)).exp()).collect();
    let total: f32 = w.iter().sum();
    w.iter().map(|v| v / total).collect()
}

/// The size of pyramid level `k` of a `size` screen (each level half the
/// one before, rounded up).
pub fn level_size(size: (usize, usize), k: usize) -> (usize, usize) {
    let mut s = size;
    for _ in 0..k {
        s = (s.0.div_ceil(2), s.1.div_ceil(2));
    }
    s
}

/// The factor the tone curve scales a colour by whose brightest channel
/// is `m`.
#[inline]
pub fn tone_scale(m: f32) -> f32 {
    if m <= TONE_KNEE {
        return 1.0;
    }
    let u = (m - TONE_KNEE) / (1.0 - TONE_KNEE);
    (TONE_KNEE + (1.0 - TONE_KNEE) * u / (1.0 + u)) / m
}

/// The tone curve of a channel value on its own (for tests).
#[cfg(test)]
pub fn tone(x: f32) -> f32 {
    x * tone_scale(x)
}

/// The constants as WGSL declarations, prepended to the shaders.
#[cfg(feature = "gpu")]
pub fn wgsl() -> String {
    let consts = [
        ("CORE", CORE),
        ("SIGMA_MIN_PX", SIGMA_MIN_PX),
        ("SIGMA_PER_WIDTH", SIGMA_PER_WIDTH),
        ("DEFAULT_WIDTH", DEFAULT_WIDTH),
        ("BEAM_REACH", BEAM_REACH),
        ("DWELL", DWELL),
        ("DOT", DOT),
        ("GLYPH", GLYPH),
        ("TEXT_HALO", TEXT_HALO),
        ("BLOOM_WHITE_FROM", BLOOM_WHITE_FROM),
        ("BLOOM_WHITE", BLOOM_WHITE),
        ("DEPOSIT", DEPOSIT),
        ("FAST_SHARE", FAST_SHARE),
        ("EPSILON", EPSILON),
        ("TONE_KNEE", TONE_KNEE),
        ("SPILL_FROM", SPILL_FROM),
        ("SPILL", SPILL),
    ];
    let mut out: String = consts.iter().map(|(name, v)| format!("const {name}: f32 = {v:?};\n")).collect();
    out.push_str(&format!("const HALO_FACTOR: i32 = {};\n", 1 << HALO_FIRST));
    out.push_str(WGSL_FUNCTIONS);
    out
}

/// The shaders' versions of [`erf`], [`beam_light`], [`sigma`], [`peak`] and
/// the tone curve with its spill and tint.
#[cfg(feature = "gpu")]
const WGSL_FUNCTIONS: &str = r#"
fn erf_as(x: f32) -> f32 {
    let a = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let p = ((((1.0614054 * t - 1.4531521) * t + 1.4214138) * t - 0.28449674) * t + 0.2548296) * t;
    let y = 1.0 - p * exp(-a * a);
    return select(y, -y, x < 0.0);
}

fn beam_profile(along: f32, across: f32, len: f32) -> f32 {
    let g = exp(-0.5 * across * across);
    if len <= 1e-4 {
        return DOT * g * exp(-0.5 * along * along);
    }
    let s = 0.70710677;
    return g * 0.5 * (erf_as(along * s) - erf_as((along - len) * s));
}

// σ in pixels for a width in sixteenths, a pixel being px sixteenths.
fn beam_sigma(width: f32, px: f32) -> f32 {
    return max(SIGMA_MIN_PX, SIGMA_PER_WIDTH * max(width - DEFAULT_WIDTH, 0.0) / px);
}

fn beam_peak(width: f32) -> f32 {
    return CORE * min(width / DEFAULT_WIDTH, 1.0);
}

// The shown colour of linear light: a monochrome tube's tint (look.rgb, all
// 0 for colour), the spill into white, the hue-preserving tone curve.
fn tube_tone(light_in: vec3<f32>, look: vec4<f32>) -> vec3<f32> {
    var light = light_in;
    if any(look.rgb > vec3<f32>(0.0)) {
        let m = max(light.r, max(light.g, light.b));
        let l = 0.5 * m + 0.5 * dot(light, vec3<f32>(0.2126, 0.7152, 0.0722));
        light = l * look.rgb;
    }
    let peak = max(light.r, max(light.g, light.b));
    light += vec3<f32>(max(peak - SPILL_FROM, 0.0) * SPILL);
    let m = max(light.r, max(light.g, light.b));
    if m > TONE_KNEE {
        let u = (m - TONE_KNEE) / (1.0 - TONE_KNEE);
        light *= (TONE_KNEE + (1.0 - TONE_KNEE) * u / (1.0 + u)) / m;
    }
    return light;
}
"#;

/// A monochrome tube's colour of linear light `c` (`tint` from
/// [`Preset::tint`]).
#[inline]
pub fn tinted(c: [f32; 3], tint: [f32; 3]) -> [f32; 3] {
    let m = c[0].max(c[1]).max(c[2]);
    let l = 0.5 * m + 0.5 * (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]);
    [l * tint[0], l * tint[1], l * tint[2]]
}

/// The glass's factor at a point `(x, y)` from −1 to 1 across the screen.
#[inline]
pub fn vignette(x: f32, y: f32, strength: f32) -> f32 {
    let r2 = 0.5 * (x * x + y * y);
    1.0 - strength * r2 * r2
}

/// The slow component's per-frame factor for a persistence (SPEC §vector.phosphor).
pub fn slow_decay(persistence: u8) -> f32 {
    persistence as f32 / 256.0
}

/// The opcodes of a display list in order (SPEC §vector.list), operands skipped:
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
/// `fast_decay` and `slow_decay` are the components' per-frame factors.
/// `shed`: the frame's [`shed_level`]; from [`SHED_SMEAR`] on there is no
/// smear (each smeared shape is drawn again 3 to 15 times).
pub fn deposits(prev: &[Primitive], cur: &[Primitive], smear: bool, shed: u8, fast_decay: f32, slow_decay: f32, out: &mut Vec<Deposit>) {
    out.clear();
    let smear = smear && prev.len() == cur.len() && prev != cur;
    if !smear || shed >= SHED_SMEAR {
        return;
    }
    let (fast, slow) = (SMEAR * DEPOSIT * FAST_SHARE, SMEAR * DEPOSIT * (1.0 - FAST_SHARE));
    let most = if cur.len() * MAX_SUBSTEPS as usize <= DEPOSIT_BUDGET { MAX_SUBSTEPS } else { SUBSTEPS };
    for (i, shape) in cur.iter().enumerate() {
        if let Some(d) = moved(&prev[i], shape).filter(|&d| d > 0) {
            let steps = ((d as u32).div_ceil(SMEAR_SPACING as u32)).clamp(SUBSTEPS, most);
            for s in 1..steps {
                let t = s as f32 / steps as f32;
                let Some(p) = between(&prev[i], shape, t) else { break };
                let age = 1.0 - t;
                out.push(Deposit { shape: p, index: i, fast: fast * fast_decay.powf(age), slow: slow * slow_decay.powf(age) });
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

    fn srgb(c: f32) -> f32 {
        255.0 * if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    }

    #[test]
    fn constants_meet_the_brief() {
        // A static line shows its own light; its afterglow is below it.
        assert!(DEPOSIT < 1.0 && SMEAR <= 0.5);
        // One stroke's core sits just above the knee, so a repetition is
        // still brighter; a line drawn twice stays below the spill point,
        // three times goes past it.
        assert!(CORE > TONE_KNEE && CORE < 0.9);
        assert!(2.0 * CORE < SPILL_FROM && 3.0 * CORE > SPILL_FROM);
        // The core is about 1.2 px wide at half maximum at the default width,
        // whatever the resolution.
        for px in [16.0, 8.0, 4.0, 2.0] {
            let fwhm = 2.0 * (2.0 * 2f32.ln()).sqrt() * sigma(DEFAULT_WIDTH, px);
            assert!((1.1..=1.3).contains(&fwhm), "{fwhm}");
        }
        // Wider lines get wider in proportion to the picture.
        assert!(sigma(64.0, 4.0) > 4.0 && sigma(64.0, 2.0) > 2.0 * sigma(64.0, 4.0) - 0.01);
        assert!((0.3..=0.6).contains(&DWELL));
        // The fast afterglow of the default tube is about 10% after three frames.
        assert!((Preset::Xy.fast_decay().powi(3) - 0.08).abs() < 0.03, "{}", Preset::Xy.fast_decay());
        assert!(Preset::P31.fast_decay() < 0.01);
    }

    /// The error function against known values, and the beam's profile:
    /// 1 along a long line, ½ at a lone end, segments joining end to end
    /// add up to the whole line.
    #[test]
    fn the_beam_profile() {
        for (x, want) in [(0.0, 0.0), (0.5, 0.520_499_9), (1.0, 0.842_700_8), (2.0, 0.995_322_3), (-1.0, -0.842_700_8)] {
            assert!((erf(x) - want).abs() < 2e-6, "{x}");
        }
        assert!((beam_light(10.0, 0.0, 20.0) - 1.0).abs() < 1e-5);
        assert!((beam_light(0.0, 0.0, 20.0) - 0.5).abs() < 1e-5);
        assert!((beam_light(10.0, 1.0, 20.0) - (-0.5f32).exp()).abs() < 1e-5);
        for along in [-3.0, -0.5, 0.0, 1.3, 4.0, 7.2, 9.0] {
            for across in [0.0, 0.7, 2.0] {
                let whole = beam_light(along, across, 9.0);
                let parts = beam_light(along, across, 2.5) + beam_light(along - 2.5, across, 4.0) + beam_light(along - 6.5, across, 2.5);
                assert!((whole - parts).abs() < 2e-6, "{along} {across}");
            }
        }
        // A dot: the energy of 2σ of line.
        let mut dot = 0.0;
        for i in -60..=60 {
            for j in -60..=60 {
                dot += beam_light(i as f32 * 0.1, j as f32 * 0.1, 0.0) * 0.01;
            }
        }
        let line_per_sigma = (2.0 * std::f32::consts::PI).sqrt();
        assert!((dot - 2.0 * line_per_sigma).abs() < 0.01, "{dot}");
    }

    /// The tone curve leaves light below the knee alone, keeps the hue
    /// above it, and each repetition of one stroke is brighter.
    #[test]
    fn tone_curve_is_smooth_and_bounded() {
        assert_eq!(tone(0.0), 0.0);
        assert_eq!(tone(0.5), 0.5);
        assert_eq!(tone(TONE_KNEE), TONE_KNEE);
        let mut last = 0.0;
        for i in 1..1000 {
            let v = tone(i as f32 * 0.01);
            assert!(v > last && v < 1.0, "{i}");
            last = v;
        }
        // Continuous at the knee, with slope 1.
        assert!((tone(TONE_KNEE + 1e-4) - TONE_KNEE - 1e-4).abs() < 2e-5);
        let shown: Vec<f32> = (1..=3).map(|n| srgb(tone(n as f32 * CORE))).collect();
        assert!(shown[0] > 230.0 && shown[0] < 240.0, "{shown:?}");
        assert!(shown[1] - shown[0] >= 10.0 && shown[2] - shown[1] >= 1.5, "{shown:?}");
    }

    #[test]
    fn halo_blurs_are_wide_and_cheap() {
        for h in [240.0, 480.0, 960.0, 1920.0, 2880.0] {
            let blurs = halo_blurs(h);
            assert_eq!(blurs.len(), HALO.len());
            for (b, &(frac, _)) in blurs.iter().zip(&HALO) {
                assert!((HALO_FIRST..=HALO_LEVELS).contains(&b.level));
                assert!(b.taps <= HALO_MAX_TAPS);
                // The spread it ends up with is the one asked for (unless it
                // is narrower than the pyramid's own).
                let scale = (1 << b.level) as f32;
                let got = ((b.sigma * scale).powi(2) + pyramid_spread(b.level)).sqrt();
                let want = frac * h;
                assert!(got >= want * 0.99 && (got <= want * 1.01 || b.sigma == 0.0), "{h} {frac}: {got} {want}");
                let k = kernel(b);
                assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            }
        }
        assert_eq!(level_size((1280, 961), 1), (640, 481));
        assert_eq!(level_size((1280, 961), 5), (40, 31));
        // The spread of sampling up: by 2 (3:1 mixes) and by 4.
        assert!((up_spread(2) - 0.75).abs() < 1e-6);
        assert!((up_spread(4) - 2.75).abs() < 1e-6);
        assert_eq!(up(0, 5, 2), (0, 0, 0.75));
        assert_eq!(up(1, 5, 2), (0, 1, 0.25));
        assert_eq!(up(9, 5, 2), (4, 4, 0.25));
        assert_eq!(up(5, 5, 4), (0, 1, 0.875));
    }

    #[test]
    fn presets() {
        assert_eq!(Preset::parse("p31"), Some(Preset::P31));
        assert_eq!(Preset::parse("xy"), Some(Preset::Xy));
        assert_eq!(Preset::parse("nope"), None);
        assert_eq!(Look::default().uniform(), [0.0; 4]);
        // A monochrome tube shows white at its tint's full brightness.
        let t = Preset::P31.tint().unwrap();
        assert_eq!(tinted([1.0; 3], t), t);
        assert_eq!(vignette(0.0, 0.0, VIGNETTE), 1.0);
        assert!((vignette(1.0, 1.0, VIGNETTE) - (1.0 - VIGNETTE)).abs() < 1e-6);
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
        let fd = Preset::Xy.fast_decay();
        // The earlier positions only: the current one is in the beam.
        deposits(&[line(0)], &[line(64)], true, 0, fd, 0.5, &mut out);
        assert_eq!(out.len(), SUBSTEPS as usize - 1);
        // Faster: more steps, up to the most.
        deposits(&[line(0)], &[line(10 * SMEAR_SPACING as i16)], true, 0, fd, 0.5, &mut out);
        assert_eq!(out.len(), 9);
        deposits(&[line(0)], &[line(MAX_SMEAR as i16)], true, 0, fd, 0.5, &mut out);
        assert_eq!(out.len(), MAX_SUBSTEPS as usize - 1);
        deposits(&[line(0), line(9)], &[line(64), line(9)], true, 0, fd, 0.5, &mut out);
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
        assert!(out[2].fast < SMEAR * DEPOSIT * FAST_SHARE);
        // A different structure, a teleport or no movement: nothing extra.
        deposits(&[line(0)], &[line(64)], false, 0, fd, 0.5, &mut out);
        assert!(out.is_empty());
        deposits(&[line(0)], &[line(4000)], true, 0, fd, 0.5, &mut out);
        assert!(out.is_empty());
        deposits(&[line(64)], &[line(64)], true, 0, fd, 0.5, &mut out);
        assert!(out.is_empty());
        deposits(&[line(0)], &[line(64)], true, SHED_SMEAR, fd, 0.5, &mut out);
        assert!(out.is_empty(), "shed: no smear");
    }

    /// The look is shed by the frame's ink alone, step by step, never
    /// below the governor's floor, and not at all with --no-shed.
    #[test]
    fn shedding_follows_the_ink() {
        // Lines across the screen, 16 units wide, about 9,500 units of ink each.
        let wide = |n: usize| -> Vec<Primitive> {
            (0..n).map(|k| Primitive::Line { x0: 0, y0: 16 * k as i16, x1: 10_240, y1: 16 * k as i16, width: 255, rgba: [255; 4] }).collect()
        };
        let ink = |s: &[Primitive]| s.iter().map(terse_core::vector::stroke_ink).sum::<u64>();
        let look = Look::default();
        let levels: Vec<u8> = [1, 10, 12, 15].iter().map(|&n| shed_level(&look, &wide(n))).collect();
        assert_eq!(levels, [0, SHED_SMEAR, SHED_HALO, SHED_PLAIN], "{:?}", [1, 10, 12, 15].map(|n| ink(&wide(n))));
        assert_eq!(shed_level(&Look { floor: SHED_HALO, ..look }, &wide(1)), SHED_HALO);
        assert_eq!(shed_level(&Look { no_shed: true, floor: SHED_PLAIN, ..look }, &wide(15)), 0);
    }

    /// The bundled programs that draw keep the whole look: sparks and holo
    /// (each of its scenes: Space switches) in the phosphor style, frame by
    /// frame, never shed. Prints the most ink a frame drew (`--nocapture`).
    #[test]
    fn bundled_scenes_keep_the_whole_look() {
        use terse_core::{Event, FrameInput, Image, Machine};
        for (name, image, key) in [("sparks", terse_net::builtin::PROGRAMS[1], None), ("holo", terse_net::builtin::PROGRAMS[5], Some(0x2Cu8))] {
            let mut m = Machine::new(&Image::parse(image).unwrap());
            let (mut most, mut shapes, mut opcodes) = (0u64, Vec::new(), Vec::new());
            for f in 0..1200u32 {
                // Holo: the next scene every 150 frames (Space).
                let events: Vec<Event> = match key {
                    Some(k) if f % 150 == 149 => vec![Event::key_down(k, 0), Event::key_up(k, 0)],
                    _ => Vec::new(),
                };
                m.run_frame(&FrameInput { events: &events, ..Default::default() });
                let scene = crate::display::VectorScene { vector: m.vector(), vector3: m.vector3(), eye: (0.0, 0.0), caps: crate::display::Caps::FULL };
                scene.shapes(true, &mut shapes, &mut opcodes);
                most = most.max(shapes.iter().map(terse_core::vector::stroke_ink).sum());
                assert_eq!(shed_level(&Look::default(), &shapes), 0, "{name}, frame {f}");
            }
            eprintln!("{name}: at most {most} square units of beam ink a frame (smear shed above {SMEAR_INK_MAX})");
        }
    }

    /// The governor steps down after 3 late frames, up after 120 quick
    /// ones, and holds in between.
    #[test]
    fn the_governor_has_hysteresis() {
        use std::time::Duration;
        let (late, ok, quick) = (Duration::from_millis(25), Duration::from_millis(12), Duration::from_millis(4));
        let mut g = Governor::default();
        assert_eq!([late, late].map(|t| g.presented(t)), [None, None]);
        assert_eq!(g.presented(late), Some(1));
        for _ in 0..1000 {
            assert_eq!(g.presented(ok), None);
        }
        for _ in 0..119 {
            assert_eq!(g.presented(quick), None);
        }
        assert_eq!(g.presented(quick), Some(0));
        for k in 1..=3 {
            assert_eq!([late, late, late].map(|t| g.presented(t))[2], Some(k));
        }
        assert_eq!([late, late, late].map(|t| g.presented(t)), [None; 3], "at most plain lines");
    }
}

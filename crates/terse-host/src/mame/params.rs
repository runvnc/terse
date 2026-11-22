// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/vectorrenderer.cpp and
// vectorrenderer.h, with the render states and default uniforms of
// bgfx/effects/vector-crt/*.json (copyright (c) 2016, Ryan Holtz and MAME
// Development Team), commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b, to Rust
// and wgpu for Tersenet. The originals are in third_party/mame-vector-crt/.
//
//!
//! MAME's sliders and constants (`vectorrenderer.h`, `create_sliders`) and the
//! shaders' sources, shared by the GPU pipeline ([`super::gpu`]) and the CPU
//! one ([`super::cpu`]).

/// `BLOOM_PASSES`: horizontal and vertical blur iterations.
pub const BLOOM_PASSES: u32 = 2;

/// `BEAM_SIGMA_SCALE`: "Empirical conversion from MAME's target-pixel line
/// width to Gaussian sigma. This controls profile calibration; it is not a
/// resolution-scaling factor."
pub const BEAM_SIGMA_SCALE: f32 = 0.085;

/// The seven sliders of `create_sliders`, at their values (MAME's defaults
/// unless changed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// "Vector phosphor persistence": τ in seconds (1–100 ms, default 2 ms).
    pub persistence: f32,
    /// "Vector beam width": multiplier on the target-scaled line width
    /// (0.30–4.00, default 0.75).
    pub beam_width: f32,
    /// "Vector beam intensity": HDR energy gain (0.10–5.00, default 4.00).
    pub beam_intensity: f32,
    /// "Vector beam halo": the wide Gaussian's strength (0–1, default 0.04).
    pub halo: f32,
    /// "Vector bloom strength" (0–3, default 0.20).
    pub bloom_strength: f32,
    /// "Vector bloom radius" in output pixels at 1080 lines (0.50–2.60,
    /// default 2.12).
    pub bloom_radius: f32,
    /// "Vector exposure" (0.10–4.00, default 0.78).
    pub exposure: f32,
}

impl Default for Params {
    /// The sliders' default values (`create_sliders`' `descriptions`):
    /// persistence 2 (× 0.001 s), width 75, intensity 400, halo 4, bloom
    /// strength 20, bloom radius 212, exposure 78 (× 0.01).
    fn default() -> Self {
        Params { persistence: 0.002, beam_width: 0.75, beam_intensity: 4.00, halo: 0.04, bloom_strength: 0.20, bloom_radius: 2.12, exposure: 0.78 }
    }
}

impl Params {
    /// The defaults, changed by environment variables named as the sliders
    /// (`TERSE_MAME_PERSISTENCE` in milliseconds, `TERSE_MAME_BEAM_WIDTH`,
    /// `TERSE_MAME_BEAM_INTENSITY`, `TERSE_MAME_HALO`,
    /// `TERSE_MAME_BLOOM_STRENGTH`, `TERSE_MAME_BLOOM_RADIUS`,
    /// `TERSE_MAME_EXPOSURE`), each clamped to its slider's range.
    pub fn from_env() -> Self {
        let mut p = Params::default();
        let get = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
        if let Some(v) = get("TERSE_MAME_PERSISTENCE") {
            p.persistence = v.clamp(1.0, 100.0) / 1000.0;
        }
        let set = |name: &str, field: &mut f32, lo: f32, hi: f32| {
            if let Some(v) = get(name) {
                *field = v.clamp(lo, hi);
            }
        };
        set("TERSE_MAME_BEAM_WIDTH", &mut p.beam_width, 0.30, 4.00);
        set("TERSE_MAME_BEAM_INTENSITY", &mut p.beam_intensity, 0.10, 5.00);
        set("TERSE_MAME_HALO", &mut p.halo, 0.0, 1.0);
        set("TERSE_MAME_BLOOM_STRENGTH", &mut p.bloom_strength, 0.0, 3.0);
        set("TERSE_MAME_BLOOM_RADIUS", &mut p.bloom_radius, 0.50, 2.60);
        set("TERSE_MAME_EXPOSURE", &mut p.exposure, 0.10, 4.00);
        p
    }
}

/// The shaders: the BSD-3-Clause translations and the two interface files.
pub mod shaders {
    pub const BEAM_COMMON: &str = include_str!("../../shaders/mame/beam_common.wgsl");
    pub const VS_BEAM: &str = include_str!("../../shaders/mame/vs_beam.wgsl");
    pub const FS_BEAM: &str = include_str!("../../shaders/mame/fs_beam.wgsl");
    pub const POST_COMMON: &str = include_str!("../../shaders/mame/post_common.wgsl");
    pub const VS_POST: &str = include_str!("../../shaders/mame/vs_post.wgsl");
    pub const FS_DECAY: &str = include_str!("../../shaders/mame/fs_decay.wgsl");
    pub const FS_DOWNSAMPLE: &str = include_str!("../../shaders/mame/fs_downsample.wgsl");
    pub const FS_BLUR: &str = include_str!("../../shaders/mame/fs_blur.wgsl");
    pub const FS_COMPOSITE: &str = include_str!("../../shaders/mame/fs_composite.wgsl");

    /// Each program as one WGSL module: (name, vertex entry, fragment
    /// entry, source). bgfx compiles a vertex and a fragment shader
    /// separately and links them by the varyings; WGSL wants one module.
    pub fn programs() -> Vec<(&'static str, &'static str, &'static str, String)> {
        let post = |fs: &str| format!("{POST_COMMON}\n{VS_POST}\n{fs}");
        vec![
            ("beam", "vs_beam", "fs_beam", format!("{BEAM_COMMON}\n{VS_BEAM}\n{FS_BEAM}")),
            ("decay", "vs_post", "fs_decay", post(FS_DECAY)),
            ("downsample", "vs_post", "fs_downsample", post(FS_DOWNSAMPLE)),
            ("blur", "vs_post", "fs_blur", post(FS_BLUR)),
            ("composite", "vs_post", "fs_composite", post(FS_COMPOSITE)),
        ]
    }
}

//! Screen renderer: the terminal and the vector display, combined in the
//! layer order of SPEC §vector.layers. Independent of windowing and of where glyph
//! bitmaps come from, so a browser front end can reuse it.
//!
//! Two passes of one full-screen terminal shader (over the cells, glyph
//! atlas and palette textures) with the vectors between them:
//!
//! 1. background mode: cell backgrounds into an offscreen layer that holds
//!    sRGB-encoded colours (`Rgba8Unorm`);
//! 2. the vector shapes, blended into that layer (`vectors.rs`);
//! 3. glyph mode: into the target, each pixel is the layer below mixed with
//!    the cell's foreground by glyph (or cursor) coverage, in sRGB space as
//!    before, then converted to linear for an sRGB target.
//!
//! With no shapes the layer holds exactly the background colours, so the
//! result is the same as the earlier single pass.
//!
//! The phosphor style (SPEC §vector.phosphor; the model and its constants are in
//! `phosphor.rs`) treats the whole screen as one tube. Cell backgrounds are
//! not drawn. Once per machine frame (`accumulate`) the glyphs and cursor
//! are drawn as light in their foreground colour (a third mode of the same
//! shader) into the text texture, and the page's light over them; then
//! `vectors.rs` fades the afterglow, draws the shapes into the beam texture
//! and deposits beam and text into the afterglow, and `halo.rs` computes
//! the halo. The glyph pass then shows `max(beam + text, afterglow)` plus
//! the halo through the tone curve on black instead of steps 1-3.

use std::collections::HashMap;

use terse_core::{COLS, ROWS, SCREEN_HEIGHT, SCREEN_WIDTH, Terminal, Vector};

pub use crate::display::GlyphSource;
use crate::display::{VectorScene, grid_rect, outlined, screen_pos};
use crate::page::{HostLayout, PageLayer};
use crate::halo::{HaloChain, HaloPipelines};
use crate::phosphor;
use crate::vectors::{PHOSPHOR_FORMAT, PhosphorTargets, VectorLayer};

/// A machine frame in seconds (MAME's frame interval).
const FRAME_SECONDS_F64: f64 = phosphor::FRAME_SECONDS as f64;

/// Format of the offscreen layer of backgrounds and shapes.
const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const ATLAS_COLS: u32 = 64;
const ATLAS_ROWS: u32 = 64;
const ATLAS_SLOTS: usize = (ATLAS_COLS * ATLAS_ROWS) as usize;
/// Slot 0 stays blank and is used for spaces and unrenderable code points.
const BLANK_SLOT: u16 = 0;

const SHADER: &str = r#"
struct Params {
    origin: vec2<f32>,      // grid rectangle in framebuffer pixels
    size: vec2<f32>,
    cell_px: vec2<u32>,     // glyph cell size in the atlas
    grid: vec2<u32>,        // columns, rows
    cursor: vec2<u32>,
    atlas_cols: u32,
    flags: u32,             // bit 0: cursor shown, bit 1: target is sRGB, bit 2: background mode,
                            // bit 3: show the phosphor light instead (glyph mode),
                            // bit 4: glyph light mode (into the text texture),
                            // bit 5: the phosphor light is MAME's picture (in `beam`, gamma-encoded),
                            // bit 6: glyph light without the cells the beam draws (slot bit 15) or the cursor,
                            // bits 8-15: the first row of the host's overlay (glyph mode; 0: none)
    look: vec4<f32>,        // the phosphor tube: a monochrome tube's tint (rgb, 0 for colour), the vignette
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var cells: texture_2d<u32>;
@group(0) @binding(2) var atlas: texture_2d<f32>;
@group(0) @binding(3) var palette: texture_2d<f32>;
@group(0) @binding(4) var under: texture_2d<f32>;  // backgrounds and shapes (glyph mode)
// Phosphor light over the grid rectangle, linear: the beam, the
// afterglow's fast and slow components (which may be below zero: dark),
// the halo (the first level of halo.rs's pyramid) and the text.
@group(0) @binding(5) var beam: texture_2d<f32>;
@group(0) @binding(6) var glow_fast: texture_2d<f32>;
@group(0) @binding(7) var glow_slow: texture_2d<f32>;
@group(0) @binding(8) var halo: texture_2d<f32>;
@group(0) @binding(9) var text: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the viewport.
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}

fn palette_rgb(index: u32) -> vec3<f32> {
    return textureLoad(palette, vec2<u32>(index, 0u), 0).rgb;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// Glyph light mode: the light of the glyph (or cursor) at q, pixels from
// the grid's top left: its coverage times its foreground colour, linear.
fn glyph_light(q: vec2<f32>) -> vec3<f32> {
    let rel = q / p.size;
    if any(rel < vec2<f32>(0.0)) || any(rel >= vec2<f32>(1.0)) {
        return vec3<f32>(0.0);
    }
    let g = rel * vec2<f32>(p.grid);
    let cell = min(vec2<u32>(g), p.grid - 1u);
    let px = min(vec2<u32>(fract(g) * vec2<f32>(p.cell_px)), p.cell_px - 1u);
    let c = textureLoad(cells, cell, 0);
    let strokes = (p.flags & 64u) != 0u;
    if strokes && (c.g & 128u) != 0u {
        return vec3<f32>(0.0);
    }
    let slot = (c.r | (c.g << 8u)) & 0x7FFFu;
    let origin = vec2<u32>(slot % p.atlas_cols, slot / p.atlas_cols) * p.cell_px;
    var coverage = textureLoad(atlas, origin + px, 0).r;
    let underline = max(p.cell_px.y / 10u, 1u);
    if !strokes && (p.flags & 1u) != 0u && all(cell == p.cursor) && px.y >= p.cell_px.y - underline {
        coverage = 1.0;
    }
    return to_linear(palette_rgb(c.b)) * coverage;
}

// Where pixel i samples the halo, HALO_FACTOR times smaller (n pixels): the
// two pixels and the second's weight, edges repeated.
fn up(i: i32, n: i32) -> vec3<f32> {
    let f = (f32(i) + 0.5) / f32(HALO_FACTOR) - 0.5;
    let i0 = floor(f);
    return vec3<f32>(clamp(i0, 0.0, f32(n - 1)), clamp(i0 + 1.0, 0.0, f32(n - 1)), f - i0);
}

// The halo at a texel of the screen: the pyramid's first level, sampled up.
fn halo_at(at: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(halo));
    let ux = up(at.x, size.x);
    let uy = up(at.y, size.y);
    let a = textureLoad(halo, vec2<i32>(i32(ux.x), i32(uy.x)), 0).rgb;
    let b = textureLoad(halo, vec2<i32>(i32(ux.y), i32(uy.x)), 0).rgb;
    let c = textureLoad(halo, vec2<i32>(i32(ux.x), i32(uy.y)), 0).rgb;
    let d = textureLoad(halo, vec2<i32>(i32(ux.y), i32(uy.y)), 0).rgb;
    let top = a + (b - a) * ux.z;
    let bottom = c + (d - c) * ux.z;
    return top + (bottom - top) * uy.z;
}

// The phosphor screen at pos (sRGB-encoded): what is lit now or its
// afterglow, whichever is brighter, plus the halo, through the tube's
// tint, spill and tone curve (tube_tone) and its glass.
fn phosphor_screen(pos: vec2<f32>) -> vec3<f32> {
    let at = vec2<i32>(floor(pos - p.origin));
    let size = vec2<i32>(textureDimensions(beam));
    if any(at < vec2<i32>(0)) || any(at >= size) {
        return vec3<f32>(0.0);
    }
    if (p.flags & 32u) != 0u {
        // MAME's picture (mame::gpu), tone-mapped and gamma-encoded.
        return textureLoad(beam, at, 0).rgb;
    }
    let direct = textureLoad(beam, at, 0).rgb + textureLoad(text, at, 0).rgb;
    let glow = max(textureLoad(glow_fast, at, 0).rgb, vec3<f32>(0.0)) + max(textureLoad(glow_slow, at, 0).rgb, vec3<f32>(0.0));
    var shown = tube_tone(max(direct, glow) + halo_at(at), p.look);
    if p.look.w > 0.0 {
        let g = 2.0 * vec2<f32>(at) / vec2<f32>(size) - 1.0;
        let r2 = 0.5 * dot(g, g);
        shown *= 1.0 - p.look.w * r2 * r2;
    }
    return to_srgb(min(shown, vec3<f32>(1.0)));
}

// The host's overlay (its consent prompt): fixed colours, not the
// program's palette. Foreground codes 0 text, 1 key, 2 dim; background
// codes 0 base, 3 band (sRGB).
fn overlay_fg(code: u32) -> vec3<f32> {
    switch code {
        case 1u: { return vec3<f32>(1.0, 0.80, 0.26); }
        case 2u: { return vec3<f32>(0.66, 0.70, 0.78); }
        default: { return vec3<f32>(0.96, 0.96, 0.98); }
    }
}

fn overlay_bg(code: u32) -> vec3<f32> {
    if code == 3u {
        return vec3<f32>(0.20, 0.23, 0.33);
    }
    return vec3<f32>(0.09, 0.10, 0.14);
}

// The overlay's colour at `pos`, or a negative red if `pos` is not in it.
fn overlay(pos: vec2<f32>) -> vec3<f32> {
    let first = p.flags >> 8u;
    let rel = (pos - p.origin) / p.size;
    if first == 0u || any(rel < vec2<f32>(0.0)) || any(rel >= vec2<f32>(1.0)) {
        return vec3<f32>(-1.0);
    }
    let g = rel * vec2<f32>(p.grid);
    let cell = min(vec2<u32>(g), p.grid - 1u);
    if cell.y < first {
        return vec3<f32>(-1.0);
    }
    let px = min(vec2<u32>(fract(g) * vec2<f32>(p.cell_px)), p.cell_px - 1u);
    let c = textureLoad(cells, cell, 0);
    let slot = (c.r | (c.g << 8u)) & 0x7FFFu;
    let origin = vec2<u32>(slot % p.atlas_cols, slot / p.atlas_cols) * p.cell_px;
    let coverage = textureLoad(atlas, origin + px, 0).r;
    return mix(overlay_bg(c.a), overlay_fg(c.b), coverage);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Glyph mode only (not the background or light passes).
    if (p.flags & 20u) == 0u && (p.flags >> 8u) != 0u {
        var o = overlay(pos.xy);
        if o.r >= 0.0 {
            if (p.flags & 2u) != 0u {
                o = to_linear(o);
            }
            return vec4<f32>(o, 1.0);
        }
    }
    if (p.flags & 16u) != 0u {
        return vec4<f32>(GLYPH * glyph_light(pos.xy), 0.0);
    }
    if (p.flags & 8u) != 0u {
        var screen = phosphor_screen(pos.xy);
        if (p.flags & 2u) != 0u {
            screen = to_linear(screen);
        }
        return vec4<f32>(screen, 1.0);
    }
    let rel = (pos.xy - p.origin) / p.size;
    // Outside the grid (letterbox): palette entry 0.
    var color = palette_rgb(0u);
    let background = (p.flags & 4u) != 0u;
    if all(rel >= vec2<f32>(0.0)) && all(rel < vec2<f32>(1.0)) {
        let g = rel * vec2<f32>(p.grid);
        let cell = min(vec2<u32>(g), p.grid - 1u);
        let px = min(vec2<u32>(fract(g) * vec2<f32>(p.cell_px)), p.cell_px - 1u);
        let c = textureLoad(cells, cell, 0);
        if background {
            // sRGB-encoded, unconverted: the layer is not an sRGB format.
            return vec4<f32>(palette_rgb(c.a), 1.0);
        }
        let slot = (c.r | (c.g << 8u)) & 0x7FFFu;
        let origin = vec2<u32>(slot % p.atlas_cols, slot / p.atlas_cols) * p.cell_px;
        var coverage = textureLoad(atlas, origin + px, 0).r;
        let underline = max(p.cell_px.y / 10u, 1u);
        if (p.flags & 1u) != 0u && all(cell == p.cursor) && px.y >= p.cell_px.y - underline {
            coverage = 1.0;
        }
        // Blend in sRGB space, as terminals usually do.
        let below = textureLoad(under, vec2<u32>(pos.xy), 0).rgb;
        color = mix(below, palette_rgb(c.b), coverage);
    }
    if (p.flags & 2u) != 0u && !background {
        color = to_linear(color);
    }
    return vec4<f32>(color, 1.0);
}
"#;

/// Uniform block; layout matches `Params` in the shader.
#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    origin: [f32; 2],
    size: [f32; 2],
    cell_px: [u32; 2],
    grid: [u32; 2],
    cursor: [u32; 2],
    atlas_cols: u32,
    flags: u32,
    look: [f32; 4],
}

impl Params {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        for v in self.origin.iter().chain(&self.size) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.cell_px.iter().chain(&self.grid).chain(&self.cursor) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.atlas_cols.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for v in self.look {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// Maps code points to atlas slots, rasterising glyphs on first use. A wide
/// glyph takes two consecutive slots, its left and right halves.
struct Atlas {
    slots: HashMap<(u16, bool), u16>,
    next: usize,
}

/// A device and its shader modules, each compiled the first time it is
/// asked for: every machine's screen has a renderer of its own (SPEC §machines.screens),
/// and the renderers of one device share their shaders through clones of
/// this. The modules are kept with their device, not in a table keyed by
/// it: wgpu's device ids are per instance, so a second instance's device
/// can compare equal to the first's.
#[derive(Clone)]
pub struct Shaders {
    device: wgpu::Device,
    modules: std::sync::Arc<std::sync::Mutex<HashMap<&'static str, wgpu::ShaderModule>>>,
    /// MAME's effects on this device (the phosphor style's MAME model),
    /// made the first time they are needed.
    mame: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<crate::mame::gpu::Gpu>>>>,
}

impl Shaders {
    pub fn new(device: &wgpu::Device) -> Self {
        Shaders { device: device.clone(), modules: Default::default(), mame: Default::default() }
    }

    /// MAME's effects on this device (`mame::gpu`).
    pub fn mame(&self, queue: &wgpu::Queue) -> std::sync::Arc<crate::mame::gpu::Gpu> {
        self.mame
            .lock()
            .unwrap()
            .get_or_insert_with(|| std::sync::Arc::new(crate::mame::gpu::Gpu::on(self.device.clone(), queue.clone(), String::new(), false)))
            .clone()
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The shader module `label`, compiled from `source` the first time.
    pub fn module(&self, label: &'static str, source: impl FnOnce() -> String) -> wgpu::ShaderModule {
        let mut modules = self.modules.lock().unwrap();
        modules
            .entry(label)
            .or_insert_with(|| {
                self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(label),
                    source: wgpu::ShaderSource::Wgsl(source().into()),
                })
            })
            .clone()
    }
}

pub struct Renderer {
    shaders: Shaders,
    glyphs: Box<dyn GlyphSource>,
    atlas: Atlas,
    cell_px: (u32, u32),
    target_is_srgb: bool,
    /// Background mode, drawing into the layer.
    background_pipeline: wgpu::RenderPipeline,
    background_bind_group: wgpu::BindGroup,
    background_params: wgpu::Buffer,
    /// Glyph mode, drawing into the target.
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    /// Glyph light mode, drawing into the text texture.
    light_pipeline: wgpu::RenderPipeline,
    light_bind_group: wgpu::BindGroup,
    light_params: wgpu::Buffer,
    /// The halo's pipelines (`halo.rs`); its textures are in `PhosphorTextures`.
    halo: HaloPipelines,
    /// The layer of backgrounds and shapes, its size, and the glyph-mode
    /// bind group that reads it; made for the viewport on first use.
    layer: Option<(((u32, u32), (u32, u32)), wgpu::TextureView, wgpu::BindGroup)>,
    /// The phosphor textures (SPEC §vector.phosphor), the size of the grid rectangle of
    /// the layer's viewport; made with the layer.
    phosphor: Option<PhosphorTextures>,
    vectors: VectorLayer,
    /// The page device (SPEC §page): between the backgrounds and the shapes.
    page: PageLayer,
    /// The presentation: phosphor or clean, and the persistence.
    style: (bool, u8),
    /// How the phosphor style is drawn (`--tube`, `--vignette`).
    look: phosphor::Look,
    /// The user turned trails off (persistence treated as 0).
    no_trails: bool,
    /// `--force-phosphor`: the phosphor presentation whatever the style.
    force_phosphor: bool,
    /// The afterglow must be cleared before the next frame's light.
    phosphor_stale: bool,
    cell_texture: wgpu::Texture,
    atlas_texture: wgpu::Texture,
    palette_texture: wgpu::Texture,
    cell_data: Vec<u8>,
    glyph_scratch: Vec<u8>,
    cursor: Option<(u32, u32)>,
    /// The first row of the host's overlay (its prompt), drawn over
    /// everything in its own colours; 0 for none.
    overlay: u32,
    /// Where the screen is drawn in the target, if not the grid rectangle
    /// of the whole viewport (a machine's tile, SPEC §machines.screens).
    rect: Option<(f32, f32, f32, f32)>,
    /// The glyph pass clears the target first (the screen drawn first).
    clears: bool,
    /// The sources of the machine's Surfaces (screen units), which give no
    /// phosphor light on its glass (SPEC §vector3d.presentation).
    excluded: Vec<[u16; 4]>,
    /// 1 × 1 stand-ins for the textures a pass does not read.
    unused: wgpu::Texture,
    unused_phosphor: wgpu::Texture,
    /// The MAME model (`mame::gpu`): its tube and what it was made for,
    /// made in the first frame of light after the layer.
    mame: Option<(MameKey, crate::mame::gpu::Crt)>,
    /// The terminal's text drawn by the beam (as `soft::screen`).
    strokes: Vec<crate::mame::text::Lit>,
    stroked: Vec<bool>,
    stroke_key: u64,
}

/// What a MAME tube is made for: the picture's size, the phosphor's, the
/// sliders, the bloom, the vignette.
#[derive(Clone, Copy, PartialEq)]
struct MameKey {
    size: (u32, u32),
    internal: (u32, u32),
    params: crate::mame::Params,
    bloom: bool,
    vignette: f32,
}

/// The phosphor textures: beam, text and the afterglow's components, the
/// bind group that deposits beam and text, and the halo's pyramid.
struct PhosphorTextures {
    size: (u32, u32),
    beam: wgpu::TextureView,
    text: wgpu::TextureView,
    fast: wgpu::TextureView,
    slow: wgpu::TextureView,
    beam_group: wgpu::BindGroup,
    halo: HaloChain,
    /// The MAME model's picture (its tube's composite, copied here), bound
    /// in place of the beam; `None` for the classic model.
    picture: Option<wgpu::Texture>,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Writes a `size` rectangle at `origin`; `bpp` is bytes per texel.
fn write_texture(
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    origin: (u32, u32),
    size: (u32, u32),
    bpp: u32,
    data: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d { x: origin.0, y: origin.1, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0 * bpp), rows_per_image: None },
        wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
    );
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl Renderer {
    pub fn new(
        shaders: &Shaders,
        target_format: wgpu::TextureFormat,
        glyphs: Box<dyn GlyphSource>,
    ) -> Self {
        let device = shaders.device();
        let cell_px = glyphs.cell_size();
        let cell_texture = texture(device, "cells", (COLS as u32, ROWS as u32), wgpu::TextureFormat::Rgba8Uint);
        let atlas_texture = texture(
            device,
            "glyph atlas",
            (ATLAS_COLS * cell_px.0, ATLAS_ROWS * cell_px.1),
            wgpu::TextureFormat::R8Unorm,
        );
        let palette_texture = texture(device, "palette", (256, 1), wgpu::TextureFormat::Rgba8Unorm);
        let uniform = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params_buffer = uniform("params");
        let background_params = uniform("background params");
        let light_params = uniform("glyph light params");

        let float = wgpu::TextureSampleType::Float { filterable: false };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terminal"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1, wgpu::TextureSampleType::Uint),
                texture_entry(2, float),
                texture_entry(3, float),
                texture_entry(4, float),
                texture_entry(5, float),
                texture_entry(6, float),
                texture_entry(7, float),
                texture_entry(8, float),
                texture_entry(9, float),
            ],
        });
        // Background and glyph light modes never read `under` or the
        // phosphor textures; they get 1 × 1 stand-ins.
        let unused = texture(device, "unused layer", (1, 1), LAYER_FORMAT);
        let unused_phosphor = texture(device, "unused phosphor", (1, 1), PHOSPHOR_FORMAT);
        let stand_ins = |params| {
            let u = &unused_phosphor;
            Self::bind_group(device, &layout, params, [&cell_texture, &atlas_texture, &palette_texture, &unused, u, u, u, u, u])
        };
        let background_bind_group = stand_ins(&background_params);
        let light_bind_group = stand_ins(&light_params);

        let shader = shaders.module("terminal", || format!("{}{SHADER}", phosphor::wgsl()));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminal"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline_for = |format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("terminal"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let glyph_bytes = (2 * cell_px.0 * cell_px.1) as usize;
        Renderer {
            glyphs,
            atlas: Atlas { slots: HashMap::new(), next: 1 },
            cell_px,
            target_is_srgb: target_format.is_srgb(),
            background_pipeline: pipeline_for(LAYER_FORMAT),
            background_bind_group,
            background_params,
            pipeline: pipeline_for(target_format),
            bind_group_layout: layout,
            params_buffer,
            light_pipeline: pipeline_for(PHOSPHOR_FORMAT),
            light_bind_group,
            light_params,
            halo: HaloPipelines::new(shaders),
            layer: None,
            phosphor: None,
            vectors: VectorLayer::new(shaders, LAYER_FORMAT),
            page: PageLayer::new(shaders, LAYER_FORMAT),
            shaders: shaders.clone(),
            style: (false, 0),
            look: phosphor::Look::default(),
            no_trails: false,
            force_phosphor: false,
            phosphor_stale: true,
            cell_texture,
            atlas_texture,
            palette_texture,
            cell_data: vec![0; COLS * ROWS * 4],
            glyph_scratch: vec![0; glyph_bytes],
            cursor: None,
            overlay: 0,
            rect: None,
            clears: true,
            excluded: Vec::new(),
            unused,
            unused_phosphor,
            mame: None,
            strokes: Vec::new(),
            stroked: Vec::new(),
            stroke_key: 0,
        }
    }

    /// Whether this screen is drawn first, clearing the target (else over
    /// what the target holds).
    pub fn set_clears(&mut self, clears: bool) {
        self.clears = clears;
    }

    /// The sources of the machine's Surfaces (SPEC §vector3d.presentation): in the phosphor
    /// style they give no light (the clean style's glass is blacked out
    /// there by whoever draws the Surfaces).
    pub fn set_excluded(&mut self, rects: &[[u16; 4]]) {
        if self.excluded != rects {
            self.excluded = rects.to_vec();
        }
    }

    /// The glyph source for cells `width` pixels wide (for another screen).
    /// The device and its shaders, for the other renderers on it.
    pub fn shaders(&self) -> &Shaders {
        &self.shaders
    }

    pub fn glyphs_for(&self, width: u32) -> Box<dyn GlyphSource> {
        self.glyphs.resized(width)
    }

    /// Draws the screen into `rect` (pixels of the target) instead of the
    /// grid rectangle of the viewport, with glyphs rasterised for that size;
    /// `None` goes back to the viewport's grid rectangle.
    pub fn set_rect(&mut self, device: &wgpu::Device, rect: Option<(f32, f32, f32, f32)>) {
        self.rect = rect;
        if let Some((_, _, w, _)) = rect {
            // Whole pixels per cell, never more than the cell shown: a glyph
            // is then drawn at its size or a fraction of a pixel larger.
            self.set_cell_width(device, ((w / COLS as f32).floor() as u32).max(2));
        }
    }

    /// Rasterises the glyphs for cells `width` pixels wide from now on.
    fn set_cell_width(&mut self, device: &wgpu::Device, width: u32) {
        if self.cell_px.0 == width {
            return;
        }
        self.glyphs = self.glyphs.resized(width);
        self.cell_px = self.glyphs.cell_size();
        let (w, h) = self.cell_px;
        self.atlas_texture = texture(device, "glyph atlas", (ATLAS_COLS * w, ATLAS_ROWS * h), wgpu::TextureFormat::R8Unorm);
        self.atlas = Atlas { slots: HashMap::new(), next: 1 };
        self.glyph_scratch = vec![0; (2 * w * h) as usize];
        let u = &self.unused_phosphor;
        let textures = [&self.cell_texture, &self.atlas_texture, &self.palette_texture, &self.unused, u, u, u, u, u];
        self.background_bind_group = Self::bind_group(device, &self.bind_group_layout, &self.background_params, textures);
        self.light_bind_group = Self::bind_group(device, &self.bind_group_layout, &self.light_params, textures);
        // The layer's bind groups read the atlas too.
        self.layer = None;
        self.phosphor = None;
    }

    /// The rectangle drawn into, as a scissor in whole pixels of a
    /// `viewport` target; `None` if nothing of it is inside.
    fn scissor(&self, viewport: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
        let (x, y, w, h) = self.grid_rect(viewport);
        let x0 = (x.max(0.0).floor() as u32).min(viewport.0);
        let y0 = (y.max(0.0).floor() as u32).min(viewport.1);
        let x1 = ((x + w).ceil().max(0.0) as u32).min(viewport.0);
        let y1 = ((y + h).ceil().max(0.0) as u32).min(viewport.1);
        (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
    }

    /// The terminal shader's bind group: params, cells, atlas, palette, the
    /// layer below the glyphs and the phosphor textures (beam, fast, slow,
    /// the halo and the text).
    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        textures: [&wgpu::Texture; 9],
    ) -> wgpu::BindGroup {
        let views = textures.map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        Self::bind_group_of(device, layout, params, views)
    }

    /// The same with views.
    fn bind_group_of(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        views: [wgpu::TextureView; 9],
    ) -> wgpu::BindGroup {
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }];
        for (i, v) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: i as u32 + 1, resource: wgpu::BindingResource::TextureView(v) });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("terminal"), layout, entries: &entries })
    }

    /// Takes the vector display's style (SPEC §vector.phosphor) and uploads its list
    /// (SPEC §vector) as `Vector::shapes` gives it (outlines in the phosphor
    /// presentation even if only forced), unless the list's generation shows
    /// it is unchanged, and the 3D list's segments over it (SPEC §vector3d).
    /// `vector` `None`: the image has no vector display.
    pub fn update_vectors(&mut self, queue: &wgpu::Queue, scene: &VectorScene) {
        let vector = scene.vector;
        let persistence = vector.map_or(terse_core::vector::DEFAULT_PERSISTENCE, Vector::persistence);
        let style = (self.force_phosphor || vector.is_some_and(Vector::phosphor), persistence);
        if style.0 != self.style.0 {
            // Switching style starts the phosphor afresh.
            self.phosphor_stale = true;
        }
        self.style = style;
        let outlines = outlined(style.0, vector);
        self.page.set_outlines(outlines);
        self.vectors.update(queue, scene, outlines);
    }

    /// Takes the page device's state and the layout to show (once per
    /// machine frame, after `update_vectors`).
    pub fn update_page(&mut self, page: Option<&terse_core::Page>, layout: Option<std::sync::Arc<HostLayout>>) {
        self.page.set(page, layout);
    }

    /// What the page's figures did since the last call (`TERSE_PAGE_TRACE`).
    pub fn take_page_figure_log(&mut self) -> Vec<String> {
        self.page.take_figure_log()
    }

    /// Whether the page's scroll is animated (not for screenshots).
    pub fn set_page_animation(&mut self, on: bool) {
        self.page.set_animate(on);
    }

    /// The page's anchor under a position in screen units, as shown.
    pub fn page_anchor_at_units(&self, x: f32, y: f32) -> u16 {
        self.page.anchor_at(x, y)
    }

    /// Treat the persistence as 0 (the user turned trails off, SPEC §vector.phosphor).
    pub fn set_no_trails(&mut self, no_trails: bool) {
        self.no_trails = no_trails;
    }

    /// How the phosphor style is drawn: the tube's preset and its glass.
    pub fn set_look(&mut self, look: phosphor::Look) {
        if look.stroke_font() != self.look.stroke_font() {
            self.stroke_key = 0;
        }
        self.look = look;
    }

    /// Show the phosphor presentation whatever the program's style (for
    /// testing). Takes effect at the next `update_vectors`.
    pub fn set_force_phosphor(&mut self, force: bool) {
        self.force_phosphor = force;
    }

    /// The MAME model's sliders when this screen is drawn with it (the
    /// phosphor style): a scene over it draws its lines by the beam.
    pub fn beam_params(&self) -> Option<crate::mame::Params> {
        (self.style.0 && self.look.model == phosphor::Model::Mame).then(|| self.look.mame_params(self.no_trails))
    }

    /// Whether the phosphor presentation is in use.
    pub fn phosphor(&self) -> bool {
        self.style.0
    }

    /// Phosphor presentation: one machine frame of light (`phosphor.rs`)
    /// for `viewport`, submitted at once: the beam from the terminal and the
    /// list uploaded last, and the afterglow faded and deposited. Call once
    /// per machine frame after `update` and `update_vectors`, so trails do
    /// not depend on how often frames are presented. Does nothing in the
    /// clean style.
    pub fn accumulate(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport: (u32, u32)) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("phosphor") });
        self.phosphor_frame(device, queue, &mut encoder, viewport, true);
        queue.submit([encoder.finish()]);
    }

    /// `accumulate`, recorded into `encoder` (several screens' light in
    /// one submission).
    pub fn accumulate_into(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        viewport: (u32, u32),
    ) {
        self.phosphor_frame(device, queue, encoder, viewport, true);
    }

    /// Phosphor presentation: redraws the beam from the terminal and list
    /// uploaded since the last `accumulate`, without advancing the afterglow
    /// (e.g. to show a fault reported after the last machine frame).
    pub fn refresh_beam(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport: (u32, u32)) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("phosphor") });
        self.phosphor_frame(device, queue, &mut encoder, viewport, false);
        queue.submit([encoder.finish()]);
    }

    fn phosphor_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        viewport: (u32, u32),
        advance: bool,
    ) {
        if !self.style.0 {
            return;
        }
        self.layer(device, viewport);
        let Some(t) = &self.phosphor else { return };
        let (_, _, w, h) = self.grid_rect(viewport);
        let (cursor, shown) = self.cursor.map_or(((0, 0), 0), |c| (c, 1));
        let params = Params {
            origin: [0.0, 0.0],
            size: [w, h],
            cell_px: [self.cell_px.0, self.cell_px.1],
            grid: [COLS as u32, ROWS as u32],
            cursor: [cursor.0, cursor.1],
            atlas_cols: ATLAS_COLS,
            flags: shown | 16 | if self.look.stroke_font().is_some() { 64 } else { 0 },
            look: self.look.uniform(),
        };
        queue.write_buffer(&self.light_params, 0, &params.bytes());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glyph light"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.text,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.light_pipeline);
            pass.set_bind_group(0, &self.light_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        // The page's text is light on the same tube (SPEC §vector.phosphor); no background.
        self.page.prepare(device, queue, (0.0, 0.0, w, h), t.size, true);
        if self.page.quads() > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("page light"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.text,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.page.draw(&mut pass, true);
        }
        if self.look.model == phosphor::Model::Mame {
            return self.mame_frame(queue, encoder, (w, h));
        }
        let persistence = if self.no_trails { 0 } else { self.style.1 };
        let targets = PhosphorTargets { beam: &t.beam, text: &t.text, beam_group: &t.beam_group, fast: &t.fast, slow: &t.slow };
        let clear = self.phosphor_stale;
        let fast_decay = self.look.preset.fast_decay();
        let shed = self.vectors.shed_level(&self.look);
        self.vectors.phosphor_frame(queue, encoder, &targets, t.size, persistence, fast_decay, clear, advance, shed);
        if advance {
            self.phosphor_stale = false;
        }
        // No light where the machine's Surfaces show its screen.
        let size = t.size;
        let rects: Vec<(u32, u32, u32, u32)> = self
            .excluded
            .iter()
            .filter_map(|&r| {
                let (x0, y0, x1, y1) = crate::display::source_pixels((0.0, 0.0, w, h), r);
                let (x0, y0) = (x0.clamp(0, size.0 as i32) as u32, y0.clamp(0, size.1 as i32) as u32);
                let (x1, y1) = (x1.clamp(0, size.0 as i32) as u32, y1.clamp(0, size.1 as i32) as u32);
                (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
            })
            .collect();
        self.vectors.mask(encoder, &targets, &rects);
        // The halo of all that light, unless the frame sheds it (the GPU
        // draws plain lines' level as this one: its beam costs it little).
        if shed >= crate::phosphor::SHED_HALO {
            t.halo.clear(encoder);
        } else {
            t.halo.encode(&self.halo, encoder);
        }
    }

    /// The MAME model's frame (`mame::gpu`): the shapes' beams and the
    /// terminal's strokes into its tube (timed each as MAME times a display
    /// list), its bloom and composite with the text's light, the picture
    /// copied to where the glyph pass reads it.
    fn mame_frame(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, (w, h): (f32, f32)) {
        use crate::mame::beams::{Line, Screen, instances};
        let Some(t) = &self.phosphor else { return };
        let Some(picture) = &t.picture else { return };
        let size = t.size;
        let level = self.vectors.shed_level(&self.look);
        let (max_lines, bloom) = crate::soft::mame::lines_at(level);
        let internal = crate::soft::mame::internal_size((size.0 as usize, size.1 as usize), max_lines);
        let internal = (internal.0 as u32, internal.1 as u32);
        let key = MameKey {
            size,
            internal,
            params: self.look.mame_params(self.no_trails),
            bloom,
            vignette: if self.look.vignette { phosphor::MAME_VIGNETTE } else { 0.0 },
        };
        let gpu = self.shaders.mame(queue);
        if self.phosphor_stale || self.mame.as_ref().is_none_or(|(k, _)| *k != key) {
            let extras = crate::mame::gpu::Extras { readback: false, text: Some(&t.text), text_gain: crate::mame::params::TEXT_GAIN, vignette: key.vignette };
            self.mame = Some((key, crate::mame::gpu::Crt::with(&gpu, size, internal, key.params, bloom, extras)));
            self.phosphor_stale = false;
        }
        let look = self.look;
        let screen = Screen { size: internal };
        let shapes: Vec<terse_core::Primitive> = if look.preset.tint().is_some() {
            self.vectors.shapes().iter().map(|p| crate::soft::screen::tint_primitive(p, &look)).collect()
        } else {
            self.vectors.shapes().to_vec()
        };
        let mut lines = Vec::new();
        screen.lines(&shapes, &mut lines);
        let mut beams = instances(&lines, key.params.beam_width);
        if !self.strokes.is_empty() {
            let text: Vec<Line> = self
                .strokes
                .iter()
                .map(|l| {
                    let st = l.s;
                    let rgba = look.tint_bytes([l.rgb[0], l.rgb[1], l.rgb[2], (st.level * 255.0).round() as u8]);
                    screen.line(st.x0 * 16.0, st.y0 * 16.0, st.x1 * 16.0, st.y1 * 16.0, st.width * 16.0, rgba, 1.0)
                })
                .collect();
            beams.extend(instances(&text, key.params.beam_width));
        }
        // The sources of the machine's Surfaces do not bloom: their
        // rectangles in the bloom's half-size target.
        let half = (internal.0.div_ceil(2) as f32 / size.0 as f32, internal.1.div_ceil(2) as f32 / size.1 as f32);
        let no_bloom: Vec<(u32, u32, u32, u32)> = self
            .excluded
            .iter()
            .filter_map(|&r| {
                let (x0, y0, x1, y1) = crate::display::source_pixels((0.0, 0.0, w, h), r);
                let c = |v: i32, n: u32| v.clamp(0, n as i32) as u32;
                let (x0, y0, x1, y1) = (c(x0, size.0), c(y0, size.1), c(x1, size.0), c(y1, size.1));
                let (a, b) = ((x0 as f32 * half.0) as u32, (y0 as f32 * half.1) as u32);
                let (cx, d) = ((x1 as f32 * half.0).ceil() as u32, (y1 as f32 * half.1).ceil() as u32);
                (cx > a && d > b).then_some((a, b, cx - a, d - b))
            })
            .collect();
        let (_, crt) = self.mame.as_mut().expect("made above");
        crt.encode(&gpu, encoder, &beams, FRAME_SECONDS_F64, &no_bloom);
        encoder.copy_texture_to_texture(
            crt.output_texture().as_image_copy(),
            picture.as_image_copy(),
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
    }

    /// Size of the whole grid at the atlas's native cell width: 4:3, so that
    /// screen units are square (SPEC §screen) whatever the glyph source's cell.
    pub fn grid_size(&self) -> (u32, u32) {
        let width = COLS as u32 * self.cell_px.0;
        (width, width * SCREEN_HEIGHT as u32 / SCREEN_WIDTH as u32)
    }

    /// Uploads the terminal state (SPEC §terminal.presentation: presentation is up to the host).
    pub fn update(&mut self, queue: &wgpu::Queue, terminal: &Terminal) {
        if self.atlas.next + COLS * ROWS > ATLAS_SLOTS {
            // Atlas full: start over; the screen needs at most COLS * ROWS slots.
            self.atlas.slots.clear();
            self.atlas.next = 1;
        }
        // SPEC §terminal.cells, §terminal.text: wide characters across two
        // cells, or, without full text, the basic repertoire and the placeholder.
        let cells = terse_core::text::draw_cells(terminal, |code, wide| self.slot_for(queue, code, wide) as u32);
        // The MAME model's text by the beam (as soft::screen): the cells it
        // draws are marked (bit 15 of the slot), and not drawn as light.
        match self.look.stroke_font().filter(|_| self.style.0 || self.force_phosphor) {
            Some(font) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                terminal.cell_bytes().hash(&mut h);
                terminal.palette().hash(&mut h);
                (terminal.cursor_visible(), terminal.cursor()).hash(&mut h);
                terminal.full_text().hash(&mut h);
                font.name().hash(&mut h);
                let key = h.finish() | 1;
                if key != self.stroke_key {
                    self.stroke_key = key;
                    self.strokes = crate::mame::text::screen_covered(terminal, font, &mut self.stroked);
                }
            }
            None => {
                self.strokes.clear();
                self.stroked.clear();
                self.stroke_key = 0;
            }
        }
        for (i, &(slot, fg, bg)) in cells.iter().enumerate() {
            let stroked = self.stroked.get(i).copied().unwrap_or(false);
            let [lo, hi] = (slot as u16 | if stroked { 0x8000 } else { 0 }).to_le_bytes();
            self.cell_data[i * 4..i * 4 + 4].copy_from_slice(&[lo, hi, fg, bg]);
        }
        write_texture(queue, &self.cell_texture, (0, 0), (COLS as u32, ROWS as u32), 4, &self.cell_data);

        let palette: Vec<u8> = terminal.palette().iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
        write_texture(queue, &self.palette_texture, (0, 0), (256, 1), 4, &palette);

        let (col, row) = terminal.cursor();
        self.cursor = terminal.cursor_visible().then_some((col as u32, row as u32));
    }

    /// Draws the rows from `first` on as the host's overlay: the cells as
    /// uploaded, in the overlay's own colours, over the page and shapes.
    pub fn set_overlay(&mut self, first: Option<usize>) {
        self.overlay = first.map_or(0, |r| r.clamp(1, ROWS - 1) as u32);
    }

    /// The atlas slot for `code` (for a wide glyph, the slot of its left half).
    fn slot_for(&mut self, queue: &wgpu::Queue, code: u16, wide: bool) -> u16 {
        let placeholder = code == terse_core::text::PLACEHOLDER_CODE;
        let ch = match char::from_u32(code as u32) {
            Some(c) if c != ' ' => c,
            _ => return BLANK_SLOT,
        };
        if let Some(&slot) = self.atlas.slots.get(&(code, wide)) {
            return slot;
        }
        let cells = 1 + wide as u32;
        let slot = self.atlas.next as u16;
        self.atlas.next += cells as usize;
        let (w, h) = self.cell_px;
        let glyph = &mut self.glyph_scratch[..(cells * w * h) as usize];
        glyph.fill(0);
        if placeholder {
            terse_core::text::placeholder(w, h, glyph);
        } else {
            self.glyphs.rasterize(ch, cells, glyph);
        }
        let mut half = vec![0; (w * h) as usize];
        for i in 0..cells {
            for (y, row) in half.chunks_exact_mut(w as usize).enumerate() {
                let start = (y as u32 * cells * w + i * w) as usize;
                row.copy_from_slice(&glyph[start..start + w as usize]);
            }
            let s = slot as u32 + i;
            let origin = (s % ATLAS_COLS * w, s / ATLAS_COLS * h);
            write_texture(queue, &self.atlas_texture, origin, (w, h), 1, &half);
        }
        self.atlas.slots.insert((code, wide), slot);
        slot
    }

    fn grid_rect(&self, viewport: (u32, u32)) -> (f32, f32, f32, f32) {
        self.rect.unwrap_or_else(|| grid_rect(self.grid_size(), viewport))
    }

    /// The rectangle the screen is drawn in, in viewport pixels.
    pub fn screen_rect(&self, viewport: (u32, u32)) -> (f32, f32, f32, f32) {
        self.grid_rect(viewport)
    }

    /// A viewport position in screen units (SPEC §screen), through the same
    /// transform that places the grid, clamped to the screen.
    pub fn screen_pos(&self, viewport: (u32, u32), x: f64, y: f64) -> (u16, u16) {
        screen_pos(self.grid_rect(viewport), x, y)
    }

    /// The height of one terminal row in viewport pixels.
    pub fn row_height(&self, viewport: (u32, u32)) -> f64 {
        (self.grid_rect(viewport).3 / ROWS as f32) as f64
    }

    /// What the layer and the phosphor textures are made for: the
    /// viewport and the size of the grid rectangle in whole pixels.
    fn layer_key(&self, viewport: (u32, u32)) -> ((u32, u32), (u32, u32)) {
        let (_, _, w, h) = self.grid_rect(viewport);
        (viewport, ((w.ceil() as u32).max(1), (h.ceil() as u32).max(1)))
    }

    /// The layer for `viewport` and the glyph-mode bind group reading it.
    fn layer(&mut self, device: &wgpu::Device, viewport: (u32, u32)) -> (wgpu::TextureView, wgpu::BindGroup) {
        // The phosphor textures cover the grid rectangle, which starts at a
        // whole pixel; new ones start dark.
        let size = self.layer_key(viewport).1;
        let mame = self.look.model == phosphor::Model::Mame;
        if let Some((key, view, group)) = &self.layer
            && *key == (viewport, size)
            && self.phosphor.as_ref().is_some_and(|t| t.picture.is_some() == mame)
        {
            return (view.clone(), group.clone());
        }
        // The MAME model draws its own phosphor: the classic model's
        // textures are stand-ins, its picture is bound in the beam's place.
        let classic = if mame { (1, 1) } else { size };
        let phosphor_sized = |label, size: (u32, u32)| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: PHOSPHOR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let (beam, fast, slow) = (phosphor_sized("phosphor beam", classic), phosphor_sized("afterglow fast", classic), phosphor_sized("afterglow slow", classic));
        let text = phosphor_sized("phosphor text", size);
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let (beam_view, text_view, fast_view, slow_view) = (view(&beam), view(&text), view(&fast), view(&slow));
        let halo = HaloChain::new(device, &self.halo, classic, &beam_view, &text_view, &fast_view, &slow_view, &view(&self.unused_phosphor));
        let picture = mame.then(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mame picture"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::mame::gpu::OUTPUT_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        self.mame = None;
        let halo_view = halo.halo.clone();
        self.phosphor = Some(PhosphorTextures {
            size,
            beam_group: self.vectors.beam_bind_group(device, &beam_view, &text_view),
            beam: beam_view.clone(),
            text: text_view.clone(),
            fast: fast_view.clone(),
            slow: slow_view.clone(),
            halo,
            picture: picture.clone(),
        });
        self.phosphor_stale = true;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer"),
            size: wgpu::Extent3d { width: viewport.0, height: viewport.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LAYER_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let fixed = [&self.cell_texture, &self.atlas_texture, &self.palette_texture, &tex].map(view);
        let [c, a, p, l] = fixed;
        let group = Self::bind_group_of(
            device,
            &self.bind_group_layout,
            &self.params_buffer,
            [c, a, p, l, picture.as_ref().map_or(beam_view, view), fast_view, slow_view, halo_view, text_view],
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.layer = Some(((viewport, size), view.clone(), group.clone()));
        (view, group)
    }

    /// Records the draw into `encoder`, covering all of `target`
    /// (`viewport` pixels): backgrounds, shapes, then glyphs (SPEC §vector.layers).
    /// With a rectangle set (`set_rect`), only that rectangle is drawn,
    /// over what the target holds.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) {
        let scissor = if self.rect.is_some() {
            match self.scissor(viewport) {
                Some(s) => Some(s),
                None => return,
            }
        } else {
            None
        };
        let rect = self.grid_rect(viewport);
        let (x, y, w, h) = rect;
        let (cursor, shown) = self.cursor.map_or(((0, 0), 0), |c| (c, 1));
        let params = Params {
            origin: [x, y],
            size: [w, h],
            cell_px: [self.cell_px.0, self.cell_px.1],
            grid: [COLS as u32, ROWS as u32],
            cursor: [cursor.0, cursor.1],
            atlas_cols: ATLAS_COLS,
            flags: shown
                | if self.target_is_srgb { 2 } else { 0 }
                | if self.style.0 { 8 } else { 0 }
                | if self.style.0 && self.look.model == phosphor::Model::Mame { 32 } else { 0 },
            look: self.look.uniform(),
        };
        let glyph_params = Params { flags: params.flags | self.overlay << 8, ..params };
        queue.write_buffer(&self.params_buffer, 0, &glyph_params.bytes());
        queue.write_buffer(&self.background_params, 0, &Params { flags: 4, ..params }.bytes());
        if self.style.0 && self.layer.as_ref().is_none_or(|(key, ..)| *key != self.layer_key(viewport)) {
            // Resized since the last machine frame: start the new phosphor
            // texture with this frame's light rather than show it dark.
            self.accumulate(device, queue, viewport);
        }
        let (layer, glyph_group) = self.layer(device, viewport);

        let attachment = |view, load| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })
        };
        // Phosphor style: no backgrounds, and the glyphs and shapes are in
        // the phosphor textures.
        if !self.style.0 {
            self.page.prepare(device, queue, rect, viewport, false);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("backgrounds and shapes"),
                color_attachments: &[attachment(&layer, wgpu::LoadOp::Clear(wgpu::Color::BLACK))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.background_pipeline);
            pass.set_bind_group(0, &self.background_bind_group, &[]);
            if let Some((x, y, w, h)) = scissor {
                pass.set_scissor_rect(x, y, w, h);
            }
            pass.draw(0..3, 0..1);
            // SPEC §vector.layers: the page lies between the backgrounds and the shapes.
            self.page.draw(&mut pass, false);
            self.vectors.draw(queue, &mut pass, viewport, rect);
        }
        let load = if self.clears { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("glyphs"),
            color_attachments: &[attachment(target, load)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &glyph_group, &[]);
        if let Some((x, y, w, h)) = scissor {
            pass.set_scissor_rect(x, y, w, h);
        }
        pass.draw(0..3, 0..1);
    }

    /// Draws the host's overlay rows again over what the target holds (the
    /// other machines' screens drawn after this one, SPEC §machines.screens); after
    /// `render` in the same frame.
    pub fn render_overlay(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, viewport: (u32, u32)) {
        let Some((_, _, group)) = &self.layer else { return };
        if self.overlay == 0 {
            return;
        }
        let (x, y, w, h) = self.grid_rect(viewport);
        let top = y + h * self.overlay as f32 / ROWS as f32;
        let x0 = (x.max(0.0).floor() as u32).min(viewport.0);
        let y0 = (top.max(0.0).floor() as u32).min(viewport.1);
        let x1 = ((x + w).ceil() as u32).min(viewport.0);
        let y1 = ((y + h).ceil() as u32).min(viewport.1);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device of a wgpu instance of its own, if this machine has an adapter.
    fn device() -> Option<(wgpu::Instance, wgpu::Device)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let (_, device, _) = pollster::block_on(crate::request_device(Default::default(), &instance)).ok()?;
        Some((instance, device))
    }

    /// The renderers of one device share its shader modules; a device of
    /// another wgpu instance (whose id can equal the first's) gets modules
    /// of its own, and a renderer on it is built without a validation error.
    #[test]
    fn shaders_are_per_device() {
        crate::gpu_test::or_skip("shaders_are_per_device", shaders_are_per_device_checks);
    }

    fn shaders_are_per_device_checks() {
        let Some((_a, first)) = device() else { return terse_testkit::skip!("gpu", "no GPU adapter") };
        let Some((_b, second)) = device() else { return };
        let glyphs = crate::glyphs::TermGlyphs::builtin(crate::FONT_PX);
        let shaders = Shaders::new(&first);
        let one = shaders.module("terminal", || format!("{}{SHADER}", phosphor::wgsl()));
        let again = shaders.clone().module("terminal", || unreachable!("compiled once per device"));
        assert!(one == again);
        let _first = Renderer::new(&shaders, wgpu::TextureFormat::Rgba8UnormSrgb, Box::new(glyphs.clone()));
        let scope = second.push_error_scope(wgpu::ErrorFilter::Validation);
        let _second = Renderer::new(&Shaders::new(&second), wgpu::TextureFormat::Rgba8UnormSrgb, Box::new(glyphs));
        assert!(pollster::block_on(scope.pop()).is_none(), "a renderer on the second device");
    }
}

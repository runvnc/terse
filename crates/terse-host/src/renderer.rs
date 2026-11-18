//! Screen renderer: the terminal and the vector display, combined in the
//! layer order of SPEC 8.6. Independent of windowing and of where glyph
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
//! The phosphor style (SPEC 8.7; the model and its constants are in
//! `phosphor.rs`) treats the whole screen as one tube. Cell backgrounds are
//! not drawn. Once per machine frame (`accumulate`) the glyphs and cursor
//! are drawn as light in their foreground colour (a third mode of the same
//! shader, with a faint halo) into the beam texture, then `vectors.rs`
//! fades the afterglow, adds the shapes to the beam and deposits the beam
//! into the afterglow. Two more modes of the shader blur beam + afterglow
//! (horizontally, then vertically) into the bloom texture. The glyph pass
//! then shows beam + afterglow + bloom through a tone curve on black
//! instead of steps 1-3.

use std::collections::HashMap;

use terse_core::{COLS, CONTINUATION, ROWS, SCREEN_HEIGHT, SCREEN_WIDTH, Terminal, Vector};

use crate::page::{HostLayout, PageLayer};
use crate::phosphor;
use crate::vectors::{PHOSPHOR_FORMAT, PhosphorTargets, VectorLayer};

/// Format of the offscreen layer of backgrounds and shapes.
const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Supplies glyph coverage bitmaps, one or two terminal cells in size.
pub trait GlyphSource {
    /// Cell size in pixels (width, height). Should be exactly 1:2 (SPEC 5.4:
    /// square screen units); the renderer shows the grid at 4:3 regardless,
    /// so glyphs from a cell of another shape are stretched.
    fn cell_size(&self) -> (u32, u32);
    /// Writes the coverage of `ch` spanning `cells` cells (1, or 2 for a wide
    /// character) into `out` (`cells * width * height` bytes, row major,
    /// already zeroed). Unknown characters may be left blank.
    fn rasterize(&mut self, ch: char, cells: u32, out: &mut [u8]);
}

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
                            // bit 4: glyph light mode (into the beam texture),
                            // bit 5: bloom, horizontal pass (from the phosphor light),
                            // bit 6: bloom, vertical pass (from bloom_src),
                            // bits 8-15: the first row of the host's overlay (glyph mode; 0: none)
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var cells: texture_2d<u32>;
@group(0) @binding(2) var atlas: texture_2d<f32>;
@group(0) @binding(3) var palette: texture_2d<f32>;
@group(0) @binding(4) var under: texture_2d<f32>;  // backgrounds and shapes (glyph mode)
// Phosphor light over the grid rectangle, linear: the beam and the
// afterglow's fast and slow components (which may be below zero: dark).
@group(0) @binding(5) var beam: texture_2d<f32>;
@group(0) @binding(6) var glow_fast: texture_2d<f32>;
@group(0) @binding(7) var glow_slow: texture_2d<f32>;
// The bloom (glyph mode), or the horizontal pass's result (vertical pass).
@group(0) @binding(8) var bloom_src: texture_2d<f32>;

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
    let slot = c.r | (c.g << 8u);
    let origin = vec2<u32>(slot % p.atlas_cols, slot / p.atlas_cols) * p.cell_px;
    var coverage = textureLoad(atlas, origin + px, 0).r;
    let underline = max(p.cell_px.y / 10u, 1u);
    if (p.flags & 1u) != 0u && all(cell == p.cursor) && px.y >= p.cell_px.y - underline {
        coverage = 1.0;
    }
    return to_linear(palette_rgb(c.b)) * coverage;
}

// The glyph light at q with a faint bloom around it.
fn glyph_glow(q: vec2<f32>) -> vec3<f32> {
    let r = GLYPH_BLOOM_RADIUS * p.size.x / 640.0;
    var bloom = vec3<f32>(0.0);
    for (var i = 0; i < 8; i++) {
        let a = f32(i) * 0.7853982;
        bloom += glyph_light(q + vec2<f32>(cos(a), sin(a)) * r);
    }
    return GLYPH * glyph_light(q) + GLYPH_BLOOM * bloom / 8.0;
}

fn tone(x: vec3<f32>) -> vec3<f32> {
    let k = TONE_KNEE;
    let u = (x - k) / (1.0 - k);
    let soft = k + (1.0 - k) * u / (1.0 + u);
    return select(soft, x, x <= vec3<f32>(k));
}

// The phosphor light at a texel: beam plus afterglow (whose components may
// be below zero: dark).
fn phosphor_light(at: vec2<i32>) -> vec3<f32> {
    return textureLoad(beam, at, 0).rgb
        + max(textureLoad(glow_fast, at, 0).rgb, vec3<f32>(0.0))
        + max(textureLoad(glow_slow, at, 0).rgb, vec3<f32>(0.0));
}

// What the bloom blurs: the light, whitened where it is brighter than one
// stroke, so a repeated line's glow is whiter.
fn bloom_source(at: vec2<i32>) -> vec3<f32> {
    let light = phosphor_light(at);
    let peak = max(light.r, max(light.g, light.b));
    return light + vec3<f32>(max(peak - BLOOM_WHITE_FROM, 0.0) * BLOOM_WHITE);
}

// One pass of the bloom's Gaussian blur at texel `at`: of bloom_source
// along x, or of bloom_src along y. The phosphor textures are the grid
// rectangle, p.size pixels for 640 screen units across.
fn bloom_pass(at: vec2<i32>, horizontal: bool) -> vec3<f32> {
    var size = vec2<i32>(textureDimensions(bloom_src));
    var dir = vec2<i32>(0, 1);
    if horizontal {
        size = vec2<i32>(textureDimensions(beam));
        dir = vec2<i32>(1, 0);
    }
    let sigma = max(BLOOM_SIGMA * p.size.x / 640.0, 0.5);
    let n = i32(min(ceil(3.0 * sigma), BLOOM_MAX_TAPS));
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var i = -n; i <= n; i++) {
        let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
        total += w;
        let s = at + dir * i;
        if all(s >= vec2<i32>(0)) && all(s < size) {
            if horizontal {
                sum += w * bloom_source(s);
            } else {
                sum += w * textureLoad(bloom_src, s, 0).rgb;
            }
        }
    }
    return sum / total;
}

// The phosphor screen at pos (sRGB-encoded): beam plus afterglow plus bloom,
// white-hot only where it is far brighter than one beam, through the tone
// curve.
fn phosphor_screen(pos: vec2<f32>) -> vec3<f32> {
    let at = vec2<i32>(floor(pos - p.origin));
    let size = vec2<i32>(textureDimensions(beam));
    if any(at < vec2<i32>(0)) || any(at >= size) {
        return vec3<f32>(0.0);
    }
    var light = phosphor_light(at) + BLOOM * textureLoad(bloom_src, at, 0).rgb;
    let peak = max(light.r, max(light.g, light.b));
    light += vec3<f32>(max(peak - SPILL_FROM, 0.0) * SPILL);
    return to_srgb(tone(light));
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
    let slot = c.r | (c.g << 8u);
    let origin = vec2<u32>(slot % p.atlas_cols, slot / p.atlas_cols) * p.cell_px;
    let coverage = textureLoad(atlas, origin + px, 0).r;
    return mix(overlay_bg(c.a), overlay_fg(c.b), coverage);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Glyph mode only (not the background, light or bloom passes).
    if (p.flags & 116u) == 0u && (p.flags >> 8u) != 0u {
        var o = overlay(pos.xy);
        if o.r >= 0.0 {
            if (p.flags & 2u) != 0u {
                o = to_linear(o);
            }
            return vec4<f32>(o, 1.0);
        }
    }
    if (p.flags & 16u) != 0u {
        return vec4<f32>(glyph_glow(pos.xy), 0.0);
    }
    if (p.flags & 96u) != 0u {
        return vec4<f32>(bloom_pass(vec2<i32>(floor(pos.xy)), (p.flags & 32u) != 0u), 0.0);
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
        let slot = c.r | (c.g << 8u);
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
}

impl Params {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(48);
        for v in self.origin.iter().chain(&self.size) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.cell_px.iter().chain(&self.grid).chain(&self.cursor) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.atlas_cols.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out
    }
}

/// Maps code points to atlas slots, rasterising glyphs on first use. A wide
/// glyph takes two consecutive slots, its left and right halves.
struct Atlas {
    slots: HashMap<(u16, bool), u16>,
    next: usize,
}

/// The grid rectangle (x, y, width, height) inside a viewport: the largest
/// integer multiple of the native size `grid` that fits, or a scaled-down fit,
/// centred. The scale is the same on both axes, so a 4:3 `grid` stays 4:3.
fn grid_rect(grid: (u32, u32), viewport: (u32, u32)) -> (f32, f32, f32, f32) {
    let (gw, gh) = grid;
    let fit = (viewport.0 as f32 / gw as f32).min(viewport.1 as f32 / gh as f32);
    let scale = if fit >= 1.0 { fit.floor() } else { fit };
    let (w, h) = (gw as f32 * scale, gh as f32 * scale);
    (((viewport.0 as f32 - w) / 2.0).floor(), ((viewport.1 as f32 - h) / 2.0).floor(), w, h)
}

/// A viewport position in screen units, given the grid rectangle: the grid
/// spans exactly the 640 × 480 screen.
fn screen_pos(rect: (f32, f32, f32, f32), x: f64, y: f64) -> (u16, u16) {
    let (ox, oy, w, h) = rect;
    let unit = |p: f64, origin: f32, size: f32, units: u16| {
        ((p as f32 - origin) / size * units as f32).floor().clamp(0.0, (units - 1) as f32) as u16
    };
    (unit(x, ox, w, SCREEN_WIDTH), unit(y, oy, h, SCREEN_HEIGHT))
}

/// Whether the list is drawn as outlines: in the phosphor presentation
/// (`phosphor`, which may be forced), or as `Vector::outlined` says (fill
/// off in the clean style).
fn outlined(phosphor: bool, vector: Option<&Vector>) -> bool {
    phosphor || vector.is_some_and(Vector::outlined)
}

pub struct Renderer {
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
    /// Glyph light mode, drawing into the beam texture.
    light_pipeline: wgpu::RenderPipeline,
    light_bind_group: wgpu::BindGroup,
    light_params: wgpu::Buffer,
    /// The bloom's two passes (with the glyph light pipeline): their params;
    /// the bind groups are in `PhosphorTextures`.
    bloom_params: [wgpu::Buffer; 2],
    /// The layer of backgrounds and shapes, its size, and the glyph-mode
    /// bind group that reads it; made for the viewport on first use.
    layer: Option<((u32, u32), wgpu::TextureView, wgpu::BindGroup)>,
    /// The phosphor textures (SPEC 8.7), the size of the grid rectangle of
    /// the layer's viewport; made with the layer.
    phosphor: Option<PhosphorTextures>,
    vectors: VectorLayer,
    /// The page device (SPEC 11): between the backgrounds and the shapes.
    page: PageLayer,
    /// The presentation: phosphor or clean, and the persistence.
    style: (bool, u8),
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
}

/// The phosphor textures: beam and the afterglow's components, the bind
/// group that deposits the beam, and the bloom: the horizontal pass's
/// result, the bloom itself and the two passes' bind groups.
struct PhosphorTextures {
    size: (u32, u32),
    beam: wgpu::TextureView,
    fast: wgpu::TextureView,
    slow: wgpu::TextureView,
    beam_group: wgpu::BindGroup,
    blurred: wgpu::TextureView,
    bloom: wgpu::TextureView,
    bloom_groups: [wgpu::BindGroup; 2],
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
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        glyphs: Box<dyn GlyphSource>,
    ) -> Self {
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
                size: 48,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params_buffer = uniform("params");
        let background_params = uniform("background params");
        let light_params = uniform("glyph light params");
        let bloom_params = [uniform("bloom params x"), uniform("bloom params y")];

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
            ],
        });
        // Background and glyph light modes never read `under` or the
        // phosphor textures; they get 1 × 1 stand-ins.
        let unused = texture(device, "unused layer", (1, 1), LAYER_FORMAT);
        let unused_phosphor = texture(device, "unused phosphor", (1, 1), PHOSPHOR_FORMAT);
        let stand_ins = |params| {
            let u = &unused_phosphor;
            Self::bind_group(device, &layout, params, [&cell_texture, &atlas_texture, &palette_texture, &unused, u, u, u, u])
        };
        let background_bind_group = stand_ins(&background_params);
        let light_bind_group = stand_ins(&light_params);

        let source = format!("{}{SHADER}", phosphor::wgsl());
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
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
            bloom_params,
            layer: None,
            phosphor: None,
            vectors: VectorLayer::new(device, LAYER_FORMAT),
            page: PageLayer::new(device, LAYER_FORMAT),
            style: (false, 0),
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
        }
    }

    /// The terminal shader's bind group: params, cells, atlas, palette, the
    /// layer below the glyphs and the phosphor textures (beam, fast, slow,
    /// and the bloom or the bloom's horizontal pass).
    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        textures: [&wgpu::Texture; 8],
    ) -> wgpu::BindGroup {
        let views = textures.map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }];
        for (i, v) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: i as u32 + 1, resource: wgpu::BindingResource::TextureView(v) });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("terminal"), layout, entries: &entries })
    }

    /// Takes the vector display's style (SPEC 8.7) and uploads its list
    /// (SPEC 8) as `Vector::shapes` gives it (outlines in the phosphor
    /// presentation even if only forced), unless the list's generation shows
    /// it is unchanged. `None`: the image has no vector display.
    pub fn update_vectors(&mut self, queue: &wgpu::Queue, vector: Option<&Vector>) {
        let persistence = vector.map_or(terse_core::vector::DEFAULT_PERSISTENCE, Vector::persistence);
        let style = (self.force_phosphor || vector.is_some_and(Vector::phosphor), persistence);
        if style.0 != self.style.0 {
            // Switching style starts the phosphor afresh.
            self.phosphor_stale = true;
        }
        self.style = style;
        let outlines = outlined(style.0, vector);
        let (list, generation) = vector.map_or((&[][..], 0), |v| (v.list(), v.generation()));
        self.vectors.update(queue, list, generation, outlines);
    }

    /// Takes the page device's state and the layout to show (once per
    /// machine frame, after `update_vectors`).
    pub fn update_page(&mut self, page: Option<&terse_core::Page>, layout: Option<std::sync::Arc<HostLayout>>) {
        self.page.set(page, layout);
    }

    /// Whether the page's scroll is animated (not for screenshots).
    pub fn set_page_animation(&mut self, on: bool) {
        self.page.set_animate(on);
    }

    /// The page's anchor under a position in screen units, as shown.
    pub fn page_anchor_at_units(&self, x: f32, y: f32) -> u16 {
        self.page.anchor_at(x, y)
    }

    /// The page's anchor under a viewport position (SPEC 11.5), as shown.
    pub fn page_anchor_at(&self, viewport: (u32, u32), x: f64, y: f64) -> u16 {
        let (ox, oy, w, h) = self.grid_rect(viewport);
        let sx = (x as f32 - ox) / w * SCREEN_WIDTH as f32;
        let sy = (y as f32 - oy) / h * SCREEN_HEIGHT as f32;
        if !(0.0..SCREEN_WIDTH as f32).contains(&sx) || !(0.0..SCREEN_HEIGHT as f32).contains(&sy) {
            return 0;
        }
        self.page.anchor_at(sx, sy)
    }

    /// Treat the persistence as 0 (the user turned trails off, SPEC 8.7).
    pub fn set_no_trails(&mut self, no_trails: bool) {
        self.no_trails = no_trails;
    }

    /// Show the phosphor presentation whatever the program's style (for
    /// testing). Takes effect at the next `update_vectors`.
    pub fn set_force_phosphor(&mut self, force: bool) {
        self.force_phosphor = force;
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
        self.phosphor_frame(device, queue, viewport, true);
    }

    /// Phosphor presentation: redraws the beam from the terminal and list
    /// uploaded since the last `accumulate`, without advancing the afterglow
    /// (e.g. to show a fault reported after the last machine frame).
    pub fn refresh_beam(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport: (u32, u32)) {
        self.phosphor_frame(device, queue, viewport, false);
    }

    fn phosphor_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport: (u32, u32), advance: bool) {
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
            flags: shown | 16,
        };
        queue.write_buffer(&self.light_params, 0, &params.bytes());
        for (buffer, flags) in self.bloom_params.iter().zip([32, 64]) {
            queue.write_buffer(buffer, 0, &Params { flags, ..params }.bytes());
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("phosphor") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glyph light"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.beam,
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
        // The page's text is light on the same tube (SPEC 10.6); no background.
        self.page.prepare(device, queue, (0.0, 0.0, w, h), t.size, true);
        if self.page.quads() > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("page light"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.beam,
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
        let persistence = if self.no_trails { 0 } else { self.style.1 };
        let targets = PhosphorTargets { beam: &t.beam, beam_group: &t.beam_group, fast: &t.fast, slow: &t.slow };
        let clear = self.phosphor_stale;
        self.vectors.phosphor_frame(queue, &mut encoder, &targets, t.size, persistence, clear, advance);
        if advance {
            self.phosphor_stale = false;
        }
        // The bloom of all that light, x then y.
        for (target, group) in [(&t.blurred, &t.bloom_groups[0]), (&t.bloom, &t.bloom_groups[1])] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }

    /// Size of the whole grid at the atlas's native cell width: 4:3, so that
    /// screen units are square (SPEC 5.4) whatever the glyph source's cell.
    pub fn grid_size(&self) -> (u32, u32) {
        let width = COLS as u32 * self.cell_px.0;
        (width, width * SCREEN_HEIGHT as u32 / SCREEN_WIDTH as u32)
    }

    /// Uploads the terminal state (SPEC 6.7: presentation is up to the host).
    pub fn update(&mut self, queue: &wgpu::Queue, terminal: &Terminal) {
        if self.atlas.next + COLS * ROWS > ATLAS_SLOTS {
            // Atlas full: start over; the screen needs at most COLS * ROWS slots.
            self.atlas.slots.clear();
            self.atlas.next = 1;
        }
        for row in 0..ROWS {
            let mut col = 0;
            while col < COLS {
                let cell = terminal.cell(col, row);
                // SPEC 6.3: a character followed by FFFFh is drawn across both cells.
                let wide = cell.ch != CONTINUATION
                    && col + 1 < COLS
                    && terminal.cell(col + 1, row).ch == CONTINUATION;
                let slot = if cell.ch == CONTINUATION { BLANK_SLOT } else { self.slot_for(queue, cell.ch, wide) };
                for half in 0..1 + wide as usize {
                    let c = terminal.cell(col + half, row);
                    let s = if slot == BLANK_SLOT { BLANK_SLOT } else { slot + half as u16 };
                    let i = (row * COLS + col + half) * 4;
                    let [lo, hi] = s.to_le_bytes();
                    self.cell_data[i..i + 4].copy_from_slice(&[lo, hi, c.fg, c.bg]);
                }
                col += 1 + wide as usize;
            }
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
        self.glyphs.rasterize(ch, cells, glyph);
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
        grid_rect(self.grid_size(), viewport)
    }

    /// A viewport position in screen units (SPEC 5.4), through the same
    /// transform that places the grid, clamped to the screen.
    pub fn screen_pos(&self, viewport: (u32, u32), x: f64, y: f64) -> (u16, u16) {
        screen_pos(self.grid_rect(viewport), x, y)
    }

    /// The height of one terminal row in viewport pixels.
    pub fn row_height(&self, viewport: (u32, u32)) -> f64 {
        (self.grid_rect(viewport).3 / ROWS as f32) as f64
    }

    /// The layer for `viewport` and the glyph-mode bind group reading it.
    fn layer(&mut self, device: &wgpu::Device, viewport: (u32, u32)) -> (wgpu::TextureView, wgpu::BindGroup) {
        if let Some((size, view, group)) = &self.layer
            && *size == viewport
        {
            return (view.clone(), group.clone());
        }
        // The phosphor textures cover the grid rectangle, which starts at a
        // whole pixel; new ones start dark.
        let (_, _, w, h) = self.grid_rect(viewport);
        let size = ((w.ceil() as u32).max(1), (h.ceil() as u32).max(1));
        let phosphor = |label| {
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
        let (beam, fast, slow) = (phosphor("phosphor beam"), phosphor("afterglow fast"), phosphor("afterglow slow"));
        let (blurred, bloom) = (phosphor("bloom x"), phosphor("bloom"));
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let beam_view = view(&beam);
        let unused = texture(device, "unused layer", (1, 1), LAYER_FORMAT);
        let unused_phosphor = texture(device, "unused phosphor", (1, 1), PHOSPHOR_FORMAT);
        let bloom_group = |params, textures: [&wgpu::Texture; 4]| {
            let [b, f, s, src] = textures;
            let fixed = [&self.cell_texture, &self.atlas_texture, &self.palette_texture, &unused];
            let [c, a, p, u] = fixed;
            Self::bind_group(device, &self.bind_group_layout, params, [c, a, p, u, b, f, s, src])
        };
        let u = &unused_phosphor;
        let bloom_groups = [
            bloom_group(&self.bloom_params[0], [&beam, &fast, &slow, u]),
            bloom_group(&self.bloom_params[1], [u, u, u, &blurred]),
        ];
        self.phosphor = Some(PhosphorTextures {
            size,
            beam_group: self.vectors.beam_bind_group(device, &beam_view),
            beam: beam_view,
            fast: view(&fast),
            slow: view(&slow),
            blurred: view(&blurred),
            bloom: view(&bloom),
            bloom_groups,
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
        let group = Self::bind_group(
            device,
            &self.bind_group_layout,
            &self.params_buffer,
            [&self.cell_texture, &self.atlas_texture, &self.palette_texture, &tex, &beam, &fast, &slow, &bloom],
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.layer = Some((viewport, view.clone(), group.clone()));
        (view, group)
    }

    /// Records the draw into `encoder`, covering all of `target`
    /// (`viewport` pixels): backgrounds, shapes, then glyphs (SPEC 8.6).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) {
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
            flags: shown | if self.target_is_srgb { 2 } else { 0 } | if self.style.0 { 8 } else { 0 },
        };
        let glyph_params = Params { flags: params.flags | self.overlay << 8, ..params };
        queue.write_buffer(&self.params_buffer, 0, &glyph_params.bytes());
        queue.write_buffer(&self.background_params, 0, &Params { flags: 4, ..params }.bytes());
        if self.style.0 && self.layer.as_ref().is_none_or(|(size, ..)| *size != viewport) {
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
            pass.draw(0..3, 0..1);
            // SPEC 10.5: the page lies between the backgrounds and the shapes.
            self.page.draw(&mut pass, false);
            self.vectors.draw(queue, &mut pass, viewport, rect);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("glyphs"),
            color_attachments: &[attachment(target, wgpu::LoadOp::Clear(wgpu::Color::BLACK))],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &glyph_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer uploads exactly what `Vector::shapes` gives: outlines in
    /// the phosphor style or with fill off, the capped list otherwise; and
    /// outlines when the phosphor presentation is forced.
    #[test]
    fn draws_what_the_device_says() {
        use crate::vectors::decode_shapes;
        // A disc, a rectangle and a triangle.
        let list = [
            &[0x04, 0, 1, 0, 1][..],
            &[0x07, 64, 0],
            &[0x08, 0, 2, 0, 2],
            &[0x09, 0, 3, 0, 3, 0, 4, 0, 1],
        ]
        .concat();
        for (style, fill) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let mut v = Vector::default();
            assert_eq!(v.submit(&list), 0);
            v.set_style(style);
            v.set_fill(fill);
            let mut drawn = Vec::new();
            decode_shapes(v.list(), outlined(v.phosphor(), Some(&v)), &mut drawn);
            assert_eq!(drawn, v.shapes().collect::<Vec<_>>(), "style {style} fill {fill}");
            let filled = drawn.iter().any(|p| !matches!(p, terse_core::Primitive::Line { .. } | terse_core::Primitive::Circle { .. }));
            assert_eq!(filled, style == 0 && fill == 0, "style {style} fill {fill}");
            decode_shapes(v.list(), outlined(true, Some(&v)), &mut drawn);
            assert_eq!(drawn, terse_core::vector::decode_outlines(v.list()).collect::<Vec<_>>());
        }
        assert!(!outlined(false, None) && outlined(true, None));
    }

    #[test]
    fn viewport_to_screen_units() {
        // A 10 × 20 px cell grid (800 × 600) shown at 2x in a 1700 × 1300
        // viewport: offset (50, 50), 1600 × 1200.
        let rect = grid_rect((800, 600), (1700, 1300));
        assert_eq!(rect, (50.0, 50.0, 1600.0, 1200.0));
        assert_eq!(screen_pos(rect, 50.0, 50.0), (0, 0));
        assert_eq!(screen_pos(rect, 1649.9, 1249.9), (639, 479));
        // Cell (column 3, row 2) starts at screen (24, 32) (SPEC 5.4).
        assert_eq!(screen_pos(rect, 50.0 + 3.0 * 20.0, 50.0 + 2.0 * 40.0), (24, 32));
        // Outside the grid: clamped.
        assert_eq!(screen_pos(rect, 0.0, 2000.0), (0, 479));
        assert_eq!(screen_pos(rect, 1699.0, -5.0), (639, 0));
        // Scaled down to fit.
        let rect = grid_rect((800, 600), (400, 600));
        assert_eq!(rect, (0.0, 150.0, 400.0, 300.0));
        assert_eq!(screen_pos(rect, 200.0, 300.0), (320, 240));
    }

    /// Whatever the window's shape, the grid is 4:3 and centred, so a screen
    /// unit is as wide as it is high (SPEC 5.4), and the pointer maps back
    /// through the same rectangle.
    #[test]
    fn grid_stays_four_by_three() {
        let native = (880, 660); // 11 x 22 pixel cells
        for viewport in [(880, 660), (1920, 1080), (1000, 2000), (300, 200), (1761, 1321), (881, 1500)] {
            let (x, y, w, h) = grid_rect(native, viewport);
            assert!((w / h - 4.0 / 3.0).abs() < 1e-6, "{viewport:?}: {w} x {h}");
            assert!(w <= viewport.0 as f32 && h <= viewport.1 as f32, "{viewport:?}");
            // Centred (the offset is a whole pixel, the size need not be).
            assert!((x - (viewport.0 as f32 - w - x)).abs() < 2.0, "{viewport:?}");
            assert!((y - (viewport.1 as f32 - h - y)).abs() < 2.0, "{viewport:?}");
            // One screen unit is the same size on both axes.
            assert!((w / 640.0 - h / 480.0).abs() < 1e-4);
            // The centre of the window is the centre of the screen.
            let c = screen_pos((x, y, w, h), viewport.0 as f64 / 2.0, viewport.1 as f64 / 2.0);
            assert!(c.0.abs_diff(320) <= 1 && c.1.abs_diff(240) <= 1, "{viewport:?}: {c:?}");
        }
        assert_eq!(grid_rect(native, (1920, 1080)), (520.0, 210.0, 880.0, 660.0));
        assert_eq!(grid_rect(native, (1760, 1400)), (0.0, 40.0, 1760.0, 1320.0));
    }
}

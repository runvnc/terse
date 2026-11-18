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

use std::collections::HashMap;

use terse_core::{COLS, CONTINUATION, ROWS, SCREEN_HEIGHT, SCREEN_WIDTH, Terminal};

use crate::vectors::VectorLayer;

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
    flags: u32,             // bit 0: cursor shown, bit 1: target is sRGB, bit 2: background mode
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var cells: texture_2d<u32>;
@group(0) @binding(2) var atlas: texture_2d<f32>;
@group(0) @binding(3) var palette: texture_2d<f32>;
@group(0) @binding(4) var under: texture_2d<f32>;  // backgrounds and shapes (glyph mode)

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

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
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
    /// The layer of backgrounds and shapes, its size, and the glyph-mode
    /// bind group that reads it; made for the viewport on first use.
    layer: Option<((u32, u32), wgpu::TextureView, wgpu::BindGroup)>,
    vectors: VectorLayer,
    cell_texture: wgpu::Texture,
    atlas_texture: wgpu::Texture,
    palette_texture: wgpu::Texture,
    cell_data: Vec<u8>,
    glyph_scratch: Vec<u8>,
    cursor: Option<(u32, u32)>,
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
            ],
        });
        // Background mode never reads `under`; it gets a 1 × 1 stand-in.
        let unused = texture(device, "unused layer", (1, 1), LAYER_FORMAT);
        let background_bind_group = Self::bind_group(
            device,
            &layout,
            &background_params,
            [&cell_texture, &atlas_texture, &palette_texture, &unused],
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
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
            layer: None,
            vectors: VectorLayer::new(device, LAYER_FORMAT),
            cell_texture,
            atlas_texture,
            palette_texture,
            cell_data: vec![0; COLS * ROWS * 4],
            glyph_scratch: vec![0; glyph_bytes],
            cursor: None,
        }
    }

    /// The terminal shader's bind group: params, cells, atlas, palette and
    /// the layer below the glyphs.
    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        textures: [&wgpu::Texture; 4],
    ) -> wgpu::BindGroup {
        let views = textures.map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }];
        for (i, v) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: i as u32 + 1, resource: wgpu::BindingResource::TextureView(v) });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("terminal"), layout, entries: &entries })
    }

    /// Uploads the vector display list (SPEC 8), unless `generation` (see
    /// `Machine::vector_generation`) shows it is unchanged.
    pub fn update_vectors(&mut self, queue: &wgpu::Queue, list: &[u8], generation: u64) {
        self.vectors.update(queue, list, generation);
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
            [&self.cell_texture, &self.atlas_texture, &self.palette_texture, &tex],
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
            flags: shown | if self.target_is_srgb { 2 } else { 0 },
        };
        queue.write_buffer(&self.params_buffer, 0, &params.bytes());
        queue.write_buffer(&self.background_params, 0, &Params { flags: 4, ..params }.bytes());
        let (layer, glyph_group) = self.layer(device, viewport);

        let attachment = |view, load| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })
        };
        {
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

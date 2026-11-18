//! Terminal renderer: one full-screen fragment-shader pass over three textures
//! (cells, glyph atlas, palette). Independent of windowing and of where glyph
//! bitmaps come from, so a browser front end can reuse it.

use std::collections::HashMap;

use terse_core::{COLS, CONTINUATION, ROWS, SCREEN_HEIGHT, SCREEN_WIDTH, Terminal};

/// Supplies glyph coverage bitmaps, one or two terminal cells in size.
pub trait GlyphSource {
    /// Cell size in pixels (width, height).
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
    flags: u32,             // bit 0: cursor shown, bit 1: target is sRGB
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var cells: texture_2d<u32>;
@group(0) @binding(2) var atlas: texture_2d<f32>;
@group(0) @binding(3) var palette: texture_2d<f32>;

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
    if all(rel >= vec2<f32>(0.0)) && all(rel < vec2<f32>(1.0)) {
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
        // Blend in sRGB space, as terminals usually do.
        color = mix(palette_rgb(c.a), palette_rgb(c.b), coverage);
    }
    if (p.flags & 2u) != 0u {
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
/// integer multiple of the native size `grid` that fits, or a scaled-down fit.
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
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    params_buffer: wgpu::Buffer,
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
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
            ],
        });
        let [cells_view, atlas_view, palette_view] = [&cell_texture, &atlas_texture, &palette_texture]
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        let view = wgpu::BindingResource::TextureView;
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminal"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: view(&cells_view) },
                wgpu::BindGroupEntry { binding: 2, resource: view(&atlas_view) },
                wgpu::BindGroupEntry { binding: 3, resource: view(&palette_view) },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminal"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                targets: &[Some(target_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let glyph_bytes = (2 * cell_px.0 * cell_px.1) as usize;
        Renderer {
            glyphs,
            atlas: Atlas { slots: HashMap::new(), next: 1 },
            cell_px,
            target_is_srgb: target_format.is_srgb(),
            pipeline,
            bind_group,
            params_buffer,
            cell_texture,
            atlas_texture,
            palette_texture,
            cell_data: vec![0; COLS * ROWS * 4],
            glyph_scratch: vec![0; glyph_bytes],
            cursor: None,
        }
    }

    /// Size of the whole grid at the atlas's native cell size.
    pub fn grid_size(&self) -> (u32, u32) {
        (COLS as u32 * self.cell_px.0, ROWS as u32 * self.cell_px.1)
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

    /// Records the draw into `encoder`, covering all of `target`.
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) {
        let (x, y, w, h) = self.grid_rect(viewport);
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

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terminal"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
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
}

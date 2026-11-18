//! The vector display layer (SPEC 8): the decoded display list drawn as
//! instanced quads. Lines, circle outlines and discs get round ends and
//! antialiasing from a distance computed in the fragment shader; rectangles
//! and triangles are plain geometry. Shapes are blended over each other in
//! list order (source-over, by opacity) and clipped to the screen rectangle.
//! The screen is the terminal grid's rectangle, which the renderer keeps at
//! 4:3, so screen units are square (SPEC 5.4) and a circle is round.

use terse_core::{Primitive, SCREEN_HEIGHT, SCREEN_WIDTH};
use terse_core::vector::{MAX_LIST, decode};

/// Most primitives a list can hold: a circle or disc command is 3 bytes.
pub const MAX_PRIMITIVES: usize = MAX_LIST / 3 + 1;

const KIND_LINE: u32 = 0;
const KIND_CIRCLE: u32 = 1;
const KIND_DISC: u32 = 2;
const KIND_RECT: u32 = 3;
const KIND_TRIANGLE: u32 = 4;

/// One instance: kind, a = (x0, y0, x1, y1), b = (x2, y2, radius, width),
/// all in sixteenths, then r, g, b, a.
const INSTANCE_BYTES: usize = 40;

const SHADER: &str = r#"
// Shapes are placed and measured in sixteenths of a screen unit and mapped to
// pixels: the screen covers the terminal grid's rectangle, which is 4:3, so
// the two scales are equal; they are kept apart only for generality.
struct Params {
    origin: vec2<f32>,    // the screen's top left in framebuffer pixels
    viewport: vec2<f32>,  // framebuffer size in pixels
    scale: vec2<f32>,     // pixels per sixteenth, x and y
}

@group(0) @binding(0) var<uniform> u: Params;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) kind: u32,
    @location(1) @interpolate(flat) ends: vec4<f32>,   // p0, p1
    @location(2) @interpolate(flat) shape: vec3<f32>,  // radius, half width, sixteenths per pixel
    @location(3) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) kind: u32,
    @location(1) a: vec4<f32>,
    @location(2) b: vec4<f32>,
    @location(3) color: vec4<f32>,
) -> Out {
    let p0 = a.xy;
    let p1 = a.zw;
    let p2 = b.xy;
    let r = b.z;
    let hw = b.w * 0.5;
    // One pixel in sixteenths: the larger for the quad's margin, the mean for
    // the antialiasing ramp.
    let px_max = 1.0 / min(u.scale.x, u.scale.y);
    let px_mean = 2.0 / (u.scale.x + u.scale.y);
    // Two triangles over corners (-1,-1), (1,-1), (-1,1), (1,1).
    var corner = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    let c = corner[vi];
    let s = vec2<f32>(f32(c & 1u), f32(c >> 1u)) * 2.0 - 1.0;
    // Room for the antialiased edge.
    let ext = max(hw, 0.5 * px_max) + 1.5 * px_max;
    var q: vec2<f32>;
    switch kind {
        case 0u: {
            let d = p1 - p0;
            let len = length(d);
            var dir = vec2<f32>(1.0, 0.0);
            if len > 1e-3 {
                dir = d / len;
            }
            let n = vec2<f32>(-dir.y, dir.x);
            q = (p0 + p1) * 0.5 + dir * s.x * (len * 0.5 + ext) + n * s.y * ext;
        }
        case 1u, 2u: {
            q = p0 + s * (r + ext);
        }
        case 3u: {
            q = mix(min(p0, p1), max(p0, p1), s * 0.5 + 0.5);
        }
        default: {
            // Triangle: vertices 3-5 collapse onto p0 and draw nothing.
            q = p0;
            if vi == 1u {
                q = p1;
            } else if vi == 2u {
                q = p2;
            }
        }
    }
    let px = u.origin + q * u.scale;
    var out: Out;
    out.pos = vec4<f32>(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0, 0.0, 1.0);
    out.kind = kind;
    out.ends = vec4<f32>(p0, p1);
    out.shape = vec3<f32>(r, hw, px_mean);
    out.color = color;
    return out;
}

// Coverage of a pixel whose centre is d pixels from the centre line of a
// stroke hw pixels wide on each side; strokes thinner than a pixel fade
// instead of thinning.
fn stroke(hw: f32, d: f32) -> f32 {
    if hw >= 0.5 {
        return clamp(hw + 0.5 - d, 0.0, 1.0);
    }
    return clamp(1.0 - d, 0.0, 1.0) * 2.0 * hw;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    // This pixel's centre in sixteenths; distances below are in pixels.
    let q = (in.pos.xy - u.origin) / u.scale;
    let px = in.shape.z;
    var cover = 1.0;
    switch in.kind {
        case 0u: {
            let pa = q - in.ends.xy;
            let ba = in.ends.zw - in.ends.xy;
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
            cover = stroke(in.shape.y / px, length(pa - ba * h) / px);
        }
        case 1u: {
            cover = stroke(in.shape.y / px, abs(length(q - in.ends.xy) - in.shape.x) / px);
        }
        case 2u: {
            cover = clamp((in.shape.x - length(q - in.ends.xy)) / px + 0.5, 0.0, 1.0);
        }
        default: {}
    }
    if cover <= 0.0 {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * cover);
}
"#;

/// Instance bytes for one primitive.
fn instance(p: &Primitive, out: &mut Vec<u8>) {
    let f = |v: i16| v as f32;
    let (kind, a, b, rgba) = match *p {
        Primitive::Line { x0, y0, x1, y1, width, rgba } => {
            (KIND_LINE, [f(x0), f(y0), f(x1), f(y1)], [0.0, 0.0, 0.0, width as f32], rgba)
        }
        Primitive::Circle { x, y, radius, width, rgba } => {
            (KIND_CIRCLE, [f(x), f(y), 0.0, 0.0], [0.0, 0.0, radius as f32, width as f32], rgba)
        }
        Primitive::Disc { x, y, radius, rgba } => {
            (KIND_DISC, [f(x), f(y), 0.0, 0.0], [0.0, 0.0, radius as f32, 0.0], rgba)
        }
        Primitive::Rect { x0, y0, x1, y1, rgba } => (KIND_RECT, [f(x0), f(y0), f(x1), f(y1)], [0.0; 4], rgba),
        Primitive::Triangle { x0, y0, x1, y1, x2, y2, rgba } => {
            (KIND_TRIANGLE, [f(x0), f(y0), f(x1), f(y1)], [f(x2), f(y2), 0.0, 0.0], rgba)
        }
    };
    out.extend_from_slice(&kind.to_le_bytes());
    for v in a.iter().chain(&b) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&rgba);
}

pub struct VectorLayer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    instances: wgpu::Buffer,
    count: u32,
    /// The list generation last uploaded.
    uploaded: Option<u64>,
    scratch: Vec<u8>,
}

impl VectorLayer {
    /// `format` is the format of the layer the shapes are drawn into.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vector params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vector instances"),
            size: (MAX_PRIMITIVES * INSTANCE_BYTES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vectors"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vectors"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vectors"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vectors"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Uint32, 1 => Float32x4, 2 => Float32x4, 3 => Unorm8x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vectors"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_BYTES as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        VectorLayer {
            pipeline,
            bind_group,
            params,
            instances,
            count: 0,
            uploaded: None,
            scratch: Vec::with_capacity(MAX_PRIMITIVES * INSTANCE_BYTES),
        }
    }

    /// Uploads the display list, unless `generation` says it is unchanged.
    pub fn update(&mut self, queue: &wgpu::Queue, list: &[u8], generation: u64) {
        if self.uploaded == Some(generation) {
            return;
        }
        self.uploaded = Some(generation);
        self.scratch.clear();
        let mut count = 0;
        for p in decode(list).take(MAX_PRIMITIVES) {
            instance(&p, &mut self.scratch);
            count += 1;
        }
        self.count = count;
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.instances, 0, &self.scratch);
        }
    }

    /// Draws into `pass`, whose target is `viewport` pixels; `rect` is the
    /// screen (the grid rectangle) in those pixels.
    pub fn draw(&self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass, viewport: (u32, u32), rect: (f32, f32, f32, f32)) {
        if self.count == 0 {
            return;
        }
        let (x, y, w, h) = rect;
        let mut bytes = Vec::with_capacity(32);
        // Equal for a 4:3 rectangle (Renderer::grid_size).
        let (sx, sy) = (w / (SCREEN_WIDTH as f32 * 16.0), h / (SCREEN_HEIGHT as f32 * 16.0));
        for v in [x, y, viewport.0 as f32, viewport.1 as f32, sx, sy, 0.0, 0.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.params, 0, &bytes);
        // Clip to the screen rectangle (SPEC 8.1).
        let x0 = x.max(0.0).floor() as u32;
        let y0 = y.max(0.0).floor() as u32;
        let x1 = ((x + w).ceil() as u32).min(viewport.0);
        let y1 = ((y + h).ceil() as u32).min(viewport.1);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..(self.count as u64 * INSTANCE_BYTES as u64)));
        pass.draw(0..6, 0..self.count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_layout() {
        let mut out = Vec::new();
        instance(&Primitive::Circle { x: -16, y: 32, radius: 48, width: 3, rgba: [1, 2, 3, 4] }, &mut out);
        assert_eq!(out.len(), INSTANCE_BYTES);
        assert_eq!(&out[0..4], &KIND_CIRCLE.to_le_bytes());
        assert_eq!(&out[4..8], &(-16f32).to_le_bytes());
        assert_eq!(&out[28..32], &48f32.to_le_bytes());
        assert_eq!(&out[32..36], &3f32.to_le_bytes());
        assert_eq!(&out[36..40], &[1, 2, 3, 4]);
    }
}

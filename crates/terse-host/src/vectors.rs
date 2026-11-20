//! The vector display layer (SPEC §vector): the decoded display list drawn as
//! instanced quads. Lines, circle outlines and discs get round ends and
//! antialiasing from a distance computed in the fragment shader; rectangles
//! and triangles are plain geometry. Shapes are blended over each other in
//! list order (source-over, by opacity) and clipped to the screen rectangle.
//! The screen is the terminal grid's rectangle, which the renderer keeps at
//! 4:3, so screen units are square (SPEC §screen) and a circle is round.
//!
//! The phosphor style (SPEC §vector.phosphor, see `phosphor.rs` for the model and its
//! constants) draws the list's outlines (`terse_core::vector::
//! decode_outlines`) as light into float textures the size of the screen,
//! once per machine frame: the beam and the afterglow's fast and slow
//! components, which are first faded (a full-screen pass that blends with
//! a constant and subtracts a little), then take the deposit of this
//! frame's beam and text (summed) with a `max` blend, and of the earlier
//! positions of moving shapes. Each line and circle adds the light of a
//! Gaussian beam spot moved along it (`phosphor::beam_light`), σ in
//! pixels, with a dot of extra light at a line's end points (vertex
//! dwell). The renderer adds the halo (`halo.rs`) and composites the
//! textures.

use terse_core::{Primitive, SCREEN_HEIGHT, SCREEN_WIDTH};

use crate::display::{MAX_SHAPES, SceneKey, VectorScene};
use crate::phosphor::{self, DEPOSIT_BUDGET, Deposit};

/// Format of the phosphor texture: linear light, unclamped, so long trails
/// fade smoothly and overlaps can exceed 1 (shown white-hot).
pub const PHOSPHOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

const KIND_LINE: u32 = 0;
const KIND_CIRCLE: u32 = 1;
const KIND_DISC: u32 = 2;
const KIND_RECT: u32 = 3;
const KIND_TRIANGLE: u32 = 4;
/// A 3D segment (`Primitive::Beam`): a line with a depth cue at each end
/// (bits 16-23 and 24-31 of the kind) and butt ends (bits 4-5).
const KIND_BEAM: u32 = 5;

/// One instance: kind, a = (x0, y0, x1, y1), b = (x2, y2, radius, width),
/// all in sixteenths, then r, g, b, a. An afterglow deposit (always a line or
/// circle) carries its fast and slow deposit in place of (x2, y2). In the
/// phosphor style bits 8-15 of the kind hold a line's vertex dwell
/// (`phosphor::dwell_ends`).
const INSTANCE_BYTES: usize = 40;

const SHADER: &str = r#"
// Shapes are placed and measured in sixteenths of a screen unit and mapped to
// pixels: the screen covers the terminal grid's rectangle, which is 4:3, so
// the two scales are equal; they are kept apart only for generality.
struct Params {
    origin: vec2<f32>,    // the screen's top left in framebuffer pixels
    viewport: vec2<f32>,  // framebuffer size in pixels
    scale: vec2<f32>,     // pixels per sixteenth, x and y
    glow: f32,            // 1 for the phosphor style: quads leave room for the glow
    linear: f32,          // 1: the target is sRGB, colours are converted to linear light
}

@group(0) @binding(0) var<uniform> u: Params;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) kind: u32,
    @location(1) @interpolate(flat) ends: vec4<f32>,   // p0, p1
    @location(2) @interpolate(flat) shape: vec3<f32>,  // radius, half width, sixteenths per pixel
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) deposit: vec2<f32>,  // fast, slow (afterglow deposits)
    @location(5) @interpolate(flat) dwell: u32,  // lines sharing each end (phosphor::dwell_ends)
    @location(6) @interpolate(flat) fade: vec2<f32>,  // a 3D segment's depth cue at each end
    @location(7) @interpolate(flat) butt: u32,  // a 3D segment's butt ends: bit 0 start, bit 1 end
}

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) kind_dwell: u32,
    @location(1) a: vec4<f32>,
    @location(2) b: vec4<f32>,
    @location(3) color: vec4<f32>,
) -> Out {
    let kind = kind_dwell & 0xFu;
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
    // Room for the antialiased edge, or for the beam's Gaussian.
    var ext = max(hw, 0.5 * px_max) + 1.5 * px_max;
    if u.glow > 0.0 {
        ext = max(ext, BEAM_REACH * beam_sigma(b.w, px_mean) * px_mean + 1.5 * px_max);
    }
    var q: vec2<f32>;
    switch kind {
        case 0u, 5u: {
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
    out.deposit = b.xy;
    out.dwell = (kind_dwell >> 8u) & 0xFFu;
    out.fade = vec2<f32>(1.0);
    out.butt = 0u;
    if kind == 5u {
        out.fade = vec2<f32>(f32((kind_dwell >> 16u) & 0xFFu), f32(kind_dwell >> 24u)) / 255.0;
        out.butt = (kind_dwell >> 4u) & 3u;
    }
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

// A line's (kind 0) or 3D segment's (kind 5) distance from q (sixteenths)
// and its weight there: for a segment, the depth cue interpolated along it,
// and at a butt end a step half a pixel either side of the end instead of
// a round cap (so chords that join add up to one).
fn segment(in: Out, q: vec2<f32>) -> vec2<f32> {
    let pa = q - in.ends.xy;
    let ba = in.ends.zw - in.ends.xy;
    let len2 = dot(ba, ba);
    let hr = dot(pa, ba) / max(len2, 1e-6);
    var h = clamp(hr, 0.0, 1.0);
    if in.kind != 5u {
        return vec2<f32>(length(pa - ba * h), 1.0);
    }
    let w = mix(in.fade.x, in.fade.y, h);
    if in.butt == 0u {
        return vec2<f32>(length(pa - ba * h), w);
    }
    let len = sqrt(len2);
    let along = hr * len / in.shape.z;
    let b0 = (in.butt & 1u) != 0u;
    let b1 = (in.butt & 2u) != 0u;
    let s0 = select(1.0, clamp(along + 0.5, 0.0, 1.0), b0);
    let s1 = select(0.0, clamp(along - len / in.shape.z + 0.5, 0.0, 1.0), b1);
    if (b0 && hr < 0.0) || (b1 && hr > 1.0) {
        h = hr;
    }
    return vec2<f32>(length(pa - ba * h), w * (s0 - s1));
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    // This pixel's centre in sixteenths; distances below are in pixels.
    let q = (in.pos.xy - u.origin) / u.scale;
    let px = in.shape.z;
    var cover = 1.0;
    switch in.kind {
        case 0u, 5u: {
            let dw = segment(in, q);
            cover = stroke(in.shape.y / px, dw.x / px) * dw.y;
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
    var c = in.color.rgb;
    if u.linear > 0.5 {
        c = to_linear(c);
    }
    return vec4<f32>(c, in.color.a * cover);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Phosphor style: the light a line or circle (the only kinds an outline list
// holds) adds at this pixel, before colour; 0 outside its reach. The
// Gaussian beam of phosphor::beam_light with σ in pixels (in.shape.z
// sixteenths each), the depth cue of a 3D segment, and at a line's ends
// the dot of its vertex dwell, shared between the n lines that end there.
fn beam_light(in: Out) -> f32 {
    let q = (in.pos.xy - u.origin) / u.scale;
    let px = in.shape.z;
    let width = 2.0 * in.shape.y;
    let s = beam_sigma(width, px) * px;
    let top = beam_peak(width);
    switch in.kind {
        case 0u, 5u: {
            let a = in.ends.xy;
            let b = in.ends.zw;
            let d = b - a;
            let len = length(d);
            var dir = vec2<f32>(1.0, 0.0);
            if len > 1e-3 {
                dir = d / len;
            }
            let r = (q - a) / s;
            let along = dot(r, dir);
            let across = r.y * dir.x - r.x * dir.y;
            let ls = len / s;
            var v = beam_profile(along, across, ls);
            let n0 = in.dwell & 15u;
            let n1 = (in.dwell >> 4u) & 15u;
            if n0 != 0u {
                v += DWELL * DOT / f32(n0) * exp(-0.5 * dot(r, r));
            }
            if n1 != 0u && any(a != b) {
                let e = (q - b) / s;
                v += DWELL * DOT / f32(n1) * exp(-0.5 * dot(e, e));
            }
            var h = 0.0;
            if ls > 0.0 {
                h = clamp(along / ls, 0.0, 1.0);
            }
            return v * top * mix(in.fade.x, in.fade.y, h);
        }
        case 1u: {
            let k = abs(length(q - in.ends.xy) - in.shape.x) / s;
            return top * exp(-0.5 * k * k);
        }
        default: {
            return 0.0;
        }
    }
}

// The beam: this frame's light, added into the beam texture.
@fragment
fn fs_beam(in: Out) -> @location(0) vec4<f32> {
    let light = beam_light(in);
    if light <= 0.0 {
        discard;
    }
    return vec4<f32>(to_linear(in.color.rgb) * in.color.a * light, 0.0);
}

struct Afterglow {
    @location(0) fast: vec4<f32>,
    @location(1) slow: vec4<f32>,
}

// An afterglow deposit into both components (max blend).
@fragment
fn fs_deposit(in: Out) -> Afterglow {
    let light = beam_light(in);
    if light <= 0.0 {
        discard;
    }
    let c = vec4<f32>(to_linear(in.color.rgb) * in.color.a * light, 0.0);
    return Afterglow(c * in.deposit.x, c * in.deposit.y);
}

// A triangle covering the target, for the fade.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}

// The fade: the blend keeps the target times the blend constant, minus
// this, so faint light reaches zero (below zero reads as dark; the next
// deposit's max replaces it).
@fragment
fn fs_fade() -> @location(0) vec4<f32> {
    return vec4<f32>(EPSILON);
}

// No light (the parts of the screen a machine's Surfaces show, SPEC §vector3d.presentation).
@fragment
fn fs_zero() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

// This frame's beam and text (summed, so overdraw counts) deposited into
// the afterglow (max blend).
@group(0) @binding(1) var beam: texture_2d<f32>;
@group(0) @binding(2) var text: texture_2d<f32>;

@fragment
fn fs_beam_deposit(@builtin(position) pos: vec4<f32>) -> Afterglow {
    let at = vec2<i32>(pos.xy);
    let c = (textureLoad(beam, at, 0) + textureLoad(text, at, 0)) * DEPOSIT;
    return Afterglow(c * FAST_SHARE, c * (1.0 - FAST_SHARE));
}
"#;

/// Instance bytes for one primitive; `deposit` (fast, slow) goes in place
/// of the unused (x2, y2) of a line or circle, `dwell` in bits 8-15 of the
/// kind.
fn instance(p: &Primitive, deposit: [f32; 2], dwell: u32, out: &mut Vec<u8>) {
    let f = |v: i16| v as f32;
    let [df, ds] = deposit;
    let (kind, a, b, rgba) = match *p {
        Primitive::Beam { x0, y0, x1, y1, width, rgba, fade, butt } => {
            let kind = KIND_BEAM | (butt as u32 & 3) << 4 | (fade[0] as u32) << 16 | (fade[1] as u32) << 24;
            (kind, [f(x0), f(y0), f(x1), f(y1)], [df, ds, 0.0, width as f32], rgba)
        }
        Primitive::Line { x0, y0, x1, y1, width, rgba } => {
            (KIND_LINE, [f(x0), f(y0), f(x1), f(y1)], [df, ds, 0.0, width as f32], rgba)
        }
        Primitive::Circle { x, y, radius, width, rgba } => {
            (KIND_CIRCLE, [f(x), f(y), 0.0, 0.0], [df, ds, radius as f32, width as f32], rgba)
        }
        Primitive::Disc { x, y, radius, rgba } => {
            (KIND_DISC, [f(x), f(y), 0.0, 0.0], [0.0, 0.0, radius as f32, 0.0], rgba)
        }
        Primitive::Rect { x0, y0, x1, y1, rgba } => (KIND_RECT, [f(x0), f(y0), f(x1), f(y1)], [0.0; 4], rgba),
        Primitive::Triangle { x0, y0, x1, y1, x2, y2, rgba } => {
            (KIND_TRIANGLE, [f(x0), f(y0), f(x1), f(y1)], [f(x2), f(y2), 0.0, 0.0], rgba)
        }
    };
    out.extend_from_slice(&(kind | (dwell & 0xFF) << 8).to_le_bytes());
    for v in a.iter().chain(&b) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&rgba);
}

/// The phosphor textures of one machine frame (all [`PHOSPHOR_FORMAT`],
/// the size of the screen): the beam (cleared and drawn here), the text,
/// which holds this frame's glyph and page light, and the afterglow's two
/// components; `beam_group` is [`VectorLayer::beam_bind_group`] of the
/// beam and text.
pub struct PhosphorTargets<'a> {
    pub beam: &'a wgpu::TextureView,
    pub text: &'a wgpu::TextureView,
    pub beam_group: &'a wgpu::BindGroup,
    pub fast: &'a wgpu::TextureView,
    pub slow: &'a wgpu::TextureView,
}

pub struct VectorLayer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    /// Phosphor style (own parameters, as its textures are not the
    /// target's size): the beam (additive), the afterglow deposits (max into
    /// both components), the fade, and the whole beam's deposit.
    beam_pipeline: wgpu::RenderPipeline,
    deposit_pipeline: wgpu::RenderPipeline,
    phosphor_bind_group: wgpu::BindGroup,
    phosphor_params: wgpu::Buffer,
    fade_pipeline: wgpu::RenderPipeline,
    /// Writes no light (a rectangle a Surface shows).
    zero_pipeline: wgpu::RenderPipeline,
    beam_deposit_pipeline: wgpu::RenderPipeline,
    beam_layout: wgpu::BindGroupLayout,
    instances: wgpu::Buffer,
    count: u32,
    /// The afterglow deposits of a frame, with motion sub-steps.
    deposit_instances: wgpu::Buffer,
    /// What was uploaded last (lists, eye, outlines).
    uploaded: Option<SceneKey>,
    /// The primitives uploaded and the list's opcodes; the same for the
    /// previous machine frame of the phosphor style.
    shapes: Vec<Primitive>,
    /// The vertex dwell of each of `shapes` (outlines only).
    dwell: Vec<u32>,
    opcodes: Vec<u8>,
    prev_shapes: Vec<Primitive>,
    prev_opcodes: Vec<u8>,
    deposits: Vec<Deposit>,
    scratch: Vec<u8>,
    /// The target is sRGB and takes linear light (shapes drawn straight
    /// into a target rather than into the layer).
    linear: bool,
}

/// The shader's parameters for a target of `viewport` pixels in which the
/// screen is `rect`.
fn params_bytes(viewport: (u32, u32), rect: (f32, f32, f32, f32), glow: bool, linear: bool) -> Vec<u8> {
    let (x, y, w, h) = rect;
    let mut bytes = Vec::with_capacity(32);
    // Equal for a 4:3 rectangle (Renderer::grid_size).
    let (sx, sy) = (w / (SCREEN_WIDTH as f32 * 16.0), h / (SCREEN_HEIGHT as f32 * 16.0));
    for v in [x, y, viewport.0 as f32, viewport.1 as f32, sx, sy, glow as u32 as f32, linear as u32 as f32] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

fn pass<'e>(encoder: &'e mut wgpu::CommandEncoder, label: &str, targets: &[(&wgpu::TextureView, bool)]) -> wgpu::RenderPass<'e> {
    let attachments: Vec<_> = targets
        .iter()
        .map(|&(view, clear)| {
            let load = if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load };
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })
        })
        .collect();
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

impl VectorLayer {
    /// `format` is the format of the layer the shapes are drawn into.
    pub fn new(shaders: &crate::renderer::Shaders, format: wgpu::TextureFormat) -> Self {
        let device = shaders.device();
        let uniform = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params = uniform("vector params");
        let phosphor_params = uniform("phosphor params");
        let vertex_buffer = |label, primitives: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (primitives * INSTANCE_BYTES) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let instances = vertex_buffer("vector instances", MAX_SHAPES);
        let deposit_instances = vertex_buffer("afterglow deposits", DEPOSIT_BUDGET);
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
        let light = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let beam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("phosphor beam"),
            entries: &[light(1), light(2)],
        });
        let group = |params: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vectors"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }],
            })
        };
        let bind_group = group(&params);
        let phosphor_bind_group = group(&phosphor_params);
        let shader = shaders.module("vectors", || format!("{}{SHADER}", phosphor::wgsl()));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vectors"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let target = |format, blend| Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL });
        let attributes = wgpu::vertex_attr_array![0 => Uint32, 1 => Float32x4, 2 => Float32x4, 3 => Unorm8x4];
        let shapes = |fs, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = shapes("fs", &[target(format, wgpu::BlendState::ALPHA_BLENDING)]);
        let component = |src_factor, dst_factor, operation| wgpu::BlendComponent { src_factor, dst_factor, operation };
        let blend = |c: wgpu::BlendComponent| wgpu::BlendState { color: c, alpha: c };
        use wgpu::{BlendFactor as F, BlendOperation as O};
        let add = blend(component(F::One, F::One, O::Add));
        let max = blend(component(F::One, F::One, O::Max));
        // Target × constant − source.
        let fade = blend(component(F::One, F::Constant, O::ReverseSubtract));
        let beam_pipeline = shapes("fs_beam", &[target(PHOSPHOR_FORMAT, add)]);
        let both = [target(PHOSPHOR_FORMAT, max), target(PHOSPHOR_FORMAT, max)];
        let deposit_pipeline = shapes("fs_deposit", &both);
        let full_screen = |label, layouts: &[Option<&wgpu::BindGroupLayout>], fs, targets: &[Option<wgpu::ColorTargetState>]| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let fade_pipeline = full_screen("phosphor fade", &[], "fs_fade", &[target(PHOSPHOR_FORMAT, fade)]);
        let zero_pipeline = full_screen("phosphor mask", &[], "fs_zero", &[target(PHOSPHOR_FORMAT, wgpu::BlendState::REPLACE)]);
        let beam_deposit_pipeline = full_screen("beam deposit", &[Some(&beam_layout)], "fs_beam_deposit", &both);
        VectorLayer {
            pipeline,
            bind_group,
            params,
            beam_pipeline,
            deposit_pipeline,
            phosphor_bind_group,
            phosphor_params,
            fade_pipeline,
            zero_pipeline,
            beam_deposit_pipeline,
            beam_layout,
            instances,
            count: 0,
            deposit_instances,
            uploaded: None,
            shapes: Vec::new(),
            dwell: Vec::new(),
            opcodes: Vec::new(),
            prev_shapes: Vec::new(),
            prev_opcodes: Vec::new(),
            deposits: Vec::new(),
            scratch: Vec::with_capacity(DEPOSIT_BUDGET * INSTANCE_BYTES),
            linear: false,
        }
    }

    /// Shapes drawn straight into an sRGB target (`format`), over what it
    /// holds: their colours are converted to linear light.
    pub fn for_target(shaders: &crate::renderer::Shaders, format: wgpu::TextureFormat) -> Self {
        VectorLayer { linear: format.is_srgb(), ..Self::new(shaders, format) }
    }

    /// Uploads `shapes` as they are (no vertex dwell), unless they are
    /// what was uploaded last.
    pub fn upload(&mut self, queue: &wgpu::Queue, shapes: &[Primitive]) {
        if self.uploaded.is_none() && self.shapes == shapes {
            return;
        }
        self.uploaded = None;
        self.shapes.clear();
        self.shapes.extend_from_slice(shapes);
        self.scratch.clear();
        for p in shapes.iter().take(MAX_SHAPES) {
            instance(p, [0.0; 2], 0, &mut self.scratch);
        }
        self.count = shapes.len().min(MAX_SHAPES) as u32;
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.instances, 0, &self.scratch);
        }
    }

    /// The bind group through which the light in `beam` and `text` is deposited.
    pub fn beam_bind_group(&self, device: &wgpu::Device, beam: &wgpu::TextureView, text: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("phosphor beam"),
            layout: &self.beam_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(beam) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(text) },
            ],
        })
    }

    /// Uploads the display lists, unless their generations and the eye say
    /// they are unchanged: the 2D list with `outlines` as `decode_outlines`
    /// gives it (the phosphor style, or fill off), otherwise as `decode`
    /// does (with the fill cap), the same primitives as `Vector::shapes`;
    /// then the 3D list's segments (`VectorScene::shapes`).
    pub fn update(&mut self, queue: &wgpu::Queue, scene: &VectorScene, outlines: bool) {
        let key = scene.key(outlines);
        if self.uploaded == Some(key) {
            return;
        }
        self.uploaded = Some(key);
        scene.shapes(outlines, &mut self.shapes, &mut self.opcodes);
        if outlines {
            phosphor::dwell_ends(&self.shapes, &mut self.dwell);
        } else {
            self.dwell.clear();
            self.dwell.resize(self.shapes.len(), 0);
        }
        self.scratch.clear();
        for (p, &dwell) in self.shapes.iter().zip(&self.dwell) {
            instance(p, [0.0; 2], dwell, &mut self.scratch);
        }
        self.count = self.shapes.len() as u32;
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
        queue.write_buffer(&self.params, 0, &params_bytes(viewport, rect, false, self.linear));
        // Clip to the screen rectangle (SPEC §vector.coordinates).
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

    /// One machine frame of the phosphor style (SPEC §vector.phosphor, `phosphor.rs`),
    /// recorded into `encoder`; the textures are `size` pixels, exactly the
    /// screen. The afterglow fades (or is cleared if `clear`, or with
    /// persistence 0: no trail), the uploaded outlines are drawn into the
    /// beam, then the afterglow takes the deposit of beam and text and of
    /// the earlier positions of shapes that only moved since the last frame.
    /// With `advance` false only the beam is redrawn and nothing fades.
    /// `fast_decay` is the fast component's factor per frame.
    #[allow(clippy::too_many_arguments)]
    pub fn phosphor_frame(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        targets: &PhosphorTargets,
        size: (u32, u32),
        persistence: u8,
        fast_decay: f32,
        clear: bool,
        advance: bool,
    ) {
        let rect = (0.0, 0.0, size.0 as f32, size.1 as f32);
        queue.write_buffer(&self.phosphor_params, 0, &params_bytes(size, rect, true, false));
        let trails = persistence > 0;
        let slow_decay = phosphor::slow_decay(persistence);
        if advance {
            for (view, keep) in [(targets.fast, fast_decay), (targets.slow, slow_decay)] {
                let fade = trails && !clear;
                let mut pass = pass(encoder, "phosphor fade", &[(view, !fade)]);
                if fade {
                    let keep = keep as f64;
                    pass.set_pipeline(&self.fade_pipeline);
                    pass.set_blend_constant(wgpu::Color { r: keep, g: keep, b: keep, a: keep });
                    pass.draw(0..3, 0..1);
                }
            }
        }
        {
            // Cleared: last frame's beam is gone.
            let mut pass = pass(encoder, "beam", &[(targets.beam, true)]);
            if self.count > 0 {
                pass.set_pipeline(&self.beam_pipeline);
                pass.set_bind_group(0, &self.phosphor_bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..(self.count as u64 * INSTANCE_BYTES as u64)));
                pass.draw(0..6, 0..self.count);
            }
        }
        if !advance {
            return;
        }
        if trails {
            let smear = !clear && self.opcodes == self.prev_opcodes;
            phosphor::deposits(&self.prev_shapes, &self.shapes, smear, fast_decay, slow_decay, &mut self.deposits);
            self.scratch.clear();
            for d in &self.deposits {
                instance(&d.shape, [d.fast, d.slow], self.dwell[d.index], &mut self.scratch);
            }
            if !self.scratch.is_empty() {
                queue.write_buffer(&self.deposit_instances, 0, &self.scratch);
            }
            let mut pass = pass(encoder, "afterglow deposit", &[(targets.fast, false), (targets.slow, false)]);
            pass.set_pipeline(&self.beam_deposit_pipeline);
            pass.set_bind_group(0, targets.beam_group, &[]);
            pass.draw(0..3, 0..1);
            if !self.deposits.is_empty() {
                let n = self.deposits.len() as u32;
                pass.set_pipeline(&self.deposit_pipeline);
                pass.set_bind_group(0, &self.phosphor_bind_group, &[]);
                pass.set_vertex_buffer(0, self.deposit_instances.slice(..(n as u64 * INSTANCE_BYTES as u64)));
                pass.draw(0..6, 0..n);
            }
        }
        self.prev_shapes.clone_from(&self.shapes);
        self.prev_opcodes.clone_from(&self.opcodes);
    }

    /// Takes all light out of `rects` (x, y, w, h pixels of the phosphor
    /// textures): the beam and both parts of the afterglow, so nothing of
    /// what is there is drawn or blooms (SPEC §vector3d.presentation: the sources of a
    /// machine's Surfaces are not on its glass).
    pub fn mask(&self, encoder: &mut wgpu::CommandEncoder, targets: &PhosphorTargets, rects: &[(u32, u32, u32, u32)]) {
        if rects.is_empty() {
            return;
        }
        for view in [targets.beam, targets.text, targets.fast, targets.slow] {
            let mut pass = pass(encoder, "phosphor mask", &[(view, false)]);
            pass.set_pipeline(&self.zero_pipeline);
            for &(x, y, w, h) in rects {
                pass.set_scissor_rect(x, y, w, h);
                pass.draw(0..3, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_layout() {
        let mut out = Vec::new();
        instance(&Primitive::Circle { x: -16, y: 32, radius: 48, width: 3, rgba: [1, 2, 3, 4] }, [0.0; 2], 0, &mut out);
        assert_eq!(out.len(), INSTANCE_BYTES);
        assert_eq!(&out[0..4], &KIND_CIRCLE.to_le_bytes());
        assert_eq!(&out[4..8], &(-16f32).to_le_bytes());
        assert_eq!(&out[28..32], &48f32.to_le_bytes());
        assert_eq!(&out[32..36], &3f32.to_le_bytes());
        assert_eq!(&out[36..40], &[1, 2, 3, 4]);
        out.clear();
        instance(&Primitive::Line { x0: 0, y0: 0, x1: 1, y1: 1, width: 3, rgba: [0; 4] }, [0.0; 2], 0x21, &mut out);
        assert_eq!(&out[0..4], &(KIND_LINE | 0x21 << 8).to_le_bytes());
        out.clear();
        let beam = Primitive::Beam { x0: 1, y0: 2, x1: 3, y1: 4, width: 9, rgba: [5, 6, 7, 8], fade: [255, 128], butt: 3 };
        instance(&beam, [0.0; 2], 0, &mut out);
        assert_eq!(&out[0..4], &(KIND_BEAM | 3 << 4 | 255 << 16 | 128 << 24).to_le_bytes());
        assert_eq!(&out[32..36], &9f32.to_le_bytes());
    }
}

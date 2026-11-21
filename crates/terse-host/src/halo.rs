//! The phosphor style's halo on the GPU (SPEC §vector.phosphor; the model is in
//! `phosphor.rs`, and `soft/phosphor.rs` computes the same on the CPU): a
//! pyramid of the light at ¼, ⅛ … 1/32 of the screen's resolution, each of
//! `phosphor::HALO`'s wide, faint Gaussians blurred on its level, and the
//! levels summed back up. Full-screen passes of one shader, in order:
//!
//! 1. what the halo blurs (`max(beam + TEXT_HALO × text, afterglow)`,
//!    whitened above one stroke) averaged over 4 × 4 pixels into the first
//!    level;
//! 2. each further level the 2 × 2 mean of the one before;
//! 3. from the smallest level up: each blur on that level, along x into
//!    the level's scratch texture, then along y, times its weight, added
//!    into the level's blurs; then the level's sum, its blurs plus the
//!    level below sampled up bilinearly.
//!
//! The renderer's composite samples the first level's sum up to the screen. The
//! passes' parameters depend only on the screen's size, so they are fixed
//! when the textures are made.

use wgpu::util::DeviceExt;

use crate::phosphor::{self, HALO_FIRST, HALO_LEVELS, HaloBlur, level_size};
use crate::renderer::Shaders;
use crate::vectors::PHOSPHOR_FORMAT;

const SHADER: &str = r#"
struct Params {
    mode: u32,     // 0: light into the first level, 1: 2 x 2 mean, 2: blur, 3: sum
    taps: i32,
    sigma: f32,
    weight: f32,
    dir: vec2<i32>,
    flags: u32,    // sum: bit 0 the level's blurs (a), bit 1 the level below (b)
    pad: u32,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var ta: texture_2d<f32>;
@group(0) @binding(2) var tb: texture_2d<f32>;
@group(0) @binding(3) var tc: texture_2d<f32>;
@group(0) @binding(4) var td: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}

// What the halo blurs at a texel of the screen: beam (a), text (b) and the
// afterglow's components (c, d), whitened above one stroke.
fn source(at: vec2<i32>) -> vec3<f32> {
    let direct = textureLoad(ta, at, 0).rgb + TEXT_HALO * textureLoad(tb, at, 0).rgb;
    let glow = max(textureLoad(tc, at, 0).rgb, vec3<f32>(0.0)) + max(textureLoad(td, at, 0).rgb, vec3<f32>(0.0));
    let c = max(direct, glow);
    let peak = max(c.r, max(c.g, c.b));
    return c + vec3<f32>(max(peak - BLOOM_WHITE_FROM, 0.0) * BLOOM_WHITE);
}

// Where pixel i of a level samples the level below it (n pixels): the two
// pixels and the second's weight, edges repeated.
fn up(i: i32, n: i32) -> vec3<f32> {
    let f = (f32(i) + 0.5) * 0.5 - 0.5;
    let i0 = floor(f);
    return vec3<f32>(clamp(i0, 0.0, f32(n - 1)), clamp(i0 + 1.0, 0.0, f32(n - 1)), f - i0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(floor(pos.xy));
    var v = vec3<f32>(0.0);
    switch p.mode {
        case 0u, 1u: {
            let size = vec2<i32>(textureDimensions(ta));
            let f = select(2, HALO_FACTOR, p.mode == 0u);
            var n = 0.0;
            for (var j = 0; j < f; j++) {
                for (var i = 0; i < f; i++) {
                    let s = at * f + vec2<i32>(i, j);
                    if all(s < size) {
                        if p.mode == 0u {
                            v += source(s);
                        } else {
                            v += textureLoad(ta, s, 0).rgb;
                        }
                        n += 1.0;
                    }
                }
            }
            v /= n;
        }
        case 2u: {
            let size = vec2<i32>(textureDimensions(ta));
            if p.taps == 0 {
                v = textureLoad(ta, at, 0).rgb;
            } else {
                var total = 0.0;
                for (var i = -p.taps; i <= p.taps; i++) {
                    let w = exp(-f32(i * i) / (2.0 * p.sigma * p.sigma));
                    total += w;
                    let s = at + p.dir * i;
                    if all(s >= vec2<i32>(0)) && all(s < size) {
                        v += w * textureLoad(ta, s, 0).rgb;
                    }
                }
                v /= total;
            }
            v *= p.weight;
        }
        default: {
            if (p.flags & 1u) != 0u {
                v = textureLoad(ta, at, 0).rgb;
            }
            if (p.flags & 2u) != 0u {
                let size = vec2<i32>(textureDimensions(tb));
                let ux = up(at.x, size.x);
                let uy = up(at.y, size.y);
                let a = textureLoad(tb, vec2<i32>(i32(ux.x), i32(uy.x)), 0).rgb;
                let b = textureLoad(tb, vec2<i32>(i32(ux.y), i32(uy.x)), 0).rgb;
                let c = textureLoad(tb, vec2<i32>(i32(ux.x), i32(uy.y)), 0).rgb;
                let d = textureLoad(tb, vec2<i32>(i32(ux.y), i32(uy.y)), 0).rgb;
                let top = a + (b - a) * ux.z;
                let bottom = c + (d - c) * ux.z;
                v += top + (bottom - top) * uy.z;
            }
        }
    }
    return vec4<f32>(v, 0.0);
}
"#;

/// The halo's pipelines (one device's, shared by the sizes).
pub struct HaloPipelines {
    layout: wgpu::BindGroupLayout,
    replace: wgpu::RenderPipeline,
    add: wgpu::RenderPipeline,
}

impl HaloPipelines {
    pub fn new(shaders: &Shaders) -> Self {
        let device = shaders.device();
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("halo"),
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
                texture(1),
                texture(2),
                texture(3),
                texture(4),
            ],
        });
        let shader = shaders.module("halo", || format!("{}{SHADER}", phosphor::wgsl()));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("halo"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("halo"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: PHOSPHOR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        HaloPipelines { replace: pipeline(None), add: pipeline(Some(wgpu::BlendState { color: add, alpha: add })), layout }
    }
}

/// One pass: what it draws into, whether it adds to it (else replaces;
/// `clear` first), and its parameters and textures.
struct Pass {
    target: wgpu::TextureView,
    add: bool,
    clear: bool,
    group: wgpu::BindGroup,
}

/// The pyramid's textures and passes for one screen size.
pub struct HaloChain {
    passes: Vec<Pass>,
    /// The first level's sum: the halo the composite samples.
    pub halo: wgpu::TextureView,
}

/// A pass's parameters as the shader's `Params`.
fn params(mode: u32, blur: Option<(&HaloBlur, (i32, i32), f32)>, flags: u32) -> Vec<u8> {
    let (taps, sigma, weight, dir) = match blur {
        Some((b, dir, weight)) => (b.taps, b.sigma, weight, dir),
        None => (0, 0.0, 1.0, (0, 0)),
    };
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&mode.to_le_bytes());
    out.extend_from_slice(&taps.to_le_bytes());
    out.extend_from_slice(&sigma.to_le_bytes());
    out.extend_from_slice(&weight.to_le_bytes());
    out.extend_from_slice(&dir.0.to_le_bytes());
    out.extend_from_slice(&dir.1.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

impl HaloChain {
    /// The pyramid of a `size` screen whose light is in `beam`, `text`,
    /// `fast` and `slow`; `unused` stands in for textures a pass does not read.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        pipes: &HaloPipelines,
        size: (u32, u32),
        beam: &wgpu::TextureView,
        text: &wgpu::TextureView,
        fast: &wgpu::TextureView,
        slow: &wgpu::TextureView,
        unused: &wgpu::TextureView,
    ) -> Self {
        let texture = |label, s: (usize, usize)| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: s.0 as u32, height: s.1 as u32, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: PHOSPHOR_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let screen = (size.0 as usize, size.1 as usize);
        // Per level (index 0 is the first): the mean, the scratch, the blurs, the sum.
        let levels: Vec<[wgpu::TextureView; 4]> = (HALO_FIRST..=HALO_LEVELS)
            .map(|k| {
                let s = level_size(screen, k);
                [texture("halo level", s), texture("halo scratch", s), texture("halo blurs", s), texture("halo sum", s)]
            })
            .collect();
        let group = |bytes: Vec<u8>, views: [&wgpu::TextureView; 4]| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("halo params"),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }];
            for (i, v) in views.iter().enumerate() {
                entries.push(wgpu::BindGroupEntry { binding: i as u32 + 1, resource: wgpu::BindingResource::TextureView(v) });
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("halo"), layout: &pipes.layout, entries: &entries })
        };
        let u = unused;
        let mut passes = vec![Pass { target: levels[0][0].clone(), add: false, clear: false, group: group(params(0, None, 0), [beam, text, fast, slow]) }];
        for k in 1..levels.len() {
            passes.push(Pass { target: levels[k][0].clone(), add: false, clear: false, group: group(params(1, None, 0), [&levels[k - 1][0], u, u, u]) });
        }
        let blurs = phosphor::halo_blurs(size.1 as f32);
        for k in (0..levels.len()).rev() {
            let [mean, scratch, sum_of_blurs, sum] = &levels[k];
            let mine: Vec<&HaloBlur> = blurs.iter().filter(|b| b.level == k + HALO_FIRST).collect();
            for (i, b) in mine.iter().enumerate() {
                passes.push(Pass { target: scratch.clone(), add: false, clear: false, group: group(params(2, Some((b, (1, 0), 1.0)), 0), [mean, u, u, u]) });
                let v = params(2, Some((b, (0, 1), b.weight)), 0);
                passes.push(Pass { target: sum_of_blurs.clone(), add: true, clear: i == 0, group: group(v, [scratch, u, u, u]) });
            }
            let lower = levels.get(k + 1).map(|l| &l[3]);
            let flags = (!mine.is_empty()) as u32 | (lower.is_some() as u32) << 1;
            let views = [if mine.is_empty() { u } else { sum_of_blurs }, lower.unwrap_or(u), u, u];
            passes.push(Pass { target: sum.clone(), add: false, clear: false, group: group(params(3, None, flags), views) });
        }
        HaloChain { passes, halo: levels[0][3].clone() }
    }

    /// Records a pass that leaves no halo: the sum the composite samples
    /// cleared.
    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("halo shed"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.halo,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    /// Records the passes.
    pub fn encode(&self, pipes: &HaloPipelines, encoder: &mut wgpu::CommandEncoder) {
        for p in &self.passes {
            let load = if p.clear || !p.add { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("halo"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &p.target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(if p.add { &pipes.add } else { &pipes.replace });
            pass.set_bind_group(0, &p.group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

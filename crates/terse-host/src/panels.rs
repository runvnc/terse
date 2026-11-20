//! Panels on the GPU (SPEC §machines.screens, §vector3d.list Screen): each panel's machine is
//! drawn into a texture of its own (`PANEL_FORMAT`, at a size from the
//! panel's projection, `display::panel_cell_width`), its mipmaps made by
//! halving with linear filtering (in linear light, the texture is sRGB),
//! and the panel drawn as a quad whose corners carry the projection's w,
//! so the GPU maps the texture in perspective; trilinear, anisotropic
//! sampling keeps text on a tilted or turning panel from shimmering. The
//! near plane (z = −800) is the quad's clip-space z = 0. Seen from behind,
//! the quad is the dark plate `PANEL_BACK`.

use terse_core::SCREEN_WIDTH;
use terse_core::vector3::{EYE_DISTANCE, NEAR_Z, Panel};

use crate::display::PANEL_BACK;

/// The format of a panel's texture: sRGB, so filtering is in linear light.
pub const PANEL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Bytes of one quad corner: clip position (4), uv (2), flags, pad.
const VERTEX_BYTES: usize = 32;
/// Corners of one quad (a triangle strip).
const CORNERS: usize = 4;

const SHADER: &str = r#"
struct V {
    @location(0) pos: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) flags: vec2<f32>,  // x: 1 seen from behind, 2 black; y: 1 the target is not sRGB
}

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) flags: vec2<f32>,
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs(v: V) -> Out {
    var o: Out;
    o.pos = v.pos;
    o.uv = v.uv;
    o.flags = v.flags;
    return o;
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

@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    // Sampled in uniform control flow (the derivatives choose the level).
    var c = textureSample(tex, samp, o.uv).rgb;
    if o.flags.x > 1.5 {
        c = vec3<f32>(0.0);
    } else if o.flags.x > 0.5 {
        c = to_linear(BACK);
    }
    if o.flags.y > 0.5 {
        c = to_srgb(c);
    }
    return vec4<f32>(c, 1.0);
}

// The mipmaps: one level from the one before, a triangle over the target.
struct Full {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Full {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Full;
    o.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return o;
}

@fragment
fn fs_half(o: Full) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleLevel(tex, samp, o.uv, 0.0).rgb, 1.0);
}
"#;

/// The pipelines and samplers every panel shares.
pub struct PanelPipeline {
    quad: wgpu::RenderPipeline,
    half: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Linear, one level: what each mip level is made from.
    half_sampler: wgpu::Sampler,
    target_srgb: bool,
}

/// Most quads one set draws: the shell's panels and Surfaces, and the
/// rectangles left off its glass.
pub const MAX_QUADS: usize = terse_core::machines::MAX_MACHINES + 2 * terse_core::vector3::MAX_SURFACES;

/// The quads of one scene (a vertex buffer of its own, so that several
/// scenes can be drawn in one submission).
pub struct PanelQuads {
    vertices: wgpu::Buffer,
    count: usize,
}

/// What a quad shows: its texture through the window (u0, v0, u1, v1) of
/// it, the dark plate of a panel seen from behind, or black (a rectangle
/// left off the glass).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QuadFill {
    Texture([f32; 4]),
    Back,
    Black,
}

/// One quad: its corners (screen units; top left, top right, bottom
/// right, bottom left) and what it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub corners: [[f64; 3]; 4],
    pub fill: QuadFill,
}

impl Quad {
    /// A panel: a Screen shows its machine's whole texture, a Surface its
    /// source's part of a texture of the whole screen.
    pub fn panel(p: &Panel) -> Quad {
        let [x, y, w, h] = p.source.map(|v| v as f32);
        let fill = if p.front { QuadFill::Texture([x / 640.0, y / 480.0, (x + w) / 640.0, (y + h) / 480.0]) } else { QuadFill::Back };
        Quad { corners: p.corners, fill }
    }

    /// The source rectangle `r` (screen units) on the glass, black.
    pub fn black(r: [u16; 4]) -> Quad {
        let [x, y, w, h] = r.map(|v| v as f64);
        Quad { corners: [[x, y, 0.0], [x + w, y, 0.0], [x + w, y + h, 0.0], [x, y + h, 0.0]], fill: QuadFill::Black }
    }
}

/// One panel's texture: its size, the view the quad samples, and for each
/// level after the first its target view and the bind group of the level
/// before.
pub struct PanelTexture {
    pub size: (u32, u32),
    /// The full-size level, which the machine's screen is drawn into.
    pub level0: wgpu::TextureView,
    group: wgpu::BindGroup,
    levels: Vec<(wgpu::TextureView, wgpu::BindGroup)>,
}

impl PanelPipeline {
    pub fn new(shaders: &crate::renderer::Shaders, target: wgpu::TextureFormat) -> Self {
        let device = shaders.device();
        let back = format!("const BACK: vec3<f32> = vec3<f32>({:.6}, {:.6}, {:.6});\n", PANEL_BACK[0] as f32 / 255.0, PANEL_BACK[1] as f32 / 255.0, PANEL_BACK[2] as f32 / 255.0);
        let shader = shaders.module("panels", || format!("{back}{SHADER}"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("panel"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("panel"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Float32x2];
        let quad = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("panel"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_BYTES as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(target.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let half = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("panel mipmap"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_half"),
                compilation_options: Default::default(),
                targets: &[Some(PANEL_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let clamp = wgpu::AddressMode::ClampToEdge;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("panel"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            address_mode_w: clamp,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let half_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("panel mipmap"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            address_mode_w: clamp,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        PanelPipeline { quad, half, layout, sampler, half_sampler, target_srgb: target.is_srgb() }
    }

    /// A vertex buffer for one scene's quads.
    pub fn quads(&self, device: &wgpu::Device) -> PanelQuads {
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("panel corners"),
            size: (MAX_QUADS * CORNERS * VERTEX_BYTES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        PanelQuads { vertices, count: 0 }
    }

    fn group(&self, device: &wgpu::Device, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("panel"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        })
    }

    /// A texture of `size` with its mip levels.
    pub fn texture(&self, device: &wgpu::Device, size: (u32, u32)) -> PanelTexture {
        let levels = 32 - size.0.max(size.1).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("panel"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: PANEL_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let level = |k: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: k, mip_level_count: Some(1), ..Default::default() })
        };
        let all = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = self.group(device, &all, &self.sampler);
        let views: Vec<wgpu::TextureView> = (0..levels).map(level).collect();
        let chain = (1..levels as usize).map(|k| (views[k].clone(), self.group(device, &views[k - 1], &self.half_sampler))).collect();
        PanelTexture { size, level0: views[0].clone(), group, levels: chain }
    }

    /// Makes the mip levels of `t` from its first level.
    pub fn mipmaps(&self, encoder: &mut wgpu::CommandEncoder, t: &PanelTexture) {
        for (view, group) in &t.levels {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("panel mipmap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.half);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Uploads `quads` (in drawing order) into `to`, seen from the eye at
    /// `eye` (screen units) on a screen drawn into `rect` of a `viewport`
    /// target.
    pub fn upload(&self, queue: &wgpu::Queue, to: &mut PanelQuads, quads: &[Quad], eye: (f64, f64), rect: (f32, f32, f32, f32), viewport: (u32, u32)) {
        let (vw, vh) = (viewport.0 as f64, viewport.1 as f64);
        let s = rect.2 as f64 / SCREEN_WIDTH as f64;
        let (ox, oy) = (rect.0 as f64, rect.1 as f64);
        let near_w = (EYE_DISTANCE + NEAR_Z) / EYE_DISTANCE;
        let mut bytes = Vec::with_capacity(quads.len() * CORNERS * VERTEX_BYTES);
        for q in quads.iter().take(MAX_QUADS) {
            let ([u0, v0, u1, v1], kind) = match q.fill {
                QuadFill::Texture(w) => (w, 0.0),
                QuadFill::Back => ([0.0, 0.0, 1.0, 1.0], 1.0),
                QuadFill::Black => ([0.0, 0.0, 1.0, 1.0], 2.0),
            };
            // Strip order: top left, top right, bottom left, bottom right.
            for (k, uv) in [(0, (u0, v0)), (1, (u1, v0)), (3, (u0, v1)), (2, (u1, v1))] {
                let [x, y, z] = q.corners[k];
                let w = (EYE_DISTANCE + z) / EYE_DISTANCE;
                let cx = (2.0 * (ox + s * eye.0) / vw - 1.0) * w + 2.0 * s * (x - eye.0) / vw;
                let cy = (1.0 - 2.0 * (oy + s * eye.1) / vh) * w - 2.0 * s * (y - eye.1) / vh;
                let v = [cx, cy, w - near_w, w, uv.0 as f64, uv.1 as f64, kind, if self.target_srgb { 0.0 } else { 1.0 }];
                for f in v {
                    bytes.extend_from_slice(&(f as f32).to_le_bytes());
                }
            }
        }
        to.count = quads.len().min(MAX_QUADS);
        if !bytes.is_empty() {
            queue.write_buffer(&to.vertices, 0, &bytes);
        }
    }

    /// Draws the quads uploaded into `quads`, the `i`th with `textures[i]`
    /// (any texture for one that shows none), clipped to `scissor` (x, y,
    /// w, h pixels).
    pub fn draw(&self, pass: &mut wgpu::RenderPass, quads: &PanelQuads, textures: &[&PanelTexture], scissor: (u32, u32, u32, u32)) {
        pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
        pass.set_pipeline(&self.quad);
        pass.set_vertex_buffer(0, quads.vertices.slice(..));
        for (i, t) in textures.iter().enumerate().take(quads.count) {
            pass.set_bind_group(0, &t.group, &[]);
            let first = (i * CORNERS) as u32;
            pass.draw(first..first + CORNERS as u32, 0..1);
        }
    }
}

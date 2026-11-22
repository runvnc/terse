// license:BSD-3-Clause
// copyright-holders:Hans Andersson
// Translated from MAME src/osd/modules/render/bgfx/vectorrenderer.cpp and
// vectorrenderer.h, with the render states and default uniforms of
// bgfx/effects/vector-crt/*.json (copyright (c) 2016, Ryan Holtz and MAME
// Development Team), commit 6f9a2f023e1a951ba34f289d6a4d214a9578676b, to Rust
// and wgpu for Tersenet. The originals are in third_party/mame-vector-crt/.
//
//! MAME's vector CRT pipeline on wgpu: a persistent HDR phosphor buffer
//! (two RGBA16F targets, ping-ponged), decayed each frame by
//! exp(−dt / τ); every vector of the frame added to it as an instanced
//! Gaussian beam with scan-order timing; a half-resolution box downsample;
//! two horizontal and vertical Gaussian blur iterations (the bloom); then
//! phosphor + bloom × strength, exposure, luminance tone map, gamma 1/2.2.
//!
//! The functions follow `bgfx_vector_renderer`'s: [`Crt::new`] is
//! `create_targets`, [`Crt::frame`] is `prepare` and `composite` together,
//! [`Params::default`] is `create_sliders`' defaults. Where this differs from
//! MAME, a comment starting "Tersenet:" says how and why.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use crate::mame::beams::BeamInstance;
pub use super::params::{BEAM_SIGMA_SCALE, BLOOM_PASSES, Params, shaders};

/// The format of MAME's phosphor and bloom targets (`RGBA16F`).
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Tersenet: the composite's target. MAME composites into its BGFX view (the
/// window's back buffer, 8 bits a channel, not sRGB: the shader's own
/// 1/2.2 gamma is the encoding); this is the same, read back for the layer.
/// BGRA, cleared to opaque black (the composite writes no alpha), so that
/// each pixel read back is already a light layer's word (DEVICES.md 7.1:
/// alpha 255, then red, green, blue).
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;


/// A graphics device and MAME's five effects on it, shared by every
/// machine a driver draws.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The adapter's name and type, for messages and measurements.
    pub adapter: String,
    /// True for a software adapter (lavapipe, llvmpipe, WARP).
    pub software: bool,
    beam: wgpu::RenderPipeline,
    decay: wgpu::RenderPipeline,
    downsample: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    post_vertices: wgpu::Buffer,
    beam_vertices: wgpu::Buffer,
}

fn vertex_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

impl Gpu {
    /// The first hardware adapter wgpu offers, else a software one (as
    /// terse-host does), and the effects built on it.
    pub fn new() -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) {
            Ok(a) => a,
            Err(_) => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: true, ..Default::default() }))
                .map_err(|e| format!("no graphics adapter: {e}"))?,
        };
        let info = adapter.get_info();
        let software = info.device_type == wgpu::DeviceType::Cpu;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("cannot open graphics device: {e}"))?;
        let name = format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend);
        Ok(Gpu::on(device, queue, name, software))
    }

    /// The effects on a device someone else opened (terse-host's GPU
    /// renderer).
    pub fn on(device: wgpu::Device, queue: wgpu::Queue, adapter: String, software: bool) -> Gpu {
        let mut modules = std::collections::HashMap::new();
        for (name, vs, fs, src) in shaders::programs() {
            let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(src.into()) });
            modules.insert(name, (m, vs, fs));
        }
        // The blend states of bgfx/effects/vector-crt/*.json: beam and
        // composite add (srcColor 1, dstColor 1); decay, downsample and blur
        // replace (srcColor 1, dstColor 0). Depth "always", no writes, no
        // culling: there is no depth buffer and the primitive state culls
        // nothing.
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let add = Some(wgpu::BlendState { color: add, alpha: add });
        let beam_buffers = [
            Some(wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x2] }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<BeamInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4],
            }),
        ];
        let post_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
        })];
        let pipeline = |name: &str, buffers: &[Option<wgpu::VertexBufferLayout>], format, blend, write_mask| {
            let (m, vs, fs) = &modules[name];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: None,
                vertex: wgpu::VertexState { module: m, entry_point: Some(vs), compilation_options: Default::default(), buffers },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let all = wgpu::ColorWrites::ALL;
        let beam = pipeline("beam", &beam_buffers, HDR_FORMAT, add, all);
        let decay = pipeline("decay", &post_buffers, HDR_FORMAT, None, all);
        let downsample = pipeline("downsample", &post_buffers, HDR_FORMAT, None, all);
        let blur = pipeline("blur", &post_buffers, HDR_FORMAT, None, all);
        // composite.json: "write": { "rgb": true, "alpha": false }.
        let composite = pipeline("composite", &post_buffers, OUTPUT_FORMAT, add, wgpu::ColorWrites::COLOR);
        // SAMPLE_FLAGS: U and V clamped, point mip; minification and
        // magnification linear (bgfx's default).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vector crt"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        // create_geometry: the post quad (position, texture coordinate; vtop
        // 0, as for every backend but OpenGL) and the unit beam quad.
        let (vtop, vbottom) = (0.0, 1.0);
        let post = [
            0.0, 0.0, 0.0, vtop, //
            1.0, 0.0, 1.0, vtop, //
            1.0, 1.0, 1.0, vbottom, //
            1.0, 1.0, 1.0, vbottom, //
            0.0, 1.0, 0.0, vbottom, //
            0.0, 0.0, 0.0, vtop,
        ];
        let beam_quad = [0.0, -1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0, -1.0];
        let buffer = |label, data: &[f32]| {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (data.len() * 4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&b, 0, &vertex_bytes(data));
            b
        };
        let post_vertices = buffer("post quad", &post);
        let beam_vertices = buffer("beam quad", &beam_quad);
        Gpu { device, queue, adapter, software, beam, decay, downsample, blur, composite, sampler, post_vertices, beam_vertices }
    }
}

/// A render target: `target` in vectorrenderer.h.
struct Target {
    view: wgpu::TextureView,
}

fn target(gpu: &Gpu, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, copy: bool) -> (wgpu::Texture, Target) {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    if copy {
        usage |= wgpu::TextureUsages::COPY_SRC;
    }
    let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let view = t.create_view(&wgpu::TextureViewDescriptor::default());
    (t, Target { view })
}

fn uniform(gpu: &Gpu, label: &str, size: u64) -> wgpu::Buffer {
    gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

fn vec4(v: [f32; 4]) -> Vec<u8> {
    vertex_bytes(&v)
}

/// A render pass into `view`, cleared first or not (bgfx's
/// `setViewClear` with or without `BGFX_CLEAR_COLOR`).
fn begin<'a>(encoder: &'a mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: bool) -> wgpu::RenderPass<'a> {
    begin_with(encoder, view, clear.then_some(wgpu::Color::TRANSPARENT))
}

fn begin_with<'a>(encoder: &'a mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: Option<wgpu::Color>) -> wgpu::RenderPass<'a> {
    let load = match clear {
        Some(c) => wgpu::LoadOp::Clear(c),
        None => wgpu::LoadOp::Load,
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// What one frame cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTime {
    /// Encoding and submitting the frame's passes.
    pub total: Duration,
    /// Waiting for a picture to be mapped (its GPU work and its copy to the
    /// readback buffer, as far as not done by then).
    pub wait: Duration,
    /// The mapped picture handed on (copied into the layer).
    pub copy: Duration,
    /// The beams drawn.
    pub beams: usize,
}

/// Readback buffers per tube: one in flight while the one before is read.
const RING: usize = 3;

/// A readback buffer's state.
const IDLE: u8 = 0;
const PENDING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

/// A readback buffer and its map request's state.
struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    index: Option<wgpu::SubmissionIndex>,
}

/// Which picture is read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The newest picture, waiting for the GPU (no latency; the GPU's time
    /// is the caller's).
    Latest,
    /// The picture of the frame before the newest, which the GPU finished
    /// during the frame in between: one frame of latency, no wait.
    Pipelined,
    /// Whatever is pending, newest included (the machine did not run: the
    /// picture catches up).
    Flush,
}

/// One screen's phosphor: `bgfx_vector_renderer`'s targets and state.
///
/// Tersenet: the phosphor and bloom targets may be smaller than the
/// picture (the internal resolution, [`Crt::new`]); the composite then
/// samples them (bilinear) into the picture's size. The picture is read
/// back through a ring of buffers (see [`Delivery`]).
pub struct Crt {
    /// The phosphor's (internal) size.
    width: u32,
    height: u32,
    /// The picture's size.
    out_width: u32,
    out_height: u32,
    bloom_width: u32,
    bloom_height: u32,
    accumulation: [Target; 2],
    bloom: [Target; 2],
    output_texture: wgpu::Texture,
    output: Target,
    ring: Vec<Readback>,
    /// Ring slots in flight, oldest first.
    flight: VecDeque<usize>,
    /// A picture was delivered since the tube was made.
    delivered: bool,
    padded_row: u32,
    current_accumulation: usize,
    reset_accumulation: bool,
    instances: Option<wgpu::Buffer>,
    beam_uniforms: wgpu::Buffer,
    beam_group: wgpu::BindGroup,
    decay_uniforms: wgpu::Buffer,
    /// Indexed by the previous accumulation target.
    decay_groups: [wgpu::BindGroup; 2],
    /// Indexed by the current accumulation target.
    downsample_groups: [wgpu::BindGroup; 2],
    blur_uniforms: [wgpu::Buffer; 2],
    blur_groups: [wgpu::BindGroup; 2],
    composite_groups: [wgpu::BindGroup; 2],
    /// Tersenet: clears rectangles of the bloom's first target after the
    /// downsample (the light of a machine's Surfaces' sources does not
    /// bloom): the decay effect with a factor of 0.
    zero_groups: [wgpu::BindGroup; 2],
    /// The bloom is drawn (shed: off).
    pub bloom_on: bool,
    pub params: Params,
    pub last: FrameTime,
}

/// Tersenet's additions to a tube, for terse-host's renderer (none for the
/// driver).
#[derive(Clone, Copy)]
pub struct Extras<'a> {
    /// The picture is read back ([`Crt::deliver`]); else it is only in
    /// [`Crt::output`], for the renderer to sample.
    pub readback: bool,
    /// The text's light (the picture's size, linear), added to the
    /// phosphor's in the composite times `text_gain`.
    pub text: Option<&'a wgpu::TextureView>,
    pub text_gain: f32,
    /// The composite's vignette (MAME's `u_composite.w`, 0 by default).
    pub vignette: f32,
}

impl Default for Extras<'_> {
    fn default() -> Self {
        Extras { readback: true, text: None, text_gain: 0.0, vignette: 0.0 }
    }
}

impl Crt {
    /// `create_targets`: two full-size and two half-size (rounded up)
    /// RGBA16F targets, and the accumulation to be cleared first.
    /// Tersenet: "full size" is `internal` (the phosphor's resolution),
    /// and the picture read back is `out`; with `bloom` false the bloom is
    /// not drawn (its strength 0).
    pub fn new(gpu: &Gpu, out: (u32, u32), internal: (u32, u32), params: Params, bloom: bool) -> Crt {
        Crt::with(gpu, out, internal, params, bloom, Extras::default())
    }

    /// [`Crt::new`] with Tersenet's additions ([`Extras`]).
    pub fn with(gpu: &Gpu, out: (u32, u32), internal: (u32, u32), params: Params, bloom: bool, extras: Extras) -> Crt {
        let (out_width, out_height) = (out.0.max(1), out.1.max(1));
        let (width, height) = (internal.0.max(1), internal.1.max(1));
        let bloom_width = width.div_ceil(2).max(1);
        let bloom_height = height.div_ceil(2).max(1);
        let accumulation = [target(gpu, "accumulation 0", width, height, HDR_FORMAT, false).1, target(gpu, "accumulation 1", width, height, HDR_FORMAT, false).1];
        let bloom_targets = [target(gpu, "bloom 0", bloom_width, bloom_height, HDR_FORMAT, false).1, target(gpu, "bloom 1", bloom_width, bloom_height, HDR_FORMAT, false).1];
        let (output_texture, output) = target(gpu, "output", out_width, out_height, OUTPUT_FORMAT, true);
        let padded_row = (out_width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let ring = (0..if extras.readback { RING } else { 0 })
            .map(|_| Readback {
                buffer: gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: (padded_row * out_height) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(IDLE)),
                index: None,
            })
            .collect();
        let group_with = |pipeline: &wgpu::RenderPipeline, label: &str, textures: &[(u32, &wgpu::TextureView)], uniforms: &[(u32, &wgpu::Buffer)]| {
            let layout = pipeline.get_bind_group_layout(0);
            let mut entries: Vec<wgpu::BindGroupEntry> = textures.iter().map(|(b, v)| wgpu::BindGroupEntry { binding: *b, resource: wgpu::BindingResource::TextureView(v) }).collect();
            entries.push(wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&gpu.sampler) });
            for (b, u) in uniforms {
                entries.push(wgpu::BindGroupEntry { binding: *b, resource: u.as_entire_binding() });
            }
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(label), layout: &layout, entries: &entries })
        };
        let group = |pipeline: &wgpu::RenderPipeline, label: &str, textures: &[(u32, &wgpu::TextureView)], u: &wgpu::Buffer| group_with(pipeline, label, textures, &[(2, u)]);
        let beam_uniforms = uniform(gpu, "beam uniforms", 96);
        let beam_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("beam"),
            layout: &gpu.beam.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: beam_uniforms.as_entire_binding() }],
        });
        let decay_uniforms = uniform(gpu, "u_decay", 16);
        let decay_groups = [0, 1].map(|p| group(&gpu.decay, "decay", &[(0, &accumulation[p].view)], &decay_uniforms));
        let source_texel = uniform(gpu, "u_source_texel", 16);
        gpu.queue.write_buffer(&source_texel, 0, &vec4([1.0 / width as f32, 1.0 / height as f32, 0.0, 0.0]));
        let downsample_groups = [0, 1].map(|c| group(&gpu.downsample, "downsample", &[(0, &accumulation[c].view)], &source_texel));
        let blur_uniforms = [uniform(gpu, "u_blur horizontal", 16), uniform(gpu, "u_blur vertical", 16)];
        let blur_groups =
            [group(&gpu.blur, "blur horizontal", &[(0, &bloom_targets[0].view)], &blur_uniforms[0]), group(&gpu.blur, "blur vertical", &[(0, &bloom_targets[1].view)], &blur_uniforms[1])];
        let composite_uniforms = uniform(gpu, "u_composite", 16);
        // composite(): u_composite = (bloom strength, exposure, 2.2, 0): the
        // vignette is off. Tersenet: a bloom shed is a strength of 0.
        // Tersenet: the vignette's strength is an extra (0, MAME's, by default).
        gpu.queue.write_buffer(&composite_uniforms, 0, &vec4([if bloom { params.bloom_strength } else { 0.0 }, params.exposure, 2.2, extras.vignette]));
        // Tersenet: the text's light and its gain (none for the driver).
        let text_uniforms = uniform(gpu, "u_text", 16);
        let no_text = target(gpu, "no text", 1, 1, HDR_FORMAT, false).1;
        let (text_view, gain) = match extras.text {
            Some(v) => (v, extras.text_gain),
            None => (&no_text.view, 0.0),
        };
        gpu.queue.write_buffer(&text_uniforms, 0, &vec4([gain, 0.0, 0.0, 0.0]));
        let composite_groups = [0, 1].map(|c| {
            group_with(
                &gpu.composite,
                "composite",
                &[(0, &accumulation[c].view), (3, &bloom_targets[0].view), (4, text_view)],
                &[(2, &composite_uniforms), (5, &text_uniforms)],
            )
        });
        let zero_uniforms = uniform(gpu, "u_decay zero", 16);
        gpu.queue.write_buffer(&zero_uniforms, 0, &vec4([0.0; 4]));
        let zero_groups = [0, 1].map(|p| group(&gpu.decay, "zero", &[(0, &accumulation[p].view)], &zero_uniforms));
        Crt {
            width,
            height,
            out_width,
            out_height,
            bloom_width,
            bloom_height,
            accumulation,
            bloom: bloom_targets,
            output_texture,
            output,
            ring,
            flight: VecDeque::new(),
            delivered: false,
            padded_row,
            current_accumulation: 0,
            reset_accumulation: true,
            instances: None,
            beam_uniforms,
            beam_group,
            decay_uniforms,
            decay_groups,
            downsample_groups,
            blur_uniforms,
            blur_groups,
            composite_groups,
            zero_groups,
            bloom_on: bloom,
            params,
            last: FrameTime::default(),
        }
    }

    /// The picture's size.
    pub fn size(&self) -> (u32, u32) {
        (self.out_width, self.out_height)
    }

    /// The phosphor's (internal) size.
    pub fn internal(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// `prepare` then `composite`, submitted: the picture is then read
    /// back by [`Crt::deliver`].
    ///
    /// `frame_time` is the emulated time since the last frame
    /// (`emu_time - m_last_emu_time`): `None` when the machine has not run
    /// since (MAME: "Emulation is paused or this display list has already
    /// been processed at the same emulated time. Do not excite it again."):
    /// nothing is drawn. It is capped at 0.100 s. The first frame after
    /// [`Crt::new`] clears the phosphor (and uses 1/60 s, as MAME's first
    /// frame does: the caller passes it). Tersenet: 0 draws the picture
    /// again from the phosphor as it is (no decay, no beam).
    pub fn frame(&mut self, gpu: &Gpu, beams: &[BeamInstance], frame_time: Option<f64>) {
        let Some(frame_time) = frame_time else { return };
        let start = Instant::now();
        // Tersenet: a readback buffer of the ring that is not in flight
        // (one always is: deliveries leave at most two in flight).
        let Some(slot) = (0..RING).find(|i| !self.flight.contains(i)) else { return };
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("vector crt") });
        self.encode(gpu, &mut encoder, beams, frame_time, &[]);
        encoder.copy_texture_to_buffer(
            self.output_texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.ring[slot].buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.padded_row), rows_per_image: None },
            },
            wgpu::Extent3d { width: self.out_width, height: self.out_height, depth_or_array_layers: 1 },
        );
        let index = gpu.queue.submit([encoder.finish()]);
        let rb = &mut self.ring[slot];
        rb.state.store(PENDING, Ordering::Release);
        let state = rb.state.clone();
        rb.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release));
        rb.index = Some(index);
        self.flight.push_back(slot);
        self.last = FrameTime { total: start.elapsed(), wait: Duration::ZERO, copy: Duration::ZERO, beams: beams.len() };
    }

    /// The composite's target: the picture (BGRA, gamma-encoded), for a
    /// renderer to sample or copy (Tersenet).
    pub fn output(&self) -> &wgpu::TextureView {
        &self.output.view
    }

    /// The composite's target texture.
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output_texture
    }

    /// `prepare` then `composite`, recorded into `encoder` (Tersenet: the
    /// renderer's, with its other passes). `no_bloom`: rectangles (x, y, w,
    /// h) of the bloom's half-size target cleared after the downsample.
    pub fn encode(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, beams: &[BeamInstance], frame_time: f64, no_bloom: &[(u32, u32, u32, u32)]) {
        // "Avoid an unusually large elapsed time causing undesirable behaviour."
        let frame_time = frame_time.clamp(0.0, 0.100);
        let p = self.params;
        let previous = self.current_accumulation;
        let current = previous ^ 1;
        let pass = begin;
        // Decay or clear the persistent phosphor target.
        {
            let mut rp = pass(encoder, &self.accumulation[current].view, self.reset_accumulation);
            if self.reset_accumulation {
                self.reset_accumulation = false;
            } else {
                // Exponential phosphor decay: I(t + dt) = I(t) * exp(-dt / tau);
                // m_persistence is tau in seconds.
                let tau = p.persistence.max(0.001);
                let decay = (-(frame_time as f32) / tau).exp();
                gpu.queue.write_buffer(&self.decay_uniforms, 0, &vec4([decay, 0.0, 0.0, 0.0]));
                rp.set_pipeline(&gpu.decay);
                rp.set_bind_group(0, &self.decay_groups[previous], &[]);
                rp.set_vertex_buffer(0, gpu.post_vertices.slice(..));
                rp.draw(0..6, 0..1);
            }
        }
        self.current_accumulation = current;
        // Add all vector beam instances to the same target.
        if !beams.is_empty() {
            let bytes: Vec<u8> = beams.iter().flat_map(|b| b.bytes()).collect();
            if self.instances.as_ref().is_none_or(|b| b.size() < bytes.len() as u64) {
                self.instances = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("beam instances"),
                    size: (bytes.len() as u64).next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let instances = self.instances.as_ref().unwrap();
            gpu.queue.write_buffer(instances, 0, &bytes);
            // bx::mtxOrtho(0, width, height, 0, 0, 100): target pixels, top left
            // at (0, 0), as a column-major matrix.
            let (w, h) = (self.width as f32, self.height as f32);
            let ortho = [2.0 / w, 0.0, 0.0, 0.0, 0.0, -2.0 / h, 0.0, 0.0, 0.0, 0.0, 0.01, 0.0, -1.0, 1.0, 0.0, 1.0];
            let mut u = vertex_bytes(&ortho);
            u.extend(vec4([frame_time as f32, p.persistence, p.beam_intensity, p.halo]));
            u.extend(vec4([w, h, 1.0 / w, 1.0 / h]));
            gpu.queue.write_buffer(&self.beam_uniforms, 0, &u);
            let mut rp = pass(encoder, &self.accumulation[current].view, false);
            rp.set_pipeline(&gpu.beam);
            rp.set_bind_group(0, &self.beam_group, &[]);
            rp.set_vertex_buffer(0, gpu.beam_vertices.slice(..));
            rp.set_vertex_buffer(1, instances.slice(..bytes.len() as u64));
            rp.draw(0..6, 0..beams.len() as u32);
        }
        if self.bloom_on {
            // Downsample the phosphor image into the first bloom target.
            {
                let mut rp = pass(encoder, &self.bloom[0].view, false);
                rp.set_pipeline(&gpu.downsample);
                rp.set_bind_group(0, &self.downsample_groups[current], &[]);
                rp.set_vertex_buffer(0, gpu.post_vertices.slice(..));
                rp.draw(0..6, 0..1);
            }
            // Tersenet: no bloom from where a machine's Surfaces' sources are.
            if !no_bloom.is_empty() {
                let mut rp = pass(encoder, &self.bloom[0].view, false);
                rp.set_pipeline(&gpu.decay);
                rp.set_bind_group(0, &self.zero_groups[current], &[]);
                rp.set_vertex_buffer(0, gpu.post_vertices.slice(..));
                for &(x, y, w, h) in no_bloom {
                    let (x, y) = (x.min(self.bloom_width), y.min(self.bloom_height));
                    let (w, h) = (w.min(self.bloom_width - x), h.min(self.bloom_height - y));
                    if w > 0 && h > 0 {
                        rp.set_scissor_rect(x, y, w, h);
                        rp.draw(0..6, 0..1);
                    }
                }
            }
            // "Scale the output-pixel bloom radius relative to a 1080-line reference."
            let bloom_scale = (self.height as f32 / 1080.0).clamp(0.25, 2.0);
            let bloom_radius = p.bloom_radius * bloom_scale;
            let sigma_x = bloom_radius * self.bloom_width as f32 / self.width as f32;
            let sigma_y = bloom_radius * self.bloom_height as f32 / self.height as f32;
            // "Gaussian variances add over repeated passes. Divide sigma so the
            // combined blur retains the radius requested by the slider."
            let pass_scale = 1.0 / (BLOOM_PASSES as f32).sqrt();
            gpu.queue.write_buffer(&self.blur_uniforms[0], 0, &vec4([1.0 / self.bloom_width as f32, 0.0, sigma_x * pass_scale, 0.0]));
            gpu.queue.write_buffer(&self.blur_uniforms[1], 0, &vec4([0.0, 1.0 / self.bloom_height as f32, sigma_y * pass_scale, 0.0]));
            for _ in 0..BLOOM_PASSES {
                for (dst, g) in [(1, 0), (0, 1)] {
                    let mut rp = pass(encoder, &self.bloom[dst].view, false);
                    rp.set_pipeline(&gpu.blur);
                    rp.set_bind_group(0, &self.blur_groups[g], &[]);
                    rp.set_vertex_buffer(0, gpu.post_vertices.slice(..));
                    rp.draw(0..6, 0..1);
                }
            }
        }
        // Composite persistent phosphor and bloom (added to the view, here
        // a black one). Tersenet: into the picture's size, sampling the
        // phosphor and the bloom.
        {
            let mut rp = begin_with(encoder, &self.output.view, Some(wgpu::Color::BLACK));
            rp.set_pipeline(&gpu.composite);
            rp.set_bind_group(0, &self.composite_groups[current], &[]);
            rp.set_vertex_buffer(0, gpu.post_vertices.slice(..));
            rp.draw(0..6, 0..1);
        }
    }

    /// Hands a submitted picture to `sink` (the mapped bytes, BGRA, a
    /// little-endian word 0xAARRGGBB a pixel, alpha 255, rows from the top
    /// `padded` bytes apart, the 1/2.2 gamma applied), as `how` says, and
    /// returns whether one was. Pictures older than the one handed on are
    /// dropped.
    pub fn deliver(&mut self, gpu: &Gpu, how: Delivery, sink: impl FnOnce(&[u8], usize)) -> bool {
        let n = self.flight.len();
        let pick = match how {
            _ if n == 0 => return false,
            Delivery::Latest | Delivery::Flush => n - 1,
            Delivery::Pipelined if n >= 2 => n - 2,
            // The tube's first picture is not kept waiting.
            Delivery::Pipelined if !self.delivered => n - 1,
            Delivery::Pipelined => return false,
        };
        let start = Instant::now();
        let slot = self.flight[pick];
        self.settle(gpu, slot);
        let waited = start.elapsed();
        let rb = &self.ring[slot];
        let mut handed = false;
        if rb.state.load(Ordering::Acquire) == MAPPED
            && let Ok(mapped) = rb.buffer.slice(..).get_mapped_range()
        {
            sink(&mapped, self.padded_row as usize);
            handed = true;
        }
        // This one and the older ones go back to the ring.
        for _ in 0..=pick {
            let s = self.flight.pop_front().unwrap();
            self.settle(gpu, s);
            let rb = &mut self.ring[s];
            if rb.state.load(Ordering::Acquire) == MAPPED {
                rb.buffer.unmap();
            }
            rb.state.store(IDLE, Ordering::Release);
            rb.index = None;
        }
        self.delivered |= handed;
        self.last.wait = waited;
        self.last.copy = start.elapsed() - waited;
        handed
    }

    /// Waits until slot `s`'s map request is answered.
    fn settle(&self, gpu: &Gpu, s: usize) {
        let rb = &self.ring[s];
        if rb.state.load(Ordering::Acquire) == PENDING {
            let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: rb.index.clone(), timeout: None });
        }
    }
}

impl Drop for Crt {
    fn drop(&mut self) {
        // Nothing is left mapped on buffers that go.
        for rb in &self.ring {
            if rb.state.load(Ordering::Acquire) == MAPPED {
                rb.buffer.unmap();
            }
        }
    }
}

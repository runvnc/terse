//! A scene's 3D lines drawn by the beam (the phosphor style's MAME model)
//! over its panels, on the GPU: the lines into a MAME tube the screen's size
//! (`mame::gpu`), its picture added over what the target holds there (light
//! adds), as `soft::scene` adds it in the software renderer.

use terse_core::Primitive;

use crate::mame::beams::{Screen, instances};
use crate::mame::gpu::Crt;
use crate::mame::params::Params;
use crate::renderer::Shaders;

const SHADER: &str = r#"
@group(0) @binding(0) var picture: texture_2d<f32>;
// x, y: the picture's top left in the target; z: 1 if the target is sRGB.
@group(0) @binding(1) var<uniform> u: vec4<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(floor(pos.xy - u.xy));
    let size = vec2<i32>(textureDimensions(picture));
    if any(at < vec2<i32>(0)) || any(at >= size) {
        return vec4<f32>(0.0);
    }
    var c = textureLoad(picture, at, 0).rgb;
    if u.z > 0.5 {
        c = to_linear(c);
    }
    return vec4<f32>(c, 0.0);
}
"#;

/// What a tube is made for: the picture's size and the sliders.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    size: (u32, u32),
    params: Params,
}

/// A scene's lines by the beam.
pub struct BeamLines {
    crt: Option<(Key, Crt)>,
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    uniform: Option<wgpu::Buffer>,
}

impl BeamLines {
    pub fn new() -> Self {
        BeamLines { crt: None, pipeline: None, uniform: None }
    }

    /// Draws `lines` (the scene's, screen units) by the beam into a tube the
    /// size of `rect` and adds its picture over `target` (`viewport`
    /// pixels, of `format`) there.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        shaders: &Shaders,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        lines: &[Primitive],
        rect: (f32, f32, f32, f32),
        viewport: (u32, u32),
        params: Params,
    ) {
        let device = shaders.device();
        let gpu = shaders.mame(queue);
        let size = ((rect.2.ceil() as u32).max(1), (rect.3.ceil() as u32).max(1));
        let key = Key { size, params };
        if self.crt.as_ref().is_none_or(|(k, _)| *k != key) {
            let (iw, ih) = crate::soft::mame::internal_size((size.0 as usize, size.1 as usize), crate::soft::mame::MAX_LINES);
            let extras = crate::mame::gpu::Extras { readback: false, ..Default::default() };
            self.crt = Some((key, Crt::with(&gpu, size, (iw as u32, ih as u32), params, true, extras)));
        }
        let (_, crt) = self.crt.as_mut().expect("made above");
        let (iw, ih) = crt.internal();
        let mut ls = Vec::new();
        Screen { size: (iw, ih) }.lines(lines, &mut ls);
        let beams = instances(&ls, params.beam_width);
        crt.encode(&gpu, encoder, &beams, crate::phosphor::FRAME_SECONDS as f64, &[]);
        if self.pipeline.as_ref().is_none_or(|(f, _)| *f != format) {
            let module = shaders.module("beam lines", || SHADER.to_string());
            let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("beam lines"),
                layout: None,
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState { color: add, alpha: add }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.pipeline = Some((format, pipeline));
        }
        let uniform = self.uniform.get_or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some("beam lines"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
        });
        let u = [rect.0.floor(), rect.1.floor(), if format.is_srgb() { 1.0 } else { 0.0 }, 0.0];
        queue.write_buffer(uniform, 0, &u.iter().flat_map(|v: &f32| v.to_le_bytes()).collect::<Vec<u8>>());
        let (_, pipeline) = self.pipeline.as_ref().expect("made above");
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("beam lines"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(crt.output()) },
                wgpu::BindGroupEntry { binding: 1, resource: uniform.as_entire_binding() },
            ],
        });
        let x0 = (rect.0.max(0.0).floor() as u32).min(viewport.0);
        let y0 = (rect.1.max(0.0).floor() as u32).min(viewport.1);
        let x1 = ((rect.0 + rect.2).ceil().max(0.0) as u32).min(viewport.0);
        let y1 = ((rect.1 + rect.3).ceil().max(0.0) as u32).min(viewport.1);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("beam lines"),
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
        pass.draw(0..3, 0..1);
    }
}

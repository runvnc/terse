//! `terse-host`: runs a Terse image in a window, or renders a screenshot.

mod app;
mod glyphs;
mod keymap;
mod renderer;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use terse_core::{FrameInput, Image, Machine, Terminal};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{ContentService, Node, parse_hex};

use crate::glyphs::SystemGlyphs;
use crate::renderer::Renderer;

/// Font size in logical pixels.
const FONT_PX: f32 = 18.0;

const USAGE: &str = "usage:
  terse-host <image|id> [--relay ws://...] [--store <dir>]
  terse-host <image|id> --screenshot <out.png> --frames <N> [--relay ws://...] [--store <dir>]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("terse-host: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let mut image_path = None;
    let mut screenshot = None;
    let mut frames = None;
    let (mut relay, mut store) = (None, None);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => screenshot = Some(it.next().ok_or(USAGE)?),
            "--relay" => relay = Some(it.next().ok_or(USAGE)?),
            "--store" => store = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            "--frames" => {
                let n = it.next().ok_or(USAGE)?;
                frames = Some(n.parse::<u64>().map_err(|_| format!("bad frame count {n}"))?);
            }
            _ if image_path.is_none() && !a.starts_with('-') => image_path = Some(a),
            _ => return Err(USAGE.into()),
        }
    }
    let path = image_path.ok_or(USAGE)?;
    // The blob store and relay, opened only when needed.
    let open_node = || Node::open(store.clone(), relay.as_deref(), DEFAULT_TIMEOUT);
    let mut node = None;
    // NETWORK 2: an image may be started by its ID.
    let bytes = match parse_hex(&path) {
        Some(id) if !Path::new(&path).exists() => node.insert(open_node()?).fetch_bytes(id)?,
        _ => std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?,
    };
    let image = Image::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let content = match (image.has_device(4), node) {
        (false, _) => None,
        (true, Some(node)) => Some(ContentService::start(node)),
        (true, None) => Some(ContentService::start(open_node()?)),
    };

    match (screenshot, frames) {
        (Some(out), Some(frames)) => {
            take_screenshot(&image, content, Path::new(&out), frames)?;
            Ok(ExitCode::SUCCESS)
        }
        (None, None) => app::run(image, content),
        _ => Err(USAGE.into()),
    }
}

/// The terminal to draw: an image without device 2 shows a blank grid.
fn terminal_or_blank(machine: &Machine) -> std::borrow::Cow<'_, Terminal> {
    match machine.terminal() {
        Some(t) => std::borrow::Cow::Borrowed(t),
        None => std::borrow::Cow::Owned(Terminal::default()),
    }
}

async fn request_device(
    adapter_options: wgpu::RequestAdapterOptions<'_, '_>,
    instance: &wgpu::Instance,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), String> {
    let adapter = match instance.request_adapter(&adapter_options).await {
        Ok(a) => a,
        // No hardware adapter: take whatever software adapter wgpu offers.
        Err(_) => instance
            .request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: true, ..adapter_options })
            .await
            .map_err(|e| format!("no graphics adapter: {e}"))?,
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| format!("cannot open graphics device: {e}"))?;
    Ok((adapter, device, queue))
}

/// Runs `frames` frames headless (no events, time 0, no random bytes, and
/// content requests completed at the next frame, like `terse run`) and renders
/// the terminal offscreen to a PNG.
fn take_screenshot(image: &Image, mut content: Option<ContentService>, out: &Path, frames: u64) -> Result<(), String> {
    let mut machine = Machine::new(image);
    let mut completions = Vec::new();
    for _ in 0..frames {
        if machine.run_frame(&FrameInput { completions: &completions, ..Default::default() }).exit.is_some() {
            break;
        }
        if let Some(content) = &mut content {
            content.submit(machine.take_content_requests());
            completions = content.wait_finished();
        }
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let (adapter, device, queue) = pollster::block_on(request_device(Default::default(), &instance))?;
    let info = adapter.get_info();
    eprintln!("terse-host: adapter {} ({:?}, {:?})", info.name, info.backend, info.device_type);

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::new(&device, format, Box::new(SystemGlyphs::new(FONT_PX)?));
    renderer.update(&queue, &terminal_or_blank(&machine));
    let (width, height) = renderer.grid_size();

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row_bytes = width * 4;
    let padded = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render(&queue, &mut encoder, &view, (width, height));
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
    let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let pixels: Vec<u8> = mapped
        .chunks(padded as usize)
        .flat_map(|row| &row[..row_bytes as usize])
        .copied()
        .collect();

    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&pixels).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    eprintln!("terse-host: wrote {} ({width}x{height}) after {} frames", out.display(), machine.frame());
    Ok(())
}

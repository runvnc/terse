//! `terse-host`: runs a Terse image in a window, or renders a screenshot.

mod app;
mod gamepad;
mod glyphs;
mod keymap;
mod phosphor;
mod renderer;
mod selftest;
mod vectors;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use terse_core::{Image, Machine, Script, Terminal};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::service::Headless;
use terse_net::{ContentService, Node, Subscriptions, default_config_dir, parse_hex};

use crate::glyphs::SystemGlyphs;
use crate::renderer::Renderer;

/// Font size in logical pixels.
const FONT_PX: f32 = 18.0;

/// The machine frames whose phosphor light (SPEC 8.7) a `--screenshot`
/// draws: a trail fades below 2% within 60 frames at persistence 240, and
/// below 10% within 600 at 255.
const PHOSPHOR_FRAMES: u64 = 600;

/// How long `--screenshot` waits for feeds to catch up before frame 0.
const SYNC_DEADLINE: Duration = Duration::from_secs(60);

const USAGE: &str = "usage:
  terse-host <image|id> [--no-trails] [--force-phosphor] [--font <family|file>] [--relay ws://...]
             [--store <dir>] [--config <dir>]
  terse-host <image|id> --screenshot <out.png> --frames <N> [--size <W>x<H>] [--events <file>] [--no-trails]
             [--force-phosphor] [--font <family|file>] [--relay ws://...] [--store <dir>] [--config <dir>]
  terse-host --selftest <image|id> [--frames <N>] [--events <file>] [--csv <file>] [--no-trails]
             [--force-phosphor] [--font <family|file>] [--store <dir>] [--config <dir>]
  --config holds the feed subscriptions (default $XDG_CONFIG_HOME/terse, else ~/.config/terse);
  --events is scripted input, as for `terse run`; --size is the picture's size in pixels
  (default: the grid at the font's size), the screen letterboxed in it at 4:3.
  --selftest runs N frames (default 200) in the window with only the scripted input (default:
  the image's name with .events, if that file exists) and prints per-frame timings;
  --csv writes them per frame. --no-trails: the phosphor display style (SPEC 8.7) without
  persistence. --force-phosphor: show the phosphor presentation (the whole screen as a glowing
  tube) whatever style the program sets; for testing. --font: draw the grid with this font (a
  family name as fc-list shows it, or a .ttf/.otf file) instead of the system's monospace font;
  a proportional font keeps the grid, each glyph centred in its cell.";

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
    let mut size = None;
    let (mut selftest, mut csv, mut no_trails, mut force_phosphor) = (false, None, false, false);
    let mut font = None;
    let (mut relay, mut store, mut config, mut events) = (None, None, None, None);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => screenshot = Some(it.next().ok_or(USAGE)?),
            "--selftest" => selftest = true,
            "--no-trails" => no_trails = true,
            "--force-phosphor" => force_phosphor = true,
            "--font" => font = Some(it.next().ok_or(USAGE)?),
            "--csv" => csv = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            "--relay" => relay = Some(it.next().ok_or(USAGE)?),
            "--store" => store = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            "--config" => config = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            "--events" => events = Some(it.next().ok_or(USAGE)?),
            "--size" => {
                let v = it.next().ok_or(USAGE)?;
                let parsed = v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                size = Some(parsed.filter(|&(w, h): &(u32, u32)| w > 0 && h > 0).ok_or(format!("bad size {v}"))?);
            }
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
    let content = match image.has_device(4) {
        false => None,
        true => {
            let mut node = match node {
                Some(node) => node,
                None => open_node()?,
            };
            // SPEC 7.7: the feeds are those of the subscription list, which
            // the node also follows on the relay (NETWORK 4.4).
            let dir = config.or_else(default_config_dir).ok_or("no config directory (use --config)")?;
            node.set_subscriptions(Subscriptions::load(dir.join("subscriptions"))?);
            Some(ContentService::start(node))
        }
    };

    if selftest {
        if screenshot.is_some() || size.is_some() {
            return Err(USAGE.into());
        }
        // The script beside the image (conform.terse: conform.events), unless given.
        let events = events.map(PathBuf::from).or_else(|| {
            Some(Path::new(&path).with_extension("events")).filter(|p| p.exists())
        });
        let script = match &events {
            Some(p) => {
                let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
                Script::parse(&text).map_err(|e| format!("{}: {e}", p.display()))?
            }
            None => Script::default(),
        };
        let frames = u32::try_from(frames.unwrap_or(200)).map_err(|_| "bad frame count")?;
        let test = selftest::SelfTest::new(script, frames, csv);
        return app::run(image, content, Some(test), Looks { no_trails, force_phosphor, font });
    }
    if csv.is_some() {
        return Err(USAGE.into());
    }
    match (screenshot, frames) {
        (Some(out), Some(frames)) => {
            let script = match &events {
                Some(path) => {
                    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                    Script::parse(&text).map_err(|e| format!("{path}: {e}"))?
                }
                None => Script::default(),
            };
            let looks = Looks { no_trails, force_phosphor, font };
            take_screenshot(&image, content, script, Path::new(&out), frames, size, looks)?;
            Ok(ExitCode::SUCCESS)
        }
        (None, None) if events.is_none() && size.is_none() => {
            app::run(image, content, None, Looks { no_trails, force_phosphor, font })
        }
        _ => Err(USAGE.into()),
    }
}

/// The user's presentation options.
pub struct Looks {
    pub no_trails: bool,
    pub force_phosphor: bool,
    /// `--font`: a family name or font file instead of the monospace font.
    pub font: Option<String>,
}

/// The terminal to draw: an image without device 2 shows a blank grid.
/// After a fault the bottom row shows the fault (presentation only: the
/// machine's grid is not changed).
fn displayed_terminal(machine: &Machine) -> std::borrow::Cow<'_, Terminal> {
    use std::borrow::Cow;
    let t = match machine.terminal() {
        Some(t) => Cow::Borrowed(t),
        None => Cow::Owned(Terminal::default()),
    };
    let Some(fault) = machine.fault() else { return t };
    let mut t = t.into_owned();
    t.set_colours(15, 1);
    t.set_cursor(0, terse_core::ROWS as u32 - 1);
    t.command(0x04);
    t.write(format!(" fault: {fault}").as_bytes());
    t.command(0x06);
    Cow::Owned(t)
}

/// Ends the process after a fault by signal SIGTERM, as `terse run` does,
/// so that no exit code can be mistaken for it.
fn fault_exit() -> ExitCode {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn raise(sig: i32) -> i32;
        }
        // SAFETY: raise only sends a signal to this process.
        unsafe {
            raise(15);
        }
    }
    ExitCode::from(255)
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

/// Runs `frames` frames headless, exactly as `terse run` does (scripted
/// events, time 0, no random bytes, content requests completed at the next
/// frame, feeds caught up first), and renders the terminal offscreen to a PNG.
/// In the phosphor presentation the afterglow is built up over the last
/// `PHOSPHOR_FRAMES` frames, as the window would have shown it.
fn take_screenshot(
    image: &Image,
    mut content: Option<ContentService>,
    script: Script,
    out: &Path,
    frames: u64,
    size: Option<(u32, u32)>,
    looks: Looks,
) -> Result<(), String> {
    if let Some(c) = content.as_mut().filter(|c| c.following())
        && let Err(e) = c.wait_caught_up(SYNC_DEADLINE)
    {
        eprintln!("terse-host: feeds: {e}; running with the entries held locally");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let (adapter, device, queue) = pollster::block_on(request_device(Default::default(), &instance))?;
    let info = adapter.get_info();
    eprintln!("terse-host: adapter {} ({:?}, {:?})", info.name, info.backend, info.device_type);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::new(&device, format, Box::new(SystemGlyphs::new(FONT_PX, looks.font.as_deref())?));
    renderer.set_no_trails(looks.no_trails);
    renderer.set_force_phosphor(looks.force_phosphor);
    let (width, height) = size.unwrap_or(renderer.grid_size());

    let mut machine = Machine::new(image);
    let mut headless = Headless::new(content, script);
    for f in 0..frames {
        let stopped = headless.frame(&mut machine).stopped.is_some();
        if frames - f <= PHOSPHOR_FRAMES {
            renderer.update_vectors(&queue, machine.vector());
            if renderer.phosphor() {
                renderer.update(&queue, &displayed_terminal(&machine));
                renderer.accumulate(&device, &queue, (width, height));
            }
        }
        if stopped {
            break;
        }
    }

    renderer.update(&queue, &displayed_terminal(&machine));
    renderer.update_vectors(&queue, machine.vector());
    // The beam of what is shown now (e.g. a fault reported after the last
    // frame), without advancing the afterglow.
    renderer.refresh_beam(&device, &queue, (width, height));

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
    renderer.render(&device, &queue, &mut encoder, &view, (width, height));
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

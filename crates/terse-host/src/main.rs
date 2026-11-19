//! `terse-host`: runs a Terse image in a window, or renders a screenshot.

mod app;
mod consent;
mod figures;
mod gamepad;
mod glyphs;
mod keymap;
mod page;
mod phosphor;
mod renderer;
mod screens;
mod selftest;
mod setup;
mod vectors;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use terse_core::{Image, Machine, Script, System, Terminal};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{
    HeadlessSystem, builtin,
    Consent, ContentService, Node, Subscriptions, default_config_dir, default_data_dir, default_store_dir, local,
    parse_hex,
};

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
  terse-host [<image|id> | --shell <image|id>] [--builtin <image|id>]... [--demo] [--relay ws://...|none]...
             [--no-feed-relays] [--no-prefetch] [--no-program-download] [--consent ask|allow|deny] [--store <dir>]
             [--config <dir>] [--data <dir>] [--no-trails] [--force-phosphor] [--font <family|file>] [--size <W>x<H>]
             [--page-sans <family|file>] [--page-serif <family|file>] [--page-mono <family|file>]
  terse-host [<image|id> | --shell [<image|id>]] --screenshot <out.png> --frames <N> [--size <W>x<H>]
             [--events <file>] [--demo] [--consent ask|allow|deny] [--relay ws://...] [--store <dir>]
             [--config <dir>] [--data <dir>] [--no-trails] [--force-phosphor] [--font <family|file>]
             [--builtin <image|id>]...
  terse-host --selftest <image|id> | --selftest --shell [<image|id>] [--frames <N>] [--events <file>] [--csv <file>]
             [--size <W>x<H>] [--consent allow|deny] [--no-trails] [--force-phosphor] [--font <family|file>]
             [--store <dir>] [--config <dir>] [--data <dir>] [--builtin <image|id>]...
  With no image, runs the shell built into terse-host (programs/shell, SPEC 12): it starts the
  built-in programs (reader, sparks, fetch, hello, and each --builtin image) as machines of their
  own and shows them as tiles. F12, the home key, gives the keyboard back to the shell (its
  launcher); F12 never reaches a program. --shell <image> runs another image as the shell; with
  --screenshot and --selftest, --shell alone is the built-in shell. <image> alone runs that image
  as a single machine, as before.
  --demo: before the window opens, make sure you have a feed key, subscribe to your own feed and,
  if it has no articles yet, publish four sample articles to it; then run the shell.
  Relays (windowed runs): the --relay options if given (several may be; none: no network); else
  the list in <config>/relays (`terse relay add <url>` writes it; an older <config>/relay is read
  if there is no list); else a relay already running on ws://127.0.0.1:7878; else one started
  inside terse-host on that port, with its store in $XDG_DATA_HOME/terse/relay (else
  ~/.local/share/terse/relay). One line on stderr says which. The host connects to all of them
  (at most 8) and, for feeds whose profile names relays, to up to two of those too, unless
  --no-feed-relays. --screenshot and --selftest use relays only with --relay.
  --no-prefetch: when an article is downloaded automatically, do not also fetch the first eight
  terse: links in it (NETWORK 4.4). --no-program-download: do not download the current version of
  each program of the subscribed feeds as it arrives (NETWORK 4.4).
  --data: where programs' named records are kept, in records/ (SPEC 9.6; default
  $XDG_DATA_HOME/terse, else ~/.local/share/terse). They never leave this machine.
  --consent: how requests by a program to subscribe, unsubscribe, publish, set the profile or set a nickname (SPEC 9.5) are
  answered: ask shows the host's prompt at the bottom of the window, naming the program (Y allows,
  N or Esc refuses); default ask in the window, deny for --screenshot and --selftest (where nobody
  can answer; a --screenshot with ask shows the prompt, unanswered).
  --store: the blob store (default $XDG_CACHE_HOME/terse/blobs, else ~/.cache/terse/blobs); the
  built-in programs are put into it at start. --config holds your feed key and subscriptions
  (default $XDG_CONFIG_HOME/terse, else ~/.config/terse). --events is scripted input, as for
  `terse run` (`home` is the home key). --size: the picture's size in pixels for --screenshot
  (default: the grid at the font's size), the window's for the other forms; the screen is
  letterboxed in it at 4:3.
  --selftest runs N frames (default 200) in the window with only the scripted input (default:
  the image's name with .events, if that file exists) and prints per-frame timings;
  --csv writes them per frame. --no-trails: the phosphor display style (SPEC 10.6) without
  persistence. --force-phosphor: show the phosphor presentation (the whole screen as a glowing
  tube) whatever style the program sets; for testing. --font: draw the grid with this font (a
  family name as fc-list shows it, or a .ttf/.otf file) instead of the system's monospace font;
  a proportional font keeps the grid, each glyph centred in its cell. --page-sans, --page-serif,
  --page-mono: the fonts of the page device's three faces (default: the system's sans-serif, serif
  and monospace families); every form of the command takes them.
  Paste with Ctrl+V or Shift+Insert: the clipboard's text goes to the focused program as typed text.";

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
    let mut page_fonts = page::PageFonts::default();
    let (mut relays, mut store, mut config, mut events) = (Vec::new(), None, None, None);
    let (mut feed_relays, mut prefetch, mut program_download, mut data) = (true, true, true, None);
    let (mut consent, mut demo) = (None, false);
    let (mut shell_flag, mut extra_builtins) = (false, Vec::new());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => screenshot = Some(it.next().ok_or(USAGE)?),
            "--selftest" => selftest = true,
            "--demo" => demo = true,
            "--shell" => shell_flag = true,
            "--builtin" => extra_builtins.push(it.next().ok_or(USAGE)?),
            "--consent" => {
                let v = it.next().ok_or(USAGE)?;
                consent = Some(Consent::parse(&v).ok_or(format!("bad --consent {v} (ask, allow or deny)"))?);
            }
            "--no-trails" => no_trails = true,
            "--force-phosphor" => force_phosphor = true,
            "--font" => font = Some(it.next().ok_or(USAGE)?),
            "--page-sans" => page_fonts.sans = Some(it.next().ok_or(USAGE)?),
            "--page-serif" => page_fonts.serif = Some(it.next().ok_or(USAGE)?),
            "--page-mono" => page_fonts.mono = Some(it.next().ok_or(USAGE)?),
            "--csv" => csv = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            "--relay" => relays.push(it.next().ok_or(USAGE)?),
            "--no-feed-relays" => feed_relays = false,
            "--no-prefetch" => prefetch = false,
            "--no-program-download" => program_download = false,
            "--data" => data = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
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
    let windowed = !selftest && screenshot.is_none();
    // No image: the built-in shell. --shell: the image named (if any) is
    // the shell.
    let shell = shell_flag || image_path.is_none();
    if (selftest && image_path.is_none() && !shell_flag)
        || (demo && (selftest || (image_path.is_some() && !shell_flag)))
        || (!shell && !extra_builtins.is_empty())
    {
        return Err(USAGE.into());
    }
    let consent = match consent {
        Some(Consent::Ask) if selftest => return Err("--selftest cannot ask for consent (allow or deny)".into()),
        Some(c) => c,
        None if windowed => Consent::Ask,
        None => Consent::Deny,
    };
    let config = config.or_else(default_config_dir).ok_or("no config directory (use --config)")?;
    let store = store.or_else(default_store_dir).ok_or("no blob store directory (use --store)")?;
    let data = data.or_else(default_data_dir).ok_or("no data directory (use --data)")?;
    // The relay: in a window, found or started as README "Getting started"
    // says; otherwise only --relay.
    let relays = if windowed {
        let chosen = setup::choose_relay(&relays, &config, local::local_port(), default_data_dir().as_deref())?;
        eprintln!("terse-host: {}", chosen.describe());
        chosen.urls()
    } else if relays.is_empty() {
        Vec::new()
    } else {
        local::find_relays(&relays, None, 0)?.urls()
    };
    if demo {
        setup::prepare_demo(&store, &config, &relays)?;
    }
    // The blob store and relays, opened only when needed.
    let open_node = || {
        let mut node = Node::open(Some(store.clone()), &relays, DEFAULT_TIMEOUT)?;
        node.set_feed_relays(feed_relays);
        node.set_prefetch(prefetch);
        node.set_program_download(program_download);
        node.set_records(terse_net::Records::in_data_dir(&data));
        Ok::<Node, String>(node)
    };
    let mut node = None;
    let path = image_path.clone().unwrap_or_else(|| "shell".into());
    // NETWORK 2: an image may be started by its ID.
    let image_bytes = |arg: &str, node: &mut Option<Node>| -> Result<Vec<u8>, String> {
        let bytes = match parse_hex(arg) {
            Some(id) if !Path::new(arg).exists() => match node {
                Some(n) => n.fetch_bytes(id)?,
                None => node.insert(open_node()?).fetch_bytes(id)?,
            },
            _ => std::fs::read(arg).map_err(|e| format!("{arg}: {e}"))?,
        };
        Image::parse(&bytes).map_err(|e| format!("{arg}: {e}"))?;
        Ok(bytes)
    };
    let bytes = match &image_path {
        None => builtin::SHELL.to_vec(),
        Some(p) => image_bytes(p, &mut node)?,
    };
    let image = Image::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    // SPEC 12.2: the built-in programs, in the store for the shell.
    let mut builtins = Vec::new();
    if shell {
        let extra = extra_builtins.iter().map(|a| image_bytes(a, &mut node)).collect::<Result<Vec<_>, _>>()?;
        let n = match &node {
            Some(n) => n,
            None => node.insert(open_node()?),
        };
        builtins = builtin::install(n.store(), &extra)?;
    }
    let content = match image.has_device(4) || shell {
        false => None,
        true => {
            let mut node = match node {
                Some(node) => node,
                None => open_node()?,
            };
            // SPEC 9.5: the feeds are those of the subscription list, which
            // the node also follows on the relay (NETWORK 4.4); publishing
            // uses (and if need be creates) the identity in the config
            // directory.
            node.set_subscriptions(Subscriptions::load(config.join("subscriptions"))?);
            node.set_identity_path(config.join("identity.key"))?;
            node.set_nicknames(terse_net::profile::Nicknames::load_from_config(&config)?);
            Some(ContentService::start_with_consent(node, consent))
        }
    };
    // The built-in shell has the identity of a built-in (SPEC 12.2).
    let identity = image_path.is_none().then(|| terse_core::ProgramId { publisher: [0; 32], name: image.title.clone() });
    let program = app::Program { image, shell, builtins, identity };

    if selftest {
        if screenshot.is_some() {
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
        let looks = Looks { no_trails, force_phosphor, font, page_fonts, window: size };
        return app::run(program, content, Some(test), looks);
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
            let looks = Looks { no_trails, force_phosphor, font, page_fonts, window: None };
            take_screenshot(program, content, script, Path::new(&out), frames, size, looks)?;
            Ok(ExitCode::SUCCESS)
        }
        (None, None) if events.is_none() => {
            app::run(program, content, None, Looks { no_trails, force_phosphor, font, page_fonts, window: size })
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
    /// `--page-sans`, `--page-serif`, `--page-mono`: the page's fonts.
    pub page_fonts: page::PageFonts,
    /// `--size` of a window.
    pub window: Option<(u32, u32)>,
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
/// events, time 0, no random bytes, content requests and images completed at
/// the next frame, feeds caught up first), and renders the screens offscreen
/// to a PNG. In the phosphor presentation the afterglow is built up over the
/// last `PHOSPHOR_FRAMES` frames, as the window would have shown it.
fn take_screenshot(
    program: app::Program,
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
    let renderer = Renderer::new(&device, format, Box::new(SystemGlyphs::new(FONT_PX, looks.font.as_deref())?));
    let (width, height) = size.unwrap_or(renderer.grid_size());
    let viewport = (width, height);
    let mut screens = screens::Screens::new(renderer, format);
    screens.set_no_trails(looks.no_trails);
    screens.set_force_phosphor(looks.force_phosphor);
    // SPEC 11: the page laid out with this host's fonts, between frames, so
    // the run is reproducible and the picture shows the finished layout.
    screens.set_page_animation(false);

    let image = &program.image;
    let mut system = System::new(image, program.shell, program.builtins);
    system.set_shell_identity(program.identity);
    let mut headless = HeadlessSystem::new(content, script);
    // Each machine's page has its own layouter; the fonts are shared.
    type Latest = Arc<Mutex<Option<Arc<page::HostLayout>>>>;
    let latest: Arc<Mutex<HashMap<u32, Latest>>> = Arc::default();
    if image.has_device(7) || program.shell {
        let typesetter = page::Typesetter::new(&looks.page_fonts)?;
        eprintln!("terse-host: page fonts: {}", typesetter.describe());
        let typesetter = Arc::new(Mutex::new(typesetter));
        let latest = latest.clone();
        headless = headless.with_layouter(Box::new(move |serial| {
            let layouter = page::SyncLayouter::shared(typesetter.clone());
            latest.lock().unwrap().insert(serial, layouter.latest.clone());
            Box::new(layouter)
        }));
    }
    let shown = |serial: u32| latest.lock().unwrap().get(&serial).and_then(|l| l.lock().unwrap().clone());
    for f in 0..frames {
        let done = headless.frame(&mut system);
        let stopped = system.stopped().is_some();
        if frames - f <= PHOSPHOR_FRAMES {
            let mut ran: Vec<u32> = done.shell.iter().map(|_| 0).collect();
            ran.extend(done.children.iter().filter_map(|(h, _)| Some(system.child(*h)?.serial)));
            screens.update(&device, &queue, &system, viewport, &ran, &shown, None);
        }
        if stopped {
            break;
        }
    }

    // A consent prompt still waiting for an answer (`--consent ask`) is
    // shown as the window would show it.
    let prompt = headless.content().and_then(|c| {
        let p = c.prompt()?;
        let title = match p.tag {
            0 => image.title_lossy(),
            tag => system.children().find(|(_, c)| c.serial == tag).map_or(String::new(), |(_, c)| c.title.clone()),
        };
        Some((p, c.prompts_waiting(), title))
    });
    screens.update(&device, &queue, &system, viewport, &[], &shown, prompt);
    // The beam of what is shown now (e.g. a fault reported after the last
    // frame), without advancing the afterglow.
    screens.refresh_beams(&device, &queue, viewport);

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
    screens.render(&device, &queue, &mut encoder, &view, viewport);
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
    eprintln!("terse-host: wrote {} ({width}x{height}) after {} frames", out.display(), system.shell().frame());
    Ok(())
}

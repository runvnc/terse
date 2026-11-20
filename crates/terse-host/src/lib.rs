//! `terse-host`: runs a Terse image in a window, or renders a screenshot.
//! The binary (`src/main.rs`) calls [`main`]; the library also lends its
//! software renderer's pieces (glyphs, page layout, shapes) to other hosts.

// Without the window, the windowed host's machinery (the page layout
// thread, input) waits for the bootable image's KMS/DRM front end.
#![cfg_attr(not(feature = "window"), allow(dead_code))]

#[cfg(feature = "window")]
mod app;
#[cfg(feature = "audio")]
mod audio;
pub mod consent;
pub mod display;
mod figures;
#[cfg(feature = "window")]
mod gamepad;
pub mod glyphs;
#[cfg(feature = "input")]
pub mod keymap;
pub mod page;
#[cfg(feature = "gpu")]
mod panels;
mod phosphor;
#[cfg(test)]
mod reference;
#[cfg(feature = "gpu")]
mod renderer;
#[cfg(feature = "gpu")]
mod screens;
#[cfg(feature = "window")]
mod selftest;
mod setup;
pub mod soft;
#[cfg(feature = "gpu")]
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

use crate::display::{Display, RendererChoice};
use crate::glyphs::SystemGlyphs;

/// Font size in logical pixels.
pub const FONT_PX: f32 = 18.0;

/// The machine frames whose phosphor light (SPEC 10.6) a `--screenshot`
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
             [--parallax off|pointer] [--eye <dx>,<dy>] [--panels perspective|flat|auto]
  terse-host [<image|id> | --shell [<image|id>]] --screenshot <out.png> --frames <N> [--size <W>x<H>] [--eye <dx>,<dy>]
             [--events <file>] [--demo] [--consent ask|allow|deny] [--relay ws://...] [--store <dir>]
             [--config <dir>] [--data <dir>] [--no-trails] [--force-phosphor] [--font <family|file>]
             [--builtin <image|id>]...
  terse-host --selftest <image|id> | --selftest --shell [<image|id>] [--frames <N>] [--events <file>] [--csv <file>]
             [--size <W>x<H>] [--consent allow|deny] [--no-trails] [--force-phosphor] [--font <family|file>]
             [--store <dir>] [--config <dir>] [--data <dir>] [--builtin <image|id>]...
  terse-host --bench-render [<image|id> | --shell [<image|id>]] --frames <N> [--warmup <N>] [--size <W>x<H>]
             [--events <file>] [--renderer gpu|software] [--threads <n>] [--sweep-eye] [--consent allow|deny] [...]
  3D vector display (device 9, SPEC 13): in a window the eye follows the pointer (parallax): the
  pointer at the picture's centre is the reference eye, at an edge the eye is 160 units that way;
  --parallax off keeps it still. --eye <dx>,<dy> (screen units, each -160 to 160, default 0,0)
  puts the eye there for --screenshot and --bench-render (and keeps it there with --parallax off).
  --panels: how the software renderer draws the machines the shell shows as panels in its 3D
  scene (SPEC 13.3 Screen): in perspective (perspective, auto: the default) or flat, upright in
  the rectangle the panel covers; the GPU renderer always draws them in perspective.
  Every form also takes --renderer gpu|software|auto (default auto: the GPU if a hardware graphics
  adapter is found, else the software renderer) and --threads <n> (the software renderer's drawing
  threads; default one per core).
  With no image, runs the shell built into terse-host (programs/shell, SPEC 12): it starts the
  built-in programs (reader, sparks, fetch, hello, sketch, holo, and each --builtin image) as machines of their
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
  --bench-render runs the image (or the system) headless like --screenshot, with the page's scroll
  animated as in a window, and draws every frame offscreen with the chosen renderer, timing only the
  drawing (taking the machines' screens and drawing the picture; a GPU frame is timed until the GPU
  has finished it): it prints p50, p99 and max per frame over the N frames after --warmup frames
  (default 60), and how many took over 16.7 ms. No window is opened. --sweep-eye moves the eye every
  frame (round an ellipse 160 units to either side and 80 up and down, once every 120 frames), as a
  pointer moving over the window would, so 3D lists are projected again each frame.
  Paste with Ctrl+V or Shift+Insert: the clipboard's text goes to the focused program as typed text.";

/// The `terse-host` command.
pub fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("terse-host: {e}");
            ExitCode::from(2)
        }
    }
}

/// `terse-host` with these arguments (for tests that need its pictures).
pub fn run(args: Vec<String>) -> Result<ExitCode, String> {
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
    let (mut renderer, mut threads, mut bench, mut warmup, mut sweep) = (RendererChoice::Auto, 0usize, false, None, false);
    let mut bloom = soft::BloomRes::Auto;
    let (mut eye, mut parallax) = ((0.0f32, 0.0f32), true);
    let mut panels = soft::panel::PanelMode::Perspective;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => screenshot = Some(it.next().ok_or(USAGE)?),
            "--selftest" => selftest = true,
            "--bench-render" => bench = true,
            "--sweep-eye" => sweep = true,
            "--renderer" => {
                let v = it.next().ok_or(USAGE)?;
                renderer = RendererChoice::parse(&v).ok_or(format!("bad --renderer {v} (gpu, software or auto)"))?;
            }
            "--threads" => {
                let v = it.next().ok_or(USAGE)?;
                threads = v.parse().map_err(|_| format!("bad --threads {v}"))?;
            }
            "--bloom" => {
                bloom = match it.next().ok_or(USAGE)?.as_str() {
                    "full" => soft::BloomRes::Full,
                    "half" => soft::BloomRes::Half,
                    "auto" => soft::BloomRes::Auto,
                    v => return Err(format!("bad --bloom {v} (full, half or auto)")),
                };
            }
            "--warmup" => {
                let v = it.next().ok_or(USAGE)?;
                warmup = Some(v.parse::<u64>().map_err(|_| format!("bad --warmup {v}"))?);
            }
            "--eye" => {
                let v = it.next().ok_or(USAGE)?;
                let max = terse_core::vector3::MAX_EYE_OFFSET;
                let parsed = v.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)));
                eye = parsed
                    .filter(|&(x, y)| x.abs() <= max && y.abs() <= max)
                    .ok_or(format!("bad --eye {v} (<dx>,<dy>, each from -{max} to {max})"))?;
            }
            "--panels" => {
                panels = match it.next().ok_or(USAGE)?.as_str() {
                    "perspective" | "auto" => soft::panel::PanelMode::Perspective,
                    "flat" => soft::panel::PanelMode::Flat,
                    v => return Err(format!("bad --panels {v} (perspective, flat or auto)")),
                };
            }
            "--parallax" => {
                parallax = match it.next().ok_or(USAGE)?.as_str() {
                    "pointer" => true,
                    "off" => false,
                    v => return Err(format!("bad --parallax {v} (off or pointer)")),
                };
            }
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
    if (bench && (selftest || screenshot.is_some() || frames.is_none())) || (sweep && !bench) {
        return Err(USAGE.into());
    }
    let windowed = !selftest && !bench && screenshot.is_none();
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
        node.set_snapshots(terse_net::Snapshots::in_data_dir(&data));
        Ok::<Node, String>(node)
    };
    let mut node = None;
    let path = image_path.clone().unwrap_or_else(|| "shell".into());
    // NETWORK 7: an image may be started by its ID.
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
            if !shell {
                // The manual, which every host carries.
                builtin::install_manual(node.store())?;
            }
            node.set_subscriptions(Subscriptions::load(config.join("subscriptions"))?);
            node.set_identity_path(config.join("identity.key"))?;
            node.set_nicknames(terse_net::profile::Nicknames::load_from_config(&config)?);
            Some(ContentService::start_with_consent(node, consent))
        }
    };
    // The built-in shell has the identity of a built-in (SPEC 12.2).
    let identity = image_path.is_none().then(|| terse_core::ProgramId { publisher: [0; 32], name: image.title.clone() });
    let program = Program { image, shell, builtins, identity, data };

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
        #[cfg(feature = "window")]
        let test = selftest::SelfTest::new(script, frames, csv);
        #[cfg(not(feature = "window"))]
        let test = {
            let _ = (script, frames, csv);
            SelfTestStub
        };
        let looks = Looks { no_trails, force_phosphor, font, page_fonts, window: size, renderer, threads, bloom, eye, parallax, panels };
        return run_window(program, content, Some(test), looks);
    }
    if csv.is_some() {
        return Err(USAGE.into());
    }
    let script = || -> Result<Script, String> {
        match &events {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                Script::parse(&text).map_err(|e| format!("{path}: {e}"))
            }
            None => Ok(Script::default()),
        }
    };
    if bench {
        let looks = Looks { no_trails, force_phosphor, font, page_fonts, window: None, renderer, threads, bloom, eye, parallax, panels };
        let mode = Headless::Bench { warmup: warmup.unwrap_or(60), frames: frames.unwrap_or(0), sweep };
        run_headless(program, content, script()?, mode, size, looks)?;
        return Ok(ExitCode::SUCCESS);
    }
    match (screenshot, frames) {
        (Some(out), Some(frames)) => {
            let looks = Looks { no_trails, force_phosphor, font, page_fonts, window: None, renderer, threads, bloom, eye, parallax, panels };
            let mode = Headless::Screenshot { frames };
            let (pixels, (width, height), frame) = run_headless(program, content, script()?, mode, size, looks)?.expect("a picture");
            write_png(Path::new(&out), &pixels, (width, height))?;
            eprintln!("terse-host: wrote {out} ({width}x{height}) after {frame} frames");
            Ok(ExitCode::SUCCESS)
        }
        (None, None) if events.is_none() => {
            run_window(program, content, None, Looks { no_trails, force_phosphor, font, page_fonts, window: size, renderer, threads, bloom, eye, parallax, panels })
        }
        _ => Err(USAGE.into()),
    }
}

/// What to run: an image, as the shell (with the built-in programs' IDs)
/// or as a single machine.
pub struct Program {
    pub image: Image,
    pub shell: bool,
    pub builtins: Vec<[u8; 32]>,
    /// The built-in shell's identity (SPEC 12.2), for its named records.
    pub identity: Option<terse_core::ProgramId>,
    /// The data directory: the shell's snapshots (SPEC 12.7).
    pub data: PathBuf,
}

#[cfg(feature = "window")]
fn run_window(program: Program, content: Option<ContentService>, test: Option<selftest::SelfTest>, looks: Looks) -> Result<ExitCode, String> {
    app::run(program, content, test, looks)
}

/// Without the window feature (the bootable image's build, which is to
/// show its frames on a KMS/DRM display through `soft::FrameSink`) there
/// is no window yet.
#[cfg(not(feature = "window"))]
fn run_window(_: Program, _: Option<ContentService>, _: Option<SelfTestStub>, _: Looks) -> Result<ExitCode, String> {
    Err("this terse-host was built without a window (feature \"window\"): use --screenshot or --bench-render".into())
}

#[cfg(not(feature = "window"))]
struct SelfTestStub;

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
    /// `--renderer`.
    pub renderer: RendererChoice,
    /// `--threads`: the software renderer's drawing threads (0: one per core).
    pub threads: usize,
    /// `--bloom`: the software renderer's phosphor bloom resolution.
    pub bloom: soft::BloomRes,
    /// `--eye`: where the eye is for 3D lists (SPEC 13.1), screen units
    /// from the reference eye; in a window with parallax, where it rests.
    pub eye: display::Eye,
    /// `--parallax pointer` (the default): in a window the eye follows the
    /// pointer; `off`: it stays at `eye`.
    pub parallax: bool,
    /// `--panels`: how the software renderer draws the shell's panels
    /// (SPEC 12.3); the GPU renderer draws them in perspective.
    pub panels: soft::panel::PanelMode,
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

#[cfg(feature = "window")]
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

#[cfg(feature = "gpu")]
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

/// What a headless run does with the pictures.
enum Headless {
    /// Returns the last frame's picture.
    Screenshot { frames: u64 },
    /// Draws every frame offscreen and prints how long that took; with
    /// `sweep` the eye moves every frame.
    Bench { warmup: u64, frames: u64, sweep: bool },
}

/// The renderer `--renderer` asks for, offscreen.
fn headless_display(looks: &Looks) -> Result<Box<dyn Display>, String> {
    let glyphs = SystemGlyphs::new(FONT_PX, looks.font.as_deref())?;
    if looks.renderer != RendererChoice::Software {
        #[cfg(feature = "gpu")]
        {
            match gpu_display(glyphs.clone(), looks.renderer == RendererChoice::Auto) {
                Ok(d) => return Ok(d),
                Err(e) if looks.renderer == RendererChoice::Auto => {
                    eprintln!("terse-host: {e}; using the software renderer")
                }
                Err(e) => return Err(e),
            }
        }
        #[cfg(not(feature = "gpu"))]
        if looks.renderer == RendererChoice::Gpu {
            return Err("this terse-host was built without the GPU renderer (feature \"gpu\")".into());
        }
    }
    let mut display = soft::SoftScreens::new(Box::new(glyphs), looks.threads);
    display.set_bloom(looks.bloom);
    display.set_panel_mode(looks.panels);
    Ok(Box::new(display))
}

/// The GPU renderer drawing offscreen; with `hardware_only` (`--renderer
/// auto`), an error if the only adapter is a software one.
#[cfg(feature = "gpu")]
fn gpu_display(glyphs: SystemGlyphs, hardware_only: bool) -> Result<Box<dyn Display>, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let (adapter, device, queue) = pollster::block_on(request_device(Default::default(), &instance))?;
    let info = adapter.get_info();
    if hardware_only && info.device_type == wgpu::DeviceType::Cpu {
        return Err(format!("no graphics adapter but a software one ({})", info.name));
    }
    eprintln!("terse-host: adapter {} ({:?}, {:?})", info.name, info.backend, info.device_type);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let renderer = renderer::Renderer::new(&renderer::Shaders::new(&device), format, Box::new(glyphs));
    let screens = screens::Screens::new(renderer, format);
    Ok(Box::new(screens::GpuDisplay::new(device, queue, screens, format, info.name)))
}

/// p50, p99 and max of `values` (ms).
fn spread(mut values: Vec<f64>) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.99), values[values.len() - 1])
}

/// Runs the frames headless, exactly as `terse run` does (scripted events,
/// time 0, no random bytes, content requests and images completed at the
/// next frame, feeds caught up first), and draws the screens offscreen: a
/// PNG of the last frame (in the phosphor presentation the afterglow is
/// built up over the last `PHOSPHOR_FRAMES` frames, as the window would
/// have shown it), or every frame, timed.
fn run_headless(
    program: Program,
    mut content: Option<ContentService>,
    script: Script,
    mode: Headless,
    size: Option<(u32, u32)>,
    looks: Looks,
) -> Result<Option<(Vec<u8>, (u32, u32), u64)>, String> {
    if let Some(c) = content.as_mut().filter(|c| c.following())
        && let Err(e) = c.wait_caught_up(SYNC_DEADLINE)
    {
        eprintln!("terse-host: feeds: {e}; running with the entries held locally");
    }
    let mut display = headless_display(&looks)?;
    let bench = matches!(mode, Headless::Bench { .. });
    // SPEC 11: a screenshot shows the finished page (no scroll animation,
    // figures rasterised at once); a benchmark draws as a window does.
    display.configure(looks.no_trails, looks.force_phosphor, bench);
    display.set_eye(looks.eye);
    let viewport = size.unwrap_or(display.grid_size());
    eprintln!("terse-host: renderer {}, {}x{}", display.name(), viewport.0, viewport.1);

    let image = &program.image;
    let mut system = System::new(image, program.shell, program.builtins);
    system.set_shell_identity(program.identity);
    system.set_eye(looks.eye);
    terse_net::snapshots::load_table(&mut system, &program.data);
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
    let total = match mode {
        Headless::Screenshot { frames, .. } => frames,
        Headless::Bench { warmup, frames, .. } => warmup + frames,
    };
    let mut times = Vec::new();
    for f in 0..total {
        let done = headless.frame(&mut system);
        let stopped = system.stopped().is_some();
        let mut ran: Vec<u32> = done.shell.iter().map(|_| 0).collect();
        ran.extend(done.children.iter().filter_map(|(h, _)| Some(system.child(*h)?.serial)));
        match &mode {
            Headless::Screenshot { .. } => {
                if total - f <= PHOSPHOR_FRAMES {
                    display.update(&system, viewport, &ran, &shown, None);
                }
            }
            Headless::Bench { warmup, sweep, .. } => {
                if *sweep {
                    let a = f as f32 / 120.0 * std::f32::consts::TAU;
                    let eye = (160.0 * a.cos(), 80.0 * a.sin());
                    display.set_eye(eye);
                    system.set_eye(eye);
                }
                let start = std::time::Instant::now();
                display.update(&system, viewport, &ran, &shown, None);
                display.render_offscreen(viewport)?;
                if f >= *warmup {
                    times.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
        }
        if stopped {
            break;
        }
    }
    if bench {
        let n = times.len();
        let mean = times.iter().sum::<f64>() / n.max(1) as f64;
        let over = times.iter().filter(|&&t| t > 1000.0 / 60.0).count();
        let (p50, p99, max) = spread(times);
        println!(
            "{} {}x{}: {n} frames: p50 {p50:.2} ms, p99 {p99:.2} ms, max {max:.2} ms, mean {mean:.2} ms, over 16.7 ms: {over}",
            display.name(),
            viewport.0,
            viewport.1
        );
        return Ok(None);
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
    display.update(&system, viewport, &[], &shown, prompt);
    // The beam of what is shown now (e.g. a fault reported after the last
    // frame), without advancing the afterglow.
    display.refresh_beams(viewport);
    let pixels = display.snapshot(viewport)?;
    Ok(Some((pixels, viewport, system.shell().frame() as u64)))
}

/// Writes RGBA pixels (8 bits a channel, sRGB) to a PNG file.
fn write_png(out: &Path, pixels: &[u8], (width, height): (u32, u32)) -> Result<(), String> {
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

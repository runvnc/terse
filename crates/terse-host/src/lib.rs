//! `terse-host`: runs a Terse image in a window, or renders a screenshot.
//! The binary (`src/main.rs`) calls [`main`]; the library also lends its
//! software renderer's pieces (glyphs, page layout, shapes) to other hosts.

// Without the window, the windowed host's machinery (the page layout
// thread, input) waits for the bootable image's KMS/DRM front end.
#![cfg_attr(not(feature = "window"), allow(dead_code))]

#[cfg(feature = "window")]
mod app;
pub mod audio;
pub mod consent;
pub mod display;
#[cfg(feature = "window")]
mod drive;
pub mod fonts;

/// GPU tests on a machine whose desktop is using the card.
#[cfg(test)]
pub(crate) mod gpu_test {
    /// Runs `f`, a test that checks the software renderer and then the GPU
    /// renderer. If wgpu fails with "Out of Memory" (the desktop holds the
    /// card's memory), the rest of the GPU part is skipped with a line in
    /// the test output instead of failing; the software checks, which come
    /// first, have run by then. Any other panic is the test's failure.
    pub fn or_skip(what: &str, f: impl FnOnce()) {
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            if msg.contains("wgpu error") && msg.contains("Out of Memory") {
                terse_testkit::skip!("gpu", "{what}: GPU part: no graphics memory ({})", msg.lines().next().unwrap_or(""));
            } else {
                std::panic::resume_unwind(e);
            }
        }
    }
}
mod figures;
#[cfg(feature = "window")]
mod gamepad;
pub mod glyphs;
#[cfg(feature = "input")]
pub mod keymap;
pub mod mame;
pub mod page;
#[cfg(feature = "gpu")]
mod halo;
#[cfg(feature = "gpu")]
mod beamlines;
#[cfg(feature = "gpu")]
mod panels;
pub mod phosphor;
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
pub mod hwcheck;
pub mod halt;
pub mod hold;
pub mod rule;
pub mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terse_core::{Image, Machine, Script, System, Terminal};
use terse_net::relay::DEFAULT_TIMEOUT;
use terse_net::{
    HeadlessSystem, builtin,
    Consent, ContentService, Node, Subscriptions, default_config_dir, default_data_dir, default_store_dir, local,
    parse_hex,
};

use crate::display::{Display, RendererChoice};
use crate::glyphs::TermGlyphs;

/// Font size in logical pixels.
pub const FONT_PX: f32 = 18.0;

/// The machine frames whose phosphor light (SPEC §vector.phosphor) a `--screenshot`
/// draws: a trail fades below 2% within 60 frames at persistence 240, and
/// below 10% within 600 at 255.
const PHOSPHOR_FRAMES: u64 = 600;

/// How long `--screenshot` waits for feeds to catch up before frame 0.
const SYNC_DEADLINE: Duration = Duration::from_secs(60);

const USAGE: &str = "usage:
  terse-host [<image|id|link> | --shell <image|id>] [--builtin <image|id>]... [--demo] [--relay ws://...|none]...
             [--no-feed-relays] [--no-prefetch] [--no-program-download] [--consent ask|allow|deny] [--store <dir>]
             [--config <dir>] [--data <dir>] [--no-trails] [--force-phosphor] [--size <W>x<H>]
             [--parallax off|pointer] [--eye <dx>,<dy>] [--panels perspective|flat|auto] [--volume <0-100>] [--mute]
  terse-host [<image|id> | --shell [<image|id>]] --screenshot <out.png> --frames <N> [--size <W>x<H>] [--eye <dx>,<dy>]
             [--events <file>] [--demo] [--consent ask|allow|deny] [--relay ws://...] [--store <dir>]
             [--config <dir>] [--data <dir>] [--no-trails] [--force-phosphor]
             [--builtin <image|id>]...
  terse-host --selftest <image|id> | --selftest --shell [<image|id>] [--frames <N>] [--events <file>] [--csv <file>]
             [--size <W>x<H>] [--consent allow|deny] [--no-trails] [--force-phosphor]
             [--store <dir>] [--config <dir>] [--data <dir>] [--builtin <image|id>]...
  terse-host --bench-render [<image|id> | --shell [<image|id>]] --frames <N> [--warmup <N>] [--size <W>x<H>]
             [--events <file>] [--renderer gpu|software] [--threads <n>] [--sweep-eye] [--consent allow|deny] [...]
  terse-host [<image|id> | --shell [<image|id>]] --drive <script|fifo|-> [--size <W>x<H>] [--renderer gpu|software]
             [--consent ask|allow|deny] [--relay ws://...] [--store <dir>] [--config <dir>] [--data <dir>] [...]
  terse-host --sound-check <image|id> [--frames <N>] [--events <file>] [--store <dir>] [--config <dir>] [--data <dir>]
  terse-host --version | --hardware-check
  Every form takes --perf-log <dir>|off: where the performance record goes (RECORD.md; default: <data>/perf
  for a window, none otherwise; it never leaves this computer), and --perf-rule on|off: whether the rule
  acts (PERFORMANCE.md R15; default on in a window, off for --selftest): the look is shed first; a machine
  that keeps making frames late is saved at 3 late frames within 60 and saved and stopped (halted) at 10
  within 300 (a machine alone: exit status 4); the host's own drawing is warned about and, with nothing
  left to shed, recorded as not keeping up. --version prints the host's name, version
  and build identifier, then its fonts and what it can play sound through. --hardware-check times the
  drawing of the worst legal frames (PERFORMANCE.md R16) and opens and closes the sound output, saying
  what it plays through or why there is none.
  Sound (device 6, SPEC §sound) plays in a window through PipeWire or PulseAudio, else ALSA (Linux;
  CoreAudio on macOS, WASAPI on Windows), opened when a program first makes a sound; if it cannot be
  heard, a notice and one line on stderr say why, once. TERSE_AUDIO=off plays nothing and says nothing;
  TERSE_AUDIO=pulse[:<server>] or alsa[:<pcm>] chooses the output (none: as if none were found; null:
  an output with no device, which takes the samples at a device's pace and plays nothing);
  --selftest plays only when TERSE_AUDIO is set. The volume (on the way out; the machines never know)
  starts full, or as --volume and --mute say; Ctrl+Alt+minus and Ctrl+Alt+plus turn it down and up a
  tenth, Ctrl+Alt+M mutes (and the keyboard's volume keys, if the desktop lets them through): keys the
  host keeps, which no program receives. The volume lasts until the host exits: it is not saved.
  --sound-check runs the image headless at 60 frames a second, no picture, its sound handed to the null
  output (never heard, whatever TERSE_AUDIO says), and prints each frame's hand-over to the output
  (PERFORMANCE.md R19): p50, p99 and max (default 600 frames).
  3D vector display (device 9, SPEC §vector3d): in a window the eye follows the pointer (parallax): the
  pointer at the picture's centre is the reference eye, at an edge the eye is 160 units that way;
  over a machine whose image lists device 9 (alone, or its tile or panel) it rests instead, so the
  pointer is over what that program drew (spec/vector3d.md §vector3d.space); --parallax off keeps it
  still. --eye <dx>,<dy> (screen units, each -160 to 160, default 0,0)
  puts the eye there for --screenshot and --bench-render (and keeps it there with --parallax off).
  --panels: how the software renderer draws the machines the shell shows as panels in its 3D
  scene (SPEC §vector3d.list Screen): in perspective (perspective, auto: the default) or flat, upright in
  the rectangle the panel covers; the GPU renderer always draws them in perspective.
  Every form also takes --renderer gpu|software|auto (default auto: the GPU if a hardware graphics
  adapter is found, else the software renderer) and --threads <n> (the software renderer's drawing
  threads; default one per core), --look mame|classic (default mame: MAME's vector CRT renderer,
  a sharp beam, MAME's bloom and tone map, its 2 ms persistence; classic: the earlier look with its
  halo and afterglow), --font-strokes arcade|hershey|drafting|off (the terminal's characters drawn by
  the beam in that stroke font, default arcade; off: as light), both also read from <config>/look,
  --tube xy|p31|amber|vectrex and --vignette: how the phosphor
  display style (SPEC §vector.phosphor) is drawn: a colour XY monitor with each line in its colour
  (xy, the default), a green P31 oscilloscope tube, an amber tube or the Vectrex's white tube (the
  monochrome ones show colours as brightness), and with --vignette a glass darker towards the corners.
  The phosphor look sheds its costliest parts when a frame draws much (more than a quarter of a
  screen of ink: no smear; a third: no halo; two fifths: plain lines, as SPEC §vector.phosphor
  allows), and in a window also after 3 frames in a row that took longer than a frame to draw (one
  level more each time; one less after 120 quick frames); --no-shed keeps the whole look always.
  With no image, runs the shell built into terse-host (programs/shell, SPEC §machines): it starts the
  built-in programs (reader, sparks, fetch, hello, sketch, holo, reports, reef, sheet, and each --builtin image) as machines of their
  own and shows them as tiles. F12, the home key, gives the keyboard back to the shell (its
  launcher); F12 never reaches a program. --shell <image> runs another image as the shell; with
  --screenshot and --selftest, --shell alone is the built-in shell. <image> alone runs that image
  as a single machine, as before. A terse: link instead of <image> (README, Links on the command
  line): terse:<id> runs an image alone and gives any other blob (an article, a document) to the
  built-in shell to open; terse:program/<key>/<name> runs that program alone (a built-in, or the
  current version of a feed held here); a terse:feed/ link says how to subscribe.
  --demo: before the window opens, make sure you have a feed key, subscribe to your own feed and,
  if it has no articles yet, publish four sample articles to it; then run the shell.
  Relays (windowed runs): the --relay options if given (several may be; none: no network); else
  the list in <config>/relays (`terse relay add <url>` writes it; an older <config>/relay is read
  if there is no list); else a relay already running on ws://127.0.0.1:7878; else one started
  inside terse-host on that port, with its store in $XDG_DATA_HOME/terse/relay (else
  ~/.local/share/terse/relay; ~/Library/Application Support/terse/data/relay on macOS; %LOCALAPPDATA%\\terse\\relay on Windows). One line on stderr says
  which. The host connects to all of them
  (at most 8) and, for feeds whose profile names relays, to up to two of those too, unless
  --no-feed-relays. --screenshot and --selftest use relays only with --relay.
  --no-prefetch: when an article is downloaded automatically, do not also fetch the first eight
  terse: links in it (NETWORK 4.4). --no-program-download: do not download the current version of
  each program of the subscribed feeds as it arrives (NETWORK 4.4).
  --data: where programs' named records are kept, in records/ (SPEC §content.records; default
  $XDG_DATA_HOME/terse, else ~/.local/share/terse; ~/Library/Application Support/terse/data on macOS; %LOCALAPPDATA%\\terse on Windows). They never leave this machine.
  --consent: how requests by a program to subscribe, unsubscribe, publish, set the profile or set a nickname (SPEC §content.feeds) are
  answered: ask shows the host's prompt at the bottom of the window, naming the program (Y allows,
  N or Esc refuses); default ask in the window, deny for --screenshot and --selftest (where nobody
  can answer; a --screenshot with ask shows the prompt, unanswered).
  --store: the blob store (default $XDG_CACHE_HOME/terse/blobs, else ~/.cache/terse/blobs; ~/Library/Caches/terse/blobs on macOS; %LOCALAPPDATA%\\terse\\blobs
  on Windows); the
  built-in programs are put into it at start. --config holds your feed key and subscriptions
  (default $XDG_CONFIG_HOME/terse, else ~/.config/terse; ~/Library/Application Support/terse on macOS; %APPDATA%\\terse on Windows). --events is scripted input, as for
  `terse run` (`home` is the home key). --size: the picture's size in pixels for --screenshot
  (default: the grid at the font's size), the window's for the other forms; the screen is
  letterboxed in it at 4:3.
  --selftest runs N frames (default 200) in the window with only the scripted input (default:
  the image's name with .events, if that file exists) and prints per-frame timings;
  --csv writes them per frame. --no-trails: the phosphor display style (SPEC §vector.phosphor) without
  persistence. --force-phosphor: show the phosphor presentation (the whole screen as a glowing
  tube) whatever style the program sets; for testing. Text is drawn with the built-in fonts and
  those <config>/fonts names (`terse fonts`; README, Fonts), loaded at start; --version lists them.
  --bench-render runs the image (or the system) headless like --screenshot, with the page's scroll
  animated as in a window, and draws every frame offscreen with the chosen renderer, timing only the
  drawing (taking the machines' screens and drawing the picture; a GPU frame is timed until the GPU
  has finished it): it prints p50, p99 and max per frame over the N frames after --warmup frames
  (default 60), and how many took over 16.7 ms. No window is opened. --sweep-eye moves the eye every
  frame (round an ellipse 160 units to either side and 80 up and down, once every 120 frames), as a
  pointer moving over the window would, so 3D lists are projected again each frame.
  Paste with Ctrl+V or Shift+Insert: the clipboard's text goes to the focused program as typed text.
  --drive runs the window driven by a script and watched (README, Driving the window; STATE.md 6.5):
  keys as a keyboard gives them, the pointer in window pixels, the window's size; pictures as
  presented, dumps of the terminal, page, vectors and state, a watch for pictures that change while
  idle, expectations (exit status 1 if one fails). No window is shown on this desktop: on Linux it
  starts a headless compositor (weston) of its own; on Windows and macOS it refuses, unless
  TERSE_DRIVE_DISPLAY=current, which shows the window here (for a CI machine only). The
  script is a file, a FIFO (read again after each writer) or - (standard input); with no --store,
  --config or --data, scratch directories are used and removed after; no relay unless --relay.";

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
    let all_args = args.clone();
    let mut drive_script: Option<String> = None;
    let mut image_path = None;
    let mut screenshot = None;
    let mut frames = None;
    let mut size = None;
    let (mut selftest, mut csv, mut no_trails, mut force_phosphor) = (false, None, false, false);
    let mut version = false;
    let mut hardware_check = false;
    let mut sound_check = false;
    let (mut volume, mut mute) = (None, false);
    let started = std::time::Instant::now();
    let mut perf_log: Option<Option<PathBuf>> = None;
    let mut perf_rule: Option<bool> = None;
    let (mut relays, mut store, mut config, mut events) = (Vec::new(), None, None, None);
    let (mut feed_relays, mut prefetch, mut program_download, mut data) = (true, true, true, None);
    let (mut consent, mut demo) = (None, false);
    let (mut shell_flag, mut extra_builtins) = (false, Vec::new());
    let (mut renderer, mut threads, mut bench, mut warmup, mut sweep) = (RendererChoice::Auto, 0usize, false, None, false);
    let mut look = phosphor::Look::default();
    let mut look_args: Vec<(String, Option<String>)> = Vec::new();
    let (mut eye, mut parallax) = ((0.0f32, 0.0f32), true);
    let mut panels = soft::panel::PanelMode::Perspective;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => screenshot = Some(it.next().ok_or(USAGE)?),
            "--drive" => drive_script = Some(it.next().ok_or(USAGE)?),
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
            "--tube" | "--look" | "--font-strokes" => {
                let v = it.next().ok_or(USAGE)?;
                look_args.push((a[2..].to_string(), Some(v)));
            }
            "--vignette" => look_args.push(("vignette".into(), None)),
            "--no-shed" => look.no_shed = true,
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
            "--version" => version = true,
            "--hardware-check" => hardware_check = true,
            "--sound-check" => sound_check = true,
            "--perf-log" => {
                let v = it.next().ok_or(USAGE)?;
                perf_log = Some(if v == "off" { None } else { Some(PathBuf::from(v)) });
            }
            "--perf-rule" => {
                perf_rule = Some(match it.next().ok_or(USAGE)?.as_str() {
                    "on" => true,
                    "off" => false,
                    v => return Err(format!("bad --perf-rule {v} (on or off)")),
                });
            }
            "--shell" => shell_flag = true,
            "--builtin" => extra_builtins.push(it.next().ok_or(USAGE)?),
            "--consent" => {
                let v = it.next().ok_or(USAGE)?;
                consent = Some(Consent::parse(&v).ok_or(format!("bad --consent {v} (ask, allow or deny)"))?);
            }
            "--no-trails" => no_trails = true,
            "--volume" => {
                let n = it.next().ok_or(USAGE)?;
                volume = Some(n.parse::<u8>().ok().filter(|&v| v <= 100).ok_or_else(|| format!("bad volume {n} (0 to 100)"))?);
            }
            "--mute" => mute = true,
            "--force-phosphor" => force_phosphor = true,
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
    // The look: <config>/look, then the options given (README, "The phosphor look").
    if let Some(dir) = config.clone().or_else(default_config_dir) {
        for note in look.read_settings(&dir.join("look")) {
            eprintln!("terse-host: {note}");
        }
    }
    for (name, value) in &look_args {
        look.set_option(name, value.as_deref())?;
    }
    if version {
        // The fonts this host would draw with (README "Fonts").
        let config = config.clone().or_else(default_config_dir);
        let (t, p) = (fonts::TerminalFonts::load(config.as_deref()), fonts::PageFonts::load(config.as_deref()));
        print!("{}\nfonts:\n{}", terse_perflog::Identity::this("terse-host").line(), fonts::describe(&t, &p));
        println!("sound: {}", audio::compiled());
        return Ok(ExitCode::SUCCESS);
    }
    if hardware_check {
        // R16's check, printed (no cache): each worst legal frame's slowest
        // drawing time and the verdict. First the sound: the output opened
        // and closed again, no sample played.
        match audio::probe() {
            Ok(name) => println!("sound: plays through {name}"),
            Err(s) => println!("sound: none: {} ({})", s.cause, s.detail),
        }
        let looks = Looks {
            no_trails: false,
            force_phosphor: false,
            term_fonts: fonts::TerminalFonts::builtin(),
            page_fonts: fonts::PageFonts::builtin(),
            window: None,
            renderer,
            threads,
            look,
            eye,
            parallax,
            panels,
            perf: None,
            started,
            check: None,
            rule: false,
        };
        let t = std::time::Instant::now();
        let c = hwcheck::run(&looks)?;
        for (id, slot, ms, v) in c.verdicts() {
            println!("{id} (device {slot}): {ms:.2} ms: {v}");
        }
        println!("the check took {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        return Ok(ExitCode::SUCCESS);
    }
    if (bench && (selftest || screenshot.is_some() || frames.is_none())) || (sweep && !bench) {
        return Err(USAGE.into());
    }
    if sound_check && (bench || selftest || screenshot.is_some() || drive_script.is_some() || demo || (image_path.is_none() && !shell_flag)) {
        return Err(USAGE.into());
    }
    let driving = drive_script.is_some();
    if driving && (bench || selftest || screenshot.is_some() || events.is_some() || frames.is_some() || demo) {
        return Err(USAGE.into());
    }
    #[cfg(feature = "window")]
    if let Some(path) = &drive_script {
        drive::check(path)?;
        // Never a window on this desktop: a compositor of its own.
        if let Some(status) = drive::nest(&all_args, size) {
            return status;
        }
    }
    #[cfg(not(feature = "window"))]
    let _ = &all_args;
    // A drive never uses the user's directories unless told.
    let scratch: Option<PathBuf> = match driving && (store.is_none() || config.is_none() || data.is_none()) {
        #[cfg(feature = "window")]
        true => {
            let d = drive::scratch()?;
            store = store.or(Some(d.join("store")));
            config = config.or(Some(d.join("config")));
            data = data.or(Some(d.join("data")));
            Some(d)
        }
        _ => None,
    };
    let windowed = !selftest && !bench && !sound_check && screenshot.is_none() && !driving;
    // The rule acts in a window; a self-test only records unless told.
    let rule = perf_rule.unwrap_or(windowed || driving);
    // No image: the built-in shell. --shell: the image named (if any) is
    // the shell.
    let mut shell = shell_flag || image_path.is_none();
    if (selftest && image_path.is_none() && !shell_flag)
        || (demo && (selftest || (image_path.is_some() && !shell_flag)))
        || (!shell && !extra_builtins.is_empty())
    {
        return Err(USAGE.into());
    }
    let consent = match consent {
        Some(Consent::Ask) if selftest => return Err("--selftest cannot ask for consent (allow or deny)".into()),
        Some(c) => c,
        None if windowed || driving => Consent::Ask,
        None => Consent::Deny,
    };
    let config = config.or_else(default_config_dir).ok_or("no config directory (use --config)")?;
    audio::configure(volume, mute);
    // Every font is opened here, at start, and never again (SPEC
    // §responsiveness item 5).
    let term_fonts = fonts::TerminalFonts::load(Some(&config));
    let page_fonts = fonts::PageFonts::load(Some(&config));
    let mut notes: Vec<&String> = term_fonts.notes.iter().chain(&page_fonts.notes).collect();
    notes.dedup();
    for n in notes {
        eprintln!("terse-host: fonts: {n}");
    }
    let store = store.or_else(default_store_dir).ok_or("no blob store directory (use --store)")?;
    let data = data.or_else(default_data_dir).ok_or("no data directory (use --data)")?;
    // The performance record (RECORD.md): a window's runs go to the data
    // directory's perf/; a self-test's only where --perf-log says.
    let perf = match perf_log {
        Some(p) => p,
        None if windowed || driving => Some(data.join("perf")),
        None => None,
    };
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
    // A terse: link (README "Links on the command line"): an image runs as
    // an image given by its ID; any other blob is given to the built-in
    // shell to open (an article in the reader, a document in its format's
    // program); a program link runs that program; a feed link says how to
    // follow it.
    let (mut link_bytes, mut shell_argument) = (None, None);
    if let Some(arg) = image_path.clone()
        && let Some(link) = terse_net::link::parse(&arg)
    {
        if shell_flag {
            return Err(format!("{arg}: --shell takes an image, not a link"));
        }
        let n = match &mut node {
            Some(n) => n,
            None => node.insert(open_node()?),
        };
        match link.map_err(|e| format!("{arg}: {e}"))? {
            terse_net::link::Link::Blob(id) => {
                let bytes = n.fetch_bytes(id).map_err(|e| format!("{arg}: could not be fetched: {e}"))?;
                if Image::parse(&bytes).is_ok() {
                    link_bytes = Some(bytes);
                } else {
                    image_path = None;
                    shell = true;
                    shell_argument = Some(id);
                }
            }
            terse_net::link::Link::Program { publisher, name } => {
                let title = |b: &&&[u8]| Image::parse(b).is_ok_and(|i| i.title == name);
                let bytes = if publisher == [0; 32] {
                    builtin::PROGRAMS.iter().find(title).map(|b| b.to_vec())
                } else {
                    n.current_program(&publisher, &name).map(|e| n.fetch_bytes(e.blob)).transpose()?
                };
                link_bytes = Some(bytes.ok_or_else(|| {
                    let name = String::from_utf8_lossy(&name);
                    match publisher == [0; 32] {
                        true => format!("{arg}: no program {name} is built into this host"),
                        false => format!(
                            "{arg}: this host holds no program {name} of that feed: subscribe to it (terse feed subscribe {}), \
                             let it sync, and try again",
                            terse_net::to_hex(&publisher)
                        ),
                    }
                })?);
            }
            terse_net::link::Link::Feed(address) => {
                return Err(format!(
                    "{arg}: a feed: subscribe with `terse feed subscribe {address}`, or open the reader \
                     (terse-host), press O and paste the link: the feed's page lets you subscribe"
                ));
            }
        }
    }
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
    let bytes = match (&image_path, link_bytes) {
        (_, Some(bytes)) => bytes,
        (None, None) => builtin::SHELL.to_vec(),
        (Some(p), None) => image_bytes(p, &mut node)?,
    };
    let image = Image::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    // SPEC §machines.calls: the built-in programs, in the store for the shell.
    let mut builtins = Vec::new();
    if shell {
        let extra = extra_builtins.iter().map(|a| image_bytes(a, &mut node)).collect::<Result<Vec<_>, _>>()?;
        let n = match &node {
            Some(n) => n,
            None => node.insert(open_node()?),
        };
        builtins = builtin::install(n.store(), &extra)?;
    }
    let content = match image.lists(4) || shell {
        false => None,
        true => {
            let mut node = match node {
                Some(node) => node,
                None => open_node()?,
            };
            // SPEC §content.feeds: the feeds are those of the subscription list, which
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
    // The built-in shell has the identity of a built-in (SPEC §machines.calls).
    let identity = image_path.is_none().then(|| terse_core::ProgramId { publisher: [0; 32], name: image.title.clone() });
    let program = Program { image, shell, builtins, identity, data, argument: shell_argument };

    if sound_check {
        let script = match &events {
            Some(p) => Script::parse(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?).map_err(|e| format!("{p}: {e}"))?,
            None => Script::default(),
        };
        return run_sound_check(program, content, script, frames.unwrap_or(600));
    }
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
        let looks = Looks { no_trails, force_phosphor, term_fonts, page_fonts, window: size, renderer, threads, look, eye, parallax, panels, perf: perf.clone(), started, check: None, rule };
        return run_window(program, content, Some(test), None, looks);
    }
    if csv.is_some() {
        return Err(USAGE.into());
    }
    #[cfg(feature = "window")]
    if let Some(path) = &drive_script {
        let looks = Looks { no_trails, force_phosphor, term_fonts, page_fonts, window: size, renderer, threads, look, eye, parallax, panels, perf: perf.clone(), started, check: None, rule };
        let d = drive::Drive::new(path)?;
        let status = run_window(program, content, None, Some(d), looks);
        if let Some(d) = &scratch {
            drive::remove_scratch(d);
        }
        return status;
    }
    let _ = scratch;
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
        let looks = Looks { no_trails, force_phosphor, term_fonts, page_fonts, window: None, renderer, threads, look, eye, parallax, panels, perf: perf.clone(), started, check: None, rule };
        let mode = Headless::Bench { warmup: warmup.unwrap_or(60), frames: frames.unwrap_or(0), sweep };
        run_headless(program, content, script()?, mode, size, looks)?;
        return Ok(ExitCode::SUCCESS);
    }
    match (screenshot, frames) {
        (Some(out), Some(frames)) => {
            let looks = Looks { no_trails, force_phosphor, term_fonts, page_fonts, window: None, renderer, threads, look, eye, parallax, panels, perf: perf.clone(), started, check: None, rule };
            let mode = Headless::Screenshot { frames };
            let (pixels, (width, height), frame) = run_headless(program, content, script()?, mode, size, looks)?.expect("a picture");
            write_png(Path::new(&out), &pixels, (width, height))?;
            eprintln!("terse-host: wrote {out} ({width}x{height}) after {frame} frames");
            Ok(ExitCode::SUCCESS)
        }
        (None, None) if events.is_none() => {
            let mut looks = Looks { no_trails, force_phosphor, term_fonts, page_fonts, window: size, renderer, threads, look, eye, parallax, panels, perf: perf.clone(), started, check: None, rule };
            // R16: the worst legal frames, drawn once per build and computer.
            if let Some((check, cached)) = hwcheck::cached(&looks, hwcheck::default_cache()) {
                if !cached {
                    for (id, slot, ms, v) in check.verdicts().into_iter().filter(|v| v.3 != "ok") {
                        eprintln!("terse-host: hardware check: {id} (device {slot}) takes {ms:.1} ms: {v}");
                    }
                }
                if check.shed_look() && !looks.look.no_shed {
                    looks.look.floor = phosphor::SHED_PLAIN;
                }
                looks.check = Some(check);
            }
            run_window(program, content, None, None, looks)
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
    /// The built-in shell's identity (SPEC §machines.calls), for its named records.
    pub identity: Option<terse_core::ProgramId>,
    /// The data directory: the shell's snapshots (SPEC §machines.snapshots).
    pub data: PathBuf,
    /// The shell's argument (SPEC §system `argument`): a blob a link on the
    /// command line named, which the built-in shell opens.
    pub argument: Option<[u8; 32]>,
}

#[cfg(feature = "window")]
fn run_window(program: Program, content: Option<ContentService>, test: Option<selftest::SelfTest>, drive: Option<drive::Drive>, looks: Looks) -> Result<ExitCode, String> {
    app::run(program, content, test, drive, looks)
}

/// Without the window feature (the bootable image's build, which is to
/// show its frames on a KMS/DRM display through `soft::FrameSink`) there
/// is no window yet.
#[cfg(not(feature = "window"))]
fn run_window(_: Program, _: Option<ContentService>, _: Option<SelfTestStub>, _: Option<()>, _: Looks) -> Result<ExitCode, String> {
    Err("this terse-host was built without a window (feature \"window\"): use --screenshot or --bench-render".into())
}

#[cfg(not(feature = "window"))]
struct SelfTestStub;

/// The user's presentation options.
pub struct Looks {
    pub no_trails: bool,
    pub force_phosphor: bool,
    /// The terminal's fonts and the page's (`<config>/fonts`), loaded at start.
    pub term_fonts: fonts::TerminalFonts,
    pub page_fonts: fonts::PageFonts,
    /// `--size` of a window.
    pub window: Option<(u32, u32)>,
    /// `--renderer`.
    pub renderer: RendererChoice,
    /// `--threads`: the software renderer's drawing threads (0: one per core).
    pub threads: usize,
    /// `--tube` and `--vignette`: how the phosphor style is drawn.
    pub look: phosphor::Look,
    /// `--eye`: where the eye is for 3D lists (SPEC §vector3d.space), screen units
    /// from the reference eye; in a window with parallax, where it rests.
    pub eye: display::Eye,
    /// `--parallax pointer` (the default): in a window the eye follows the
    /// pointer; `off`: it stays at `eye`.
    pub parallax: bool,
    /// `--panels`: how the software renderer draws the shell's panels
    /// (SPEC §machines.screens); the GPU renderer draws them in perspective.
    pub panels: soft::panel::PanelMode,
    /// The performance record's directory (`RECORD.md`; `None`: none), and
    /// when the process started.
    pub perf: Option<PathBuf>,
    pub started: std::time::Instant,
    /// The start-up hardware check (R16), if one was made.
    pub check: Option<hwcheck::Check>,
    /// Whether the rule acts (`--perf-rule`, PERFORMANCE.md R15): sheds,
    /// warns and saves, stops; otherwise it only records.
    pub rule: bool,
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

/// The machines' shares of the system's ink (SPEC §machines.ink).
/// `TERSE_SYSTEM_INK=off` draws every machine within its own caps only,
/// as before the rule (for measuring what the rule bounds; not
/// conforming).
fn system_shares(system: &System) -> terse_core::system_ink::Shares {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var("TERSE_SYSTEM_INK").is_ok_and(|v| v == "off")) {
        return Default::default();
    }
    system.shares()
}

/// The terminal a machine the shell shows draws: [`displayed_terminal`], or
/// a blank one when its picture is a plate (SPEC §machines.ink).
fn shown_terminal<'a>(machine: &'a Machine, share: Option<&terse_core::system_ink::Share>) -> std::borrow::Cow<'a, Terminal> {
    if share.is_some_and(|s| s.plate) { std::borrow::Cow::Owned(Terminal::default()) } else { displayed_terminal(machine) }
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
    let glyphs = TermGlyphs::new(FONT_PX, &looks.term_fonts)?;
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
    display.set_panel_mode(looks.panels);
    Ok(Box::new(display))
}

/// The GPU renderer drawing offscreen; with `hardware_only` (`--renderer
/// auto`), an error if the only adapter is a software one.
#[cfg(feature = "gpu")]
fn gpu_display(glyphs: TermGlyphs, hardware_only: bool) -> Result<Box<dyn Display>, String> {
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

/// `--sound-check` (PERFORMANCE.md R19): the frames run headless at 60 a
/// second, no picture drawn, and each frame's samples go the way a
/// window's do (`audio::Audio`, the host's volume, its queue and output
/// thread) to the null output, a device's buffer with no device behind
/// it: nothing is opened and nothing heard. Prints each frame's hand-over.
fn run_sound_check(program: Program, content: Option<ContentService>, script: Script, frames: u64) -> Result<ExitCode, String> {
    let mut system = System::new(&program.image, program.shell, program.builtins);
    system.set_shell_identity(program.identity);
    system.shell_mut().set_argument(program.argument);
    let mut headless = HeadlessSystem::new(content, script);
    let mut audio = audio::Audio::new(audio::Choice::Null);
    let frame_time = Duration::from_secs_f64(1.0 / 60.0);
    let mut next = Instant::now();
    let mut sounding = 0u64;
    for _ in 0..frames {
        headless.frame(&mut system);
        let mut samples = [0i16; 2 * terse_core::sound::FRAME_SAMPLES];
        let any = system.take_audio(&mut samples);
        if let Some(s) = audio.frame(any.then_some(&samples[..])) {
            return Err(s.line());
        }
        sounding += u64::from(audio.playing().is_some());
        if system.stopped().is_some() {
            break;
        }
        next += frame_time;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let buffer_ms = audio::buffer() as f64 * 1000.0 / audio::RATE as f64;
    let start_ms = audio::start() as f64 * 1000.0 / audio::RATE as f64;
    match audio.close() {
        None => println!("sound-check: no sound in {frames} frames: nothing handed over"),
        Some(r) => println!(
            "sound-check: {} frames handed over to {} ({sounding} frames with the output open): hand-over {}; pairs dropped {}; underruns {}; output buffer {buffer_ms:.1} ms, playing from {start_ms:.1} ms",
            r.handover.frames,
            r.name,
            r.handover.describe(),
            r.dropped,
            r.underruns.map_or("unknown".into(), |u| u.to_string()),
        ),
    }
    Ok(ExitCode::SUCCESS)
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
    // SPEC §page: a screenshot shows the finished page (no scroll animation,
    // figures rasterised at once); a benchmark draws as a window does.
    display.configure(looks.no_trails, looks.force_phosphor, bench);
    display.set_look(looks.look);
    display.set_eye(looks.eye);
    let viewport = size.unwrap_or(display.grid_size());
    eprintln!("terse-host: renderer {}, {}x{}", display.name(), viewport.0, viewport.1);

    let image = &program.image;
    let mut system = System::new(image, program.shell, program.builtins);
    system.set_shell_identity(program.identity);
    system.shell_mut().set_argument(program.argument);
    system.set_eye(looks.eye);
    terse_net::snapshots::load_table(&mut system, &program.data);
    let mut headless = HeadlessSystem::new(content, script);
    // Each machine's page has its own layouter; the fonts are shared.
    type Latest = Arc<Mutex<Option<Arc<page::HostLayout>>>>;
    let latest: Arc<Mutex<HashMap<u32, Latest>>> = Arc::default();
    if image.lists(7) || program.shell {
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

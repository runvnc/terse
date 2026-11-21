//! The software renderer against reference pictures (`tests/reference/`):
//! scripted scenes run headless and drawn by the software renderer at
//! 400 × 300, as `--screenshot --renderer software` does, each compared
//! with its checked-in picture. They need no GPU and no network, so they
//! run on any machine with the DejaVu fonts (the grid in DejaVu Sans Mono,
//! the page in DejaVu Sans, Serif and Sans Mono); without them the tests
//! say so and pass. `UPDATE_REFERENCE=1` rewrites the pictures.
//!
//! The tolerance allows for other fallback fonts (the hello screen's two
//! CJK characters and the like come from whatever the system has) and for
//! float rounding on other CPUs: at most `MAX_OFF` of the pixels may
//! differ by more than 8/255 in a channel, and the mean difference must
//! stay under `MAX_MEAN` (per channel, 0–255).

use std::path::PathBuf;

use terse_core::{Image, Script};
use terse_net::{BlobStore, Consent, ContentService, Draft, Identity, Node, Subscriptions, builtin};

use crate::display::RendererChoice;
use crate::{Headless, Looks, Program, page, run_headless};

const SIZE: (u32, u32) = (400, 300);
const MAX_OFF: f64 = 0.005;
const MAX_MEAN: f64 = 0.5;
/// 2026-11-17 18:00 UTC: the articles' times count up from here.
const T0: u32 = 1_794_938_400;

fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reference")
}

/// The looks the pictures were made with: the built-in fonts.
fn looks() -> Option<Looks> {
    Some(Looks {
        no_trails: false,
        force_phosphor: false,
        term_fonts: crate::fonts::TerminalFonts::builtin(),
        page_fonts: page::PageFonts::builtin(),
        window: None,
        renderer: RendererChoice::Software,
        threads: 2,
        look: crate::phosphor::Look::default(),
        eye: (0.0, 0.0),
        parallax: false,
        panels: crate::soft::panel::PanelMode::Perspective,
    })
}

/// A scene drawn by the software renderer: `image` (or the built-in shell)
/// run for `frames` frames with `events`, with a store holding the four
/// sample articles of `--demo` in a feed it follows.
fn render(image: Option<&[u8]>, events: &str, frames: u64, consent: Consent) -> Option<Vec<u8>> {
    let looks = looks()?;
    let dir = tempfile::tempdir().unwrap();
    let mut node = Node::new(BlobStore::open(dir.path().join("store"), None).unwrap(), None);
    let alice = Identity::from_seed([11; 32]);
    node.set_identity(alice.clone());
    let samples: [(&str, &str); 4] = [
        ("How Tersenet works: a longer read", include_str!("../demo/long.txt")),
        ("Hello in three languages: Γειά σας, مرحبا", include_str!("../demo/languages.txt")),
        ("Pictures in articles", include_str!("../demo/pictures.txt")),
        ("Welcome to Tersenet", include_str!("../demo/welcome.txt")),
    ];
    for (i, (title, text)) in samples.iter().enumerate() {
        let draft = Draft { data: text.as_bytes().to_vec(), title: title.to_string(), reply_to: None, time: T0 + 600 * i as u32 };
        assert!(matches!(node.publish_article(&draft), terse_net::Done::Published { .. }));
    }
    let builtins = builtin::install(node.store(), &[]).unwrap();
    node.set_subscriptions(Subscriptions::in_memory([alice.feed_key()]));
    node.set_identity_path(dir.path().join("identity.key")).unwrap();
    let (bytes, shell) = match image {
        Some(b) => (b.to_vec(), false),
        None => (builtin::SHELL.to_vec(), true),
    };
    let image = Image::parse(&bytes).unwrap();
    let content = (image.lists(4) || shell).then(|| ContentService::start_with_consent(node, consent));
    let program = Program { image, shell, builtins: if shell { builtins } else { Vec::new() }, identity: None, data: dir.path().join("data") };
    let script = Script::parse(events).unwrap();
    let (pixels, size, _) = run_headless(program, content, script, Headless::Screenshot { frames }, Some(SIZE), looks).unwrap()?;
    assert_eq!(size, SIZE);
    Some(pixels)
}

fn read_png(path: &std::path::Path) -> Option<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    (info.color_type == png::ColorType::Rgba && (info.width, info.height) == SIZE).then(|| buf[..info.buffer_size()].to_vec())
}

/// Compares the scene with its picture (or writes it).
fn check(name: &str, image: Option<&[u8]>, events: &str, frames: u64, consent: Consent) {
    let Some(got) = render(image, events, frames, consent) else { return };
    let path = reference_dir().join(format!("{name}.png"));
    let want = read_png(&path);
    if std::env::var_os("UPDATE_REFERENCE").is_some() || want.is_none() {
        std::fs::create_dir_all(reference_dir()).unwrap();
        crate::write_png(&path, &got, SIZE).unwrap();
        eprintln!("reference pictures: wrote {}", path.display());
        return;
    }
    let want = want.unwrap();
    let (mut off, mut sum) = (0usize, 0u64);
    for (a, b) in got.chunks_exact(4).zip(want.chunks_exact(4)) {
        let d = (0..3).map(|i| a[i].abs_diff(b[i])).collect::<Vec<_>>();
        sum += d.iter().map(|&v| v as u64).sum::<u64>();
        if d.iter().any(|&v| v > 8) {
            off += 1;
        }
    }
    let pixels = (SIZE.0 * SIZE.1) as usize;
    let (share, mean) = (off as f64 / pixels as f64, sum as f64 / (pixels * 3) as f64);
    if share > MAX_OFF || mean > MAX_MEAN {
        let failed = std::env::temp_dir().join(format!("terse-reference-{name}.png"));
        crate::write_png(&failed, &got, SIZE).unwrap();
        panic!(
            "{name}: {:.3}% of pixels differ by more than 8/255, mean difference {mean:.3} (allowed {:.1}%, {MAX_MEAN}); drawn: {}",
            share * 100.0,
            MAX_OFF * 100.0,
            failed.display()
        );
    }
}

const HELLO: &[u8] = include_bytes!("../../../programs/hello/hello.terse");
const READER: &[u8] = include_bytes!("../../../programs/reader/reader.terse");
const SPARKS: &[u8] = include_bytes!("../../../programs/sparks/sparks.terse");
const SPARKS_KEYS: &str = include_str!("../../../programs/sparks/sessions/keys.txt");
const SPARKS_PHOSPHOR: &str = include_str!("../../../programs/sparks/sessions/phosphor.txt");

/// The terminal: colours, wide characters, the cursor; the grid scaled
/// down into a small picture.
#[test]
fn hello() {
    check("hello", Some(HELLO), "", 30, Consent::Deny);
}

/// The page: text in two faces, a rule, a figure (a chart, rasterised with
/// tiny-skia), the page's background and clipping.
#[test]
fn reader_figures() {
    check("reader-figures", Some(READER), "10 press down\n12 press enter\n", 40, Consent::Deny);
}

/// The phosphor style: additive beam, halo, bloom, afterglow and smear of
/// moving shapes, glowing text.
#[test]
fn sparks_phosphor() {
    check("sparks-phosphor", Some(SPARKS), SPARKS_PHOSPHOR, 120, Consent::Deny);
}

/// The clean style with the fill off: shapes as outlines over the cells.
#[test]
fn sparks_outlines() {
    let events = format!("10 press f\n{}", SPARKS_KEYS.lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>().join("\n"));
    check("sparks-outlines", Some(SPARKS), &events, 300, Consent::Deny);
}

const HOLO: &[u8] = include_bytes!("../../../programs/holo/holo.terse");

/// The 3D vector display (device 9) in the phosphor style: the projected
/// segments as beam with the depth cue, the vertex dwell and the smear of
/// turning models; labels on the glass.
#[test]
fn holo_phosphor() {
    check("holo-phosphor", Some(HOLO), "2 press p\n", 90, Consent::Deny);
}

/// Faces on the 3D display (device 9): solid blocks hiding one another and
/// the ground, the hidden parts of their edges cut away.
#[test]
fn holo_city() {
    check("holo-city", Some(HOLO), "2 press 5\n", 90, Consent::Deny);
}

/// The cockpit (SPEC §vector3d.list Surface): three consoles, each a Surface showing
/// a region of holo's terminal in perspective, those regions off the glass,
/// lines in front of and behind them; a row of the list chosen by a click
/// on its tilted console.
#[test]
fn holo_cockpit() {
    check("holo-cockpit", Some(HOLO), "2 press 6\n60 pointer 447 382 1\n61 pointer 447 382 0\n", 90, Consent::Deny);
}

/// The shell with four machines tiled (one in the phosphor style), each
/// scaled into its picture with glyphs for its size.
#[test]
fn shell_four_machines() {
    let events = "5 press enter\n30 press down\n32 press down\n34 press down\n36 press enter\n45 home\n46 press up\n46 press up\n47 press up\n\
                  48 press up\n49 press up\n50 press enter\n75 press p\n77 press enter\n85 home\n86 press up\n86 press up\n87 press up\n88 press up\n89 press enter\n\
                  110 home\n111 press up\n111 press up\n112 press up\n113 press up\n114 press up\n115 press up\n116 press up\n117 press enter\n\
                  140 press enter\n150 home\n151 press 1\n152 press esc\n160 press pagedown\n";
    check("shell-four", None, events, 200, Consent::Deny);
}

/// The host's consent prompt over the bottom rows, in its own colours.
#[test]
fn consent_prompt() {
    check("consent", Some(READER), "10 press f\n14 press p\n18 text Carol\n22 press enter\n26 press y\n", 40, Consent::Ask);
}

/// The gallery's script: the shell in its gallery layout (L twice), the
/// reader on an article, sparks in the phosphor style, sketch with a
/// drawing, holo; then the ring turned back to the reader.
const GALLERY: &str = "2 press l\n3 press l\n5 press enter\n40 press enter\n60 home\n61 press up\n62 press up\n63 press up\n64 press up\n\
                       65 press up\n66 press enter\n80 press p\n90 home\n91 press up\n92 press up\n93 press up\n94 press enter\n\
                       120 pointer 200 150 1\n121 pointer 260 120 1\n122 pointer 320 150 1\n123 pointer 380 120 1\n124 pointer 440 150 1\n\
                       125 pointer 440 260 1\n126 pointer 320 360 1\n127 pointer 200 260 1\n128 pointer 200 150 1\n129 pointer 200 150 0\n\
                       140 home\n141 press up\n142 press up\n143 press up\n144 press enter\n175 home\n176 press 1\n";

/// The shell's gallery (SPEC §machines.screens): four machines' screens as panels on a
/// ring, in perspective, the front one on the glass; the ring's lines and
/// the panels hiding what is behind them.
#[test]
fn shell_gallery() {
    check("shell-gallery", None, GALLERY, 210, Consent::Deny);
}

/// The reader's panel part way through flying onto the glass (`face`):
/// the whole scene moved with it.
#[test]
fn shell_gallery_flying() {
    check("shell-gallery-flying", None, &format!("{GALLERY}200 press enter\n"), 209, Consent::Deny);
}

/// A system run headless with the built-in shell and `events`, for tests
/// that look at the system as well as the picture.
fn system_after(events: &str, frames: u32) -> terse_core::System {
    let dir = tempfile::tempdir().unwrap();
    let node = Node::new(BlobStore::open(dir.path().join("store"), None).unwrap(), None);
    let builtins = builtin::install(node.store(), &[]).unwrap();
    let mut node = node;
    node.set_records(terse_net::Records::in_data_dir(dir.path()));
    let content = ContentService::start_with_consent(node, Consent::Deny);
    let shell = Image::parse(builtin::SHELL).unwrap();
    let mut system = terse_core::System::new(&shell, true, builtins);
    let mut headless = terse_net::HeadlessSystem::new(Some(content), Script::parse(events).unwrap());
    for _ in 0..frames {
        headless.frame(&mut system);
    }
    system
}

/// The displays to compare: the software renderer, and the GPU's if this
/// machine has an adapter.
fn displays() -> Vec<Box<dyn crate::display::Display>> {
    let Some(looks) = looks() else { return Vec::new() };
    #[cfg_attr(not(feature = "gpu"), allow(unused_mut))]
    let mut out = vec![crate::headless_display(&looks).unwrap()];
    #[cfg(feature = "gpu")]
    {
        let glyphs = crate::glyphs::TermGlyphs::new(crate::FONT_PX, &looks.term_fonts).unwrap();
        match crate::gpu_display(glyphs, false) {
            Ok(d) => out.push(d),
            Err(e) => eprintln!("no GPU ({e}): the GPU renderer is not compared"),
        }
    }
    out
}

/// What `display` shows of `system` (no page layouts: neither picture has
/// one), as RGBA.
fn shown(display: &mut dyn crate::display::Display, system: &terse_core::System) -> Vec<u8> {
    display.configure(false, false, false);
    display.update(system, SIZE, &[], &|_| None, None);
    display.snapshot(SIZE).unwrap()
}

/// The pixels of `rect` (screen units of the shell, as drawn at SIZE).
fn region(pixels: &[u8], p: terse_core::Region) -> Vec<u8> {
    let r = crate::display::picture_rect(crate::display::system_rect(SIZE), p);
    let (x0, y0, x1, y1) = (r.0 as usize, r.1 as usize, (r.0 + r.2) as usize, (r.1 + r.3) as usize);
    (y0..y1).flat_map(|y| pixels[(y * SIZE.0 as usize + x0) * 4..(y * SIZE.0 as usize + x1) * 4].to_vec()).collect()
}

/// A machine whose panel has flown onto the glass (`face`, SPEC §machines.calls) is
/// drawn exactly as the same machine placed flat in that rectangle, by
/// both renderers; and a panel facing the viewer on the glass, as large as
/// the screen, shows the picture a machine placed over the whole screen
/// shows, within the reference pictures' tolerance. (One test: one GPU
/// device for the process.)
#[test]
fn panels_and_flat_pictures() {
    crate::gpu_test::or_skip("panels_and_flat_pictures", panels_and_flat_pictures_checks);
}

fn panels_and_flat_pictures_checks() {
    let mut displays = displays();
    a_faced_panel_is_the_flat_picture(&mut displays);
    a_panel_on_the_glass_is_the_flat_picture(&mut displays);
    surfaces_of_a_machine(&mut displays);
    surfaces_in_a_tile(&mut displays);
    cockpit_gpu_against_software(&mut displays);
}

/// holo's cockpit as the GPU and the software renderer draw it, clean and
/// phosphor, at 880 × 660: how far they differ (printed; they draw the
/// consoles from textures of different origin, so only a loose bound).
fn cockpit_gpu_against_software(displays: &mut [Box<dyn crate::display::Display>]) {
    if displays.len() < 2 {
        return;
    }
    let image = Image::parse(HOLO).unwrap();
    let viewport = (880, 660);
    for (style, script) in [("clean", "2 press 6\n"), ("phosphor", "2 press 6\n3 press p\n")] {
        let mut system = terse_core::System::new(&image, false, Vec::new());
        let mut headless = terse_net::HeadlessSystem::new(None, Script::parse(script).unwrap());
        for _ in 0..90 {
            headless.frame(&mut system);
        }
        let pictures: Vec<Vec<u8>> = displays
            .iter_mut()
            .map(|d| {
                d.configure(false, false, false);
                d.update(&system, viewport, &[0], &|_| None, None);
                d.snapshot(viewport).unwrap()
            })
            .collect();
        let (share, mean) = difference(&pictures[0], &pictures[1]);
        eprintln!("cockpit, {style}: GPU against software at 880 x 660: {:.3}% of pixels differ by more than 8, mean difference {mean:.3}", share * 100.0);
        assert!(share < 0.05 && mean < 3.0, "{style}: {:.3}%, mean {mean:.3}", share * 100.0);
    }
}

/// A Surface command (SPEC §vector3d.list): the source and the width in units, after
/// a Translate to (x, y, z) units.
fn surface_list(at: (i16, i16, i16), r: [u16; 4], width: u16) -> Vec<u8> {
    let mut v = vec![0x09];
    for c in [at.0, at.1, at.2] {
        v.extend((c * 16).to_le_bytes());
    }
    v.push(0x0E);
    r.iter().for_each(|x| v.extend(x.to_le_bytes()));
    v.extend((width * 16).to_le_bytes());
    v
}

/// How far two pictures differ: the share of pixels off by more than 8 in
/// a channel, and the mean difference.
fn difference(a: &[u8], b: &[u8]) -> (f64, f64) {
    let off = a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8)).count();
    let mean = a.iter().zip(b).map(|(p, q)| p.abs_diff(*q) as f64).sum::<f64>() / a.len() as f64;
    (off as f64 / (a.len() / 4) as f64, mean)
}

/// The pixels of `rect` (x0, y0, x1, y1) of a `width`-wide picture, and
/// those outside it.
fn split(pixels: &[u8], width: usize, rect: (usize, usize, usize, usize)) -> (Vec<u8>, Vec<u8>) {
    let (mut inside, mut outside) = (Vec::new(), Vec::new());
    for (i, p) in pixels.chunks_exact(4).enumerate() {
        let (x, y) = (i % width, i / width);
        let to = if x >= rect.0 && x < rect.2 && y >= rect.1 && y < rect.3 { &mut inside } else { &mut outside };
        to.extend_from_slice(p);
    }
    (inside, outside)
}

/// A single machine's own Surfaces (SPEC §vector3d.presentation), by both renderers: one
/// facing the viewer on the glass, as large as the screen and showing all
/// of it, is the picture without it; one far off the screen still takes
/// its source off the glass (black) and leaves the rest as it was; and an
/// idle machine with Surfaces is drawn no more (the software renderer).
fn surfaces_of_a_machine(displays: &mut [Box<dyn crate::display::Display>]) {
    let image = Image::parse(HOLO).unwrap();
    let mut system = terse_core::System::new(&image, false, Vec::new());
    let mut headless = terse_net::HeadlessSystem::new(None, Script::parse("2 press 1\n3 press a\n").unwrap());
    for _ in 0..20 {
        headless.frame(&mut system);
    }
    for d in displays.iter_mut() {
        // At the font's own size, so the glass is drawn with the glyphs a
        // screen of that size has.
        let viewport = d.grid_size();
        let name = d.name();
        let mut at = |list: Option<&[u8]>| {
            let v3 = system.shell_mut().vector3_mut().unwrap();
            match list {
                Some(l) => assert_eq!(v3.submit(l), 0),
                None => v3.clear(),
            }
            d.configure(false, false, false);
            d.update(&system, viewport, &[], &|_| None, None);
            d.snapshot(viewport).unwrap()
        };
        let plain = at(None);
        let whole = at(Some(&surface_list((320, 240, 0), [0, 0, 640, 480], 640)));
        let (share, mean) = difference(&plain, &whole);
        eprintln!("{}: the whole screen on a Surface against the plain picture: {:.3}% differ by more than 8, mean {mean:.3}", name, share * 100.0);
        assert!(share <= MAX_OFF * 4.0 && mean <= MAX_MEAN * 2.0, "{}: {:.3}% differ, mean {mean:.3}", name, share * 100.0);
        // The top left quarter on a Surface far to the left, off the screen.
        let off = at(Some(&surface_list((-1500, 240, 0), [0, 0, 320, 240], 100)));
        let (w, h) = (viewport.0 as usize, viewport.1 as usize);
        let (inside, outside) = split(&off, w, (0, 0, w / 2, h / 2));
        assert!(inside.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]), "{}: the source is off the glass", name);
        let (was_inside, was_outside) = split(&plain, w, (0, 0, w / 2, h / 2));
        assert!(was_inside.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]), "something was there");
        assert!(outside == was_outside, "{}: the rest of the glass is as it was", name);
    }
    // Idle: nothing drawn again.
    let Some(looks) = looks() else { return };
    let glyphs = crate::glyphs::TermGlyphs::new(crate::FONT_PX, &looks.term_fonts).unwrap();
    let mut soft = crate::soft::SoftScreens::new(Box::new(glyphs), 1);
    use crate::display::Display;
    system.shell_mut().vector3_mut().unwrap().submit(&surface_list((320, 300, 100), [0, 0, 320, 240], 300));
    for _ in 0..2 {
        soft.update(&system, SIZE, &[0], &|_| None, None);
        soft.render(SIZE);
    }
    soft.update(&system, SIZE, &[0], &|_| None, None);
    soft.render(SIZE);
    assert!(soft.damage().is_empty(), "idle: {:?}", soft.damage());
    soft.set_eye((40.0, 0.0));
    soft.update(&system, SIZE, &[0], &|_| None, None);
    soft.render(SIZE);
    assert!(!soft.damage().is_empty(), "the eye moved");
}

/// A machine with Surfaces in a tile of the shell (holo's cockpit beside
/// the reader), by both renderers: its picture is its complete picture,
/// Surfaces included; a Surface showing its whole screen on its glass is
/// the tile without it, and its sources are off its glass.
fn surfaces_in_a_tile(displays: &mut [Box<dyn crate::display::Display>]) {
    let mut system = system_after("2 press enter\n30 home\n31 press up\n32 press enter\n60 press 6\n", 90);
    let (h, holo) = system.children().find(|(_, c)| c.title == "holo").map(|(h, c)| (h, c.serial)).expect("holo started");
    let picture = system.pictures().into_iter().find(|p| p.0 == h).expect("holo in a tile").1;
    let list = |s: &terse_core::System| s.machine_by_serial(holo).unwrap().1.vector3().unwrap().list().to_vec();
    assert_eq!(terse_core::vector3::surfaces(&list(&system), (0.0, 0.0)).1.surfaces, 3, "the cockpit");
    for d in displays.iter_mut() {
        let cockpit = shown(d.as_mut(), &system);
        let tile = region(&cockpit, picture);
        // Something of each console is drawn in the tile: not all black.
        assert!(tile.chunks_exact(4).filter(|p| p[..3] != [0, 0, 0]).count() > tile.len() / 40, "{}", d.name());
        let saved = list(&system);
        let name = d.name();
        let mut set = |l: Option<&[u8]>| {
            let m = system.child_mut(h).unwrap().machine.as_mut().unwrap();
            let v3 = m.vector3_mut().unwrap();
            match l {
                Some(l) => assert_eq!(v3.submit(l), 0),
                None => v3.clear(),
            }
            region(&shown(d.as_mut(), &system), picture)
        };
        let plain = set(None);
        let whole = set(Some(&surface_list((320, 240, 0), [0, 0, 640, 480], 640)));
        let (share, mean) = difference(&plain, &whole);
        eprintln!("{}: in a tile, the whole screen on a Surface against the plain tile: {:.3}% differ by more than 8, mean {mean:.3}", name, share * 100.0);
        assert!(share <= MAX_OFF * 4.0 && mean <= MAX_MEAN * 2.0, "{}: {:.3}% differ, mean {mean:.3}", name, share * 100.0);
        set(Some(&saved));
    }
}

fn a_faced_panel_is_the_flat_picture(displays: &mut [Box<dyn crate::display::Display>]) {
    // hello in the gallery, faced over 20 frames once it has loaded.
    let mut system = system_after("2 press l\n3 press l\n4 press down\n5 press down\n6 press down\n7 press enter\n", 40);
    let dev = system.shell().machines().unwrap();
    let view = *dev.view();
    assert_eq!((view.handle, view.toward), (1, true));
    assert!(!view.moving(system.shell().frame() - 1), "the view has arrived");
    let picture = terse_core::machines::picture(view.rect).unwrap();
    assert_eq!(system.pictures(), [(1, picture)], "shown flat in the view's rectangle");
    for d in displays.iter_mut() {
        let faced = shown(d.as_mut(), &system);
        // The same machine placed flat in that rectangle: its Screen is
        // then ignored.
        system.shell_mut().machines_mut().unwrap().slot_mut(1).rect = view.rect;
        let flat = shown(d.as_mut(), &system);
        assert_eq!(system.pictures(), [(1, picture)]);
        system.shell_mut().machines_mut().unwrap().slot_mut(1).rect = terse_core::Region::default();
        assert!(region(&faced, picture) == region(&flat, picture), "{}: the faced picture differs from the flat one", d.name());
    }
}

fn a_panel_on_the_glass_is_the_flat_picture(displays: &mut [Box<dyn crate::display::Display>]) {
    let mut system = system_after("2 press l\n3 press l\n4 press down\n5 press down\n6 press down\n7 press enter\n10 home\n", 40);
    // The shell's list: only hello's Screen, at the identity, 640 wide.
    let list = [&[0x09u8, 0x00, 0x14, 0x00, 0x0F, 0x00, 0x00][..], &[0x0D, 1, 0x00, 0x28]].concat();
    let full = terse_core::Region { x: 0, y: 0, w: 640, h: 480 };
    for d in displays.iter_mut() {
        system.shell_mut().vector3_mut().unwrap().submit(&list);
        assert_eq!(system.scene((0.0, 0.0)).unwrap().panels.len(), 1);
        let panel = shown(d.as_mut(), &system);
        system.shell_mut().vector3_mut().unwrap().clear();
        system.shell_mut().machines_mut().unwrap().slot_mut(1).rect = full;
        let flat = shown(d.as_mut(), &system);
        system.shell_mut().machines_mut().unwrap().slot_mut(1).rect = terse_core::Region::default();
        let (a, b) = (region(&panel, full), region(&flat, full));
        let off = a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8)).count();
        let mean = a.iter().zip(&b).map(|(p, q)| p.abs_diff(*q) as f64).sum::<f64>() / a.len() as f64;
        let share = off as f64 / (a.len() / 4) as f64;
        eprintln!("{}: a panel on the glass against the flat picture: {:.3}% differ by more than 8, mean {mean:.3}", d.name(), share * 100.0);
        assert!(share <= MAX_OFF * 4.0 && mean <= MAX_MEAN * 2.0, "{}: {:.3}% differ, mean {mean:.3}", d.name(), share * 100.0);
    }
}

/// Without Screens, the shell's 3D lines are drawn as a single machine's,
/// and nothing of the scene is in effect (the other reference pictures
/// are unchanged by panels).
#[test]
fn no_screens_no_scene() {
    let system = system_after("4 press down\n5 press down\n6 press down\n7 press enter\n", 20);
    assert!(system.scene((0.0, 0.0)).is_none());
    assert!(crate::display::scene_layers(&system, (0.0, 0.0)).is_none());
}

/// An idle gallery costs nothing: when no machine's screen changes, the eye
/// stays and the list is the same, the software renderer draws nothing
/// again; moving the eye draws the scene again.
#[test]
fn an_idle_gallery_draws_nothing() {
    let Some(looks) = looks() else { return };
    // hello on the ring, the launcher open; hello counts its frames, so
    // it is paused with P.
    let system = system_after("2 press l\n3 press l\n4 press down\n5 press down\n6 press down\n7 press enter\n30 home\n31 press p\n", 60);
    assert!(system.scene((0.0, 0.0)).is_some());
    let glyphs = crate::glyphs::TermGlyphs::new(crate::FONT_PX, &looks.term_fonts).unwrap();
    let mut soft = crate::soft::SoftScreens::new(Box::new(glyphs), 1);
    use crate::display::Display;
    let ran: Vec<u32> = std::iter::once(0).chain(system.children().map(|(_, c)| c.serial)).collect();
    for _ in 0..2 {
        soft.update(&system, SIZE, &ran, &|_| None, None);
        soft.render(SIZE);
    }
    soft.update(&system, SIZE, &ran, &|_| None, None);
    soft.render(SIZE);
    assert!(soft.damage().is_empty(), "idle: {:?}", soft.damage());
    soft.set_eye((40.0, 0.0));
    soft.update(&system, SIZE, &ran, &|_| None, None);
    soft.render(SIZE);
    assert!(!soft.damage().is_empty(), "the eye moved");
}

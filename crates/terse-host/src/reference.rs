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

/// The fonts the pictures were made with, if this system has them.
fn looks() -> Option<Looks> {
    let page_fonts = page::PageFonts {
        sans: Some("DejaVu Sans".into()),
        serif: Some("DejaVu Serif".into()),
        mono: Some("DejaVu Sans Mono".into()),
    };
    let have = crate::glyphs::SystemGlyphs::new(crate::FONT_PX, Some("DejaVu Sans Mono")).is_ok()
        && page::Typesetter::new(&page_fonts).is_ok();
    if !have {
        eprintln!("reference pictures: the DejaVu fonts are not installed; not compared");
        return None;
    }
    Some(Looks {
        no_trails: false,
        force_phosphor: false,
        font: Some("DejaVu Sans Mono".into()),
        page_fonts,
        window: None,
        renderer: RendererChoice::Software,
        threads: 2,
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
        ("How Terse works: a longer read", include_str!("../demo/long.txt")),
        ("Hello in three languages: こんにちは, مرحبا", include_str!("../demo/languages.txt")),
        ("Pictures in articles", include_str!("../demo/pictures.txt")),
        ("Welcome to Terse", include_str!("../demo/welcome.txt")),
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
    let content = (image.has_device(4) || shell).then(|| ContentService::start_with_consent(node, consent));
    let program = Program { image, shell, builtins: if shell { builtins } else { Vec::new() } };
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

/// The shell with four machines tiled (one in the phosphor style), each
/// scaled into its picture with glyphs for its size.
#[test]
fn shell_four_machines() {
    let events = "5 press enter\n30 press down\n32 press down\n34 press down\n36 press enter\n45 home\n46 press up\n47 press up\n\
                  48 press up\n50 press enter\n75 press p\n77 press enter\n85 home\n86 press up\n87 press up\n89 press enter\n\
                  110 home\n111 press up\n112 press up\n113 press up\n114 press up\n115 press up\n117 press enter\n\
                  140 press enter\n150 home\n151 press 1\n152 press esc\n160 press pagedown\n";
    check("shell-four", None, events, 200, Consent::Deny);
}

/// The host's consent prompt over the bottom rows, in its own colours.
#[test]
fn consent_prompt() {
    check("consent", Some(READER), "10 press f\n14 press p\n18 text Carol\n22 press enter\n26 press y\n", 40, Consent::Ask);
}

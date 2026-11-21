//! terse-bus shows what terse-host shows. The system ink (SPEC
//! §machines.ink): a shell at its worst with 8 machines at theirs
//! (`terse_core::worst`), shown tiled, overlapping and as panels, shares the
//! system ink on the bus exactly as with the devices built in (the same
//! pictures counted, the same plates, the same caps and the same lists cut,
//! frame by frame), and terse-bus draws the same picture as terse-host's
//! software renderer, plates and cut lists included. The phosphor style
//! (SPEC §vector.phosphor, DEVICES.md 7.1): programs that choose it and
//! machines and shells forced into it are drawn as terse-host draws them
//! (beam, afterglow, halo, the tubes and the glass), the screen black where
//! the clean style draws backgrounds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use terse_bus::bus::BusConfig;
use terse_bus::host::{self, Start};
use terse_bus::inproc::ContentOptions;
use terse_core::machine::FrameResult;
use terse_core::system_ink::Shares;
use terse_core::worst::{Layout, combined_image, system_scenarios, system_shell};
use terse_core::{CONTROLLERS, Controller, Driver, Event, FrameInput, Image, Machine, ProgramId, System, Who};
use terse_net::BlobStore;

/// The directory of the driver programs (and of terse-bus).
fn bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_terse-dev-terminal")).parent().unwrap().to_path_buf()
}

const IDLE: [Controller; CONTROLLERS] = [Controller { connected: false, buttons: 0, left: (0, 0), right: (0, 0) }; CONTROLLERS];

/// Runs every machine and hands it the images it asks for (the devices
/// built in, as terse-host runs them).
#[derive(Default)]
struct Host {
    images: HashMap<[u8; 32], Image>,
    wanted: Vec<(u32, [u8; 32])>,
}

impl Driver for Host {
    fn frame(&mut self, _: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        machine.run_frame(&FrameInput { events: &events, controllers, ..Default::default() })
    }
    fn want_image(&mut self, serial: u32, image: [u8; 32]) {
        self.wanted.push((serial, image));
    }
    fn want_program(&mut self, _: u32, _: &ProgramId, _: &[[u8; 32]]) {}
    fn removed(&mut self, _: u32) {}
}

/// The shares after each of `frames` frames, the devices built in.
fn host_shares(shell: &Image, child: &Image, frames: u32) -> Vec<Shares> {
    let mut host = Host::default();
    host.images.insert(child.id, child.clone());
    let mut system = System::new(shell, true, vec![child.id]);
    let mut out = Vec::new();
    for _ in 0..frames {
        for (serial, id) in std::mem::take(&mut host.wanted) {
            system.deliver_image(serial, Ok(host.images[&id].clone()));
        }
        system.frame(&[], IDLE, &mut host);
        out.push(system.shares());
    }
    out
}

/// The same on the bus: the shares the core counts (the 2D lists' ink from
/// the vector driver's `vector.list`).
fn bus_shares(shell: &Image, child: &[u8], frames: u32, dir: &Path) -> Vec<Shares> {
    let (store, config, data) = (dir.join("store"), dir.join("config"), dir.join("data"));
    for d in [&store, &config, &data] {
        std::fs::create_dir_all(d).unwrap();
    }
    let id = BlobStore::open(&store, None).unwrap().put_unsynced(child).unwrap();
    let content = ContentOptions { store: Some(store), config: Some(config), data: Some(data), ..Default::default() };
    let slots = host::needed_slots(shell, true);
    let drivers = host::drivers(&bin_dir(), &slots, &content.args(), &[]).unwrap();
    let start = Start {
        config: BusConfig::new(PathBuf::new()),
        drivers,
        image: shell,
        shell: Some(vec![id]),
        identity: None,
        snapshot: None,
        grants: None,
        without: Vec::new(),
    };
    let mut bs = start.start().unwrap();
    let mut out = Vec::new();
    for _ in 0..frames {
        bs.frame(&[], IDLE).unwrap();
        out.push(terse_bus::compose::shares(&bs.system));
    }
    out
}

/// Frame by frame, the bus counts what the built-in devices count: the
/// pictures, the plates, the caps, the lists' ink and where they are cut.
#[test]
fn the_bus_shares_the_system_ink_as_the_host_does() {
    let tmp = tempfile::tempdir().unwrap();
    let child_bytes = combined_image();
    let child = Image::parse(&child_bytes).unwrap();
    for (name, layout) in [("tiles", Layout::Tiles), ("overlap", Layout::Overlap), ("panels", Layout::Panels), ("scaled", Layout::Scaled(500))] {
        let shell = Image::parse(&system_shell(layout, 8, 0)).unwrap();
        let host = host_shares(&shell, &child, 8);
        let bus = bus_shares(&shell, &child_bytes, 8, &tmp.path().join(name));
        for (f, (h, b)) in host.iter().zip(&bus).enumerate() {
            assert_eq!(h, b, "{name}, frame {}", f + 1);
        }
        let last = host.last().unwrap();
        assert_eq!(last.shares.len(), 8, "{name}: every machine shown");
        match layout {
            // Two pictures of the whole screen fit; the rest are plates.
            Layout::Overlap => assert_eq!(last.shares.iter().filter(|s| s.plate).count(), 6, "{name}"),
            // The later tiles' lists are cut.
            Layout::Tiles => assert!(last.cut() && last.shares.iter().all(|s| !s.plate), "{name}"),
            _ => assert!(last.used <= terse_core::system_ink::SYSTEM_INK, "{name}"),
        }
    }
}

// Pictures.

fn png_rgb(path: &Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let ch = info.color_type.samples();
    let rgb = buf[..info.buffer_size()].chunks(ch).flat_map(|p| [p[0], p[1], p[2]]).collect();
    (info.width, info.height, rgb)
}

/// The reference software renderer's tolerance (terse-host's reference
/// pictures): at most 0.5 % of the pixels off by more than 8, mean under
/// 0.5. Returns the share of pixels that are exactly the same.
fn assert_close(a: &Path, b: &Path) -> f64 {
    let (aw, ah, pa) = png_rgb(a);
    let (bw, bh, pb) = png_rgb(b);
    assert_eq!((aw, ah), (bw, bh));
    let (mut off, mut same, mut sum) = (0usize, 0usize, 0u64);
    for (x, y) in pa.chunks(3).zip(pb.chunks(3)) {
        let d = (0..3).map(|i| (x[i] as i32 - y[i] as i32).unsigned_abs()).max().unwrap();
        off += (d > 8) as usize;
        same += (d == 0) as usize;
        sum += (0..3).map(|i| (x[i] as i32 - y[i] as i32).unsigned_abs() as u64).sum::<u64>();
    }
    let n = (aw * ah) as f64;
    let (off, mean) = (off as f64 / n, sum as f64 / (n * 3.0));
    assert!(off <= 0.005 && mean < 0.5, "{} vs {}: {:.3} % off, mean {mean:.3}", a.display(), b.display(), off * 100.0);
    same as f64 / n
}

/// Whether the fonts the reference pictures assume are here, and the
/// terse-bus program is there (not when only this package's tests ran).
fn can_draw() -> bool {
    let ok = terse_host::page::Typesetter::new(&Default::default()).is_ok();
    if !ok {
        eprintln!("skipped: no fonts for pictures");
    }
    let there = bin_dir().join(format!("terse-bus{}", std::env::consts::EXE_SUFFIX)).exists();
    if !there {
        eprintln!("skipped: no terse-bus program beside the drivers");
    }
    ok && there
}

/// A temporary directory for pictures, kept (and its path printed) if
/// `TERSE_KEEP_PICTURES` is set.
fn tempdir() -> tempfile::TempDir {
    let mut t = tempfile::tempdir().unwrap();
    if std::env::var_os("TERSE_KEEP_PICTURES").is_some() {
        t.disable_cleanup(true);
        eprintln!("pictures kept in {}", t.path().display());
    }
    t
}

/// Fresh store, config and data directories under `dir`, as arguments.
fn dirs(dir: &Path) -> Vec<String> {
    let mut v = Vec::new();
    for k in ["store", "config", "data"] {
        let p = dir.join(k);
        std::fs::create_dir_all(&p).unwrap();
        v.extend([format!("--{k}"), p.display().to_string()]);
    }
    v
}

/// terse-host's (software renderer) and terse-bus's screenshots with the
/// same arguments, each with directories of its own; whether they are
/// close, and how much of them is the same.
pub fn host_and_bus(tmp: &Path, name: &str, args: &[String]) -> f64 {
    let (a, b) = (tmp.join(format!("{name}-host.png")), tmp.join(format!("{name}-bus.png")));
    let mut host = args.to_vec();
    host.extend(["--screenshot".into(), a.display().to_string(), "--renderer".into(), "software".into()]);
    host.extend(dirs(&tmp.join(format!("{name}-host"))));
    let _ = terse_host::run(host).unwrap();
    let mut bus = args.to_vec();
    bus.extend(["--screenshot".into(), b.display().to_string(), "--relay".into(), "none".into()]);
    bus.extend(dirs(&tmp.join(format!("{name}-bus"))));
    let out = Command::new(bin_dir().join(format!("terse-bus{}", std::env::consts::EXE_SUFFIX))).args(&bus).output().unwrap();
    assert!(out.status.success(), "terse-bus {bus:?}: {}", String::from_utf8_lossy(&out.stderr));
    let same = assert_close(&a, &b);
    eprintln!("{name}: {:.2} % of the pixels the same", same * 100.0);
    same
}

/// The system scenarios as terse-host and terse-bus draw them: plates,
/// lists cut by the system ink, panels; in the clean style and (overlapping
/// pictures) in the phosphor style.
#[test]
fn the_bus_draws_the_system_ink_as_the_host_does() {
    if !can_draw() {
        return;
    }
    let tmp = tempdir();
    let all = tmp.path().join("all.terse");
    std::fs::write(&all, combined_image()).unwrap();
    for s in system_scenarios(terse_net::builtin::PROGRAMS.len() as u32) {
        let img = tmp.path().join(format!("{}.terse", s.id));
        std::fs::write(&img, &s.shell).unwrap();
        let args: Vec<String> = [img.to_str().unwrap(), "--shell", "--builtin", all.to_str().unwrap(), "--frames", "8", "--size", "800x600"].map(String::from).to_vec();
        host_and_bus(tmp.path(), s.id, &args);
        if s.id == "sys-overlap" {
            let mut forced = args.clone();
            forced.push("--force-phosphor".into());
            host_and_bus(tmp.path(), "sys-overlap-phosphor", &forced);
        }
    }
}

/// The phosphor style as terse-host draws it: sparks in its phosphor
/// session (shapes moving: afterglow and smear), hello forced into it (its
/// coloured cell backgrounds must not be drawn), holo's 3D lines, a shell
/// with four machines as tiles and as panels, the tubes and the glass.
#[test]
fn the_bus_draws_the_phosphor_style_as_the_host_does() {
    if !can_draw() {
        return;
    }
    let tmp = tempdir();
    let p = |n: &str| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs").join(n).canonicalize().unwrap().display().to_string();
    let four = tmp.path().join("four.events");
    std::fs::write(&four, FOUR).unwrap();
    let four = four.display().to_string();
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("sparks", vec![p("sparks/sparks.terse"), "--events".into(), p("sparks/sessions/phosphor.txt"), "--frames".into(), "300".into()]),
        ("hello", vec![p("hello/hello.terse"), "--frames".into(), "30".into(), "--force-phosphor".into()]),
        ("hello-amber", vec![p("hello/hello.terse"), "--frames".into(), "30".into(), "--force-phosphor".into(), "--tube".into(), "amber".into(), "--vignette".into()]),
        ("holo", vec![p("holo/holo.terse"), "--frames".into(), "90".into(), "--force-phosphor".into(), "--no-trails".into()]),
        ("shell-tiles", vec!["--shell".into(), "--events".into(), four.clone(), "--frames".into(), "120".into(), "--force-phosphor".into()]),
        ("shell-panels", vec!["--shell".into(), "--events".into(), four.clone(), "--frames".into(), "260".into(), "--force-phosphor".into()]),
    ];
    for (name, mut args) in cases {
        args.extend(["--size".into(), "800x600".into()]);
        host_and_bus(tmp.path(), name, &args);
    }
    // The clean style's backgrounds are not drawn: hello's palette row has
    // coloured backgrounds, black in the phosphor style.
    let (w, _, px) = png_rgb(&tmp.path().join("hello-bus.png"));
    let (_, _, clean) = {
        let args: Vec<String> = [p("hello/hello.terse"), "--frames".into(), "30".into(), "--size".into(), "800x600".into()].to_vec();
        host_and_bus(tmp.path(), "hello-clean", &args);
        png_rgb(&tmp.path().join("hello-clean-bus.png"))
    };
    let bright = |v: &[u8], i: usize| v[3 * i..3 * i + 3].iter().map(|&c| c as u32).sum::<u32>();
    let backgrounds = (0..px.len() / 3).filter(|&i| bright(&clean, i) > 150 && bright(&px, i) < 60).count();
    eprintln!("hello: {backgrounds} pixels of background drawn in the clean style are dark in the phosphor style");
    assert!(backgrounds > (w as usize) * 10, "the clean style's backgrounds are dark in the phosphor style ({backgrounds} pixels)");
}

/// From the launcher at start: reader, sparks, fetch and hello, each started
/// from the launcher after the home key; then L: the grid; L at 200: the
/// gallery (panels).
const FOUR: &str = "\
2 press enter
10 home
11 press up
11 press up
12 press up
13 press up
14 press up
15 press enter
20 home
21 press up
21 press up
22 press up
23 press up
24 press up
25 press enter
30 home
31 press up
31 press up
32 press up
33 press up
34 press up
35 press enter
50 home
51 press l
200 press l
";

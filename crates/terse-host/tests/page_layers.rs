//! The page's layers (SPEC §page.layers) as the host draws them: a figure
//! whose base is a dark panel, then a red square in layer 1 and a green
//! one in layer 2 in front of it, placed by `layer` calls (moved, hidden,
//! shown, mirrored and scaled) without a new document. Headless
//! (`--screenshot`) with both renderers, and in the window (`--drive`, in a
//! headless compositor the host starts; skipped without weston), the
//! pixels checked where each square must be, and must not.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use terse_core::image::DeviceReq;
use terse_core::worst::{Builder, Call};

static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn text(o: &Output) -> String {
    format!("status {:?}\n--- stdout\n{}--- stderr\n{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn host(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_terse-host"));
    c.env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("TERSE_AUDIO", "none")
        .env_remove("TERSE_DRIVE_DISPLAY")
        .env_remove("TERSE_DRIVE_NESTED")
        .current_dir(dir);
    c
}

fn pt(out: &mut Vec<u8>, op: u8, x: i16, y: i16) {
    out.push(op);
    out.extend((x * 16).to_le_bytes());
    out.extend((y * 16).to_le_bytes());
}

/// A filled rectangle in units, no stroke.
fn rect(out: &mut Vec<u8>, rgb: [u8; 3], x: i16, y: i16, w: i16, h: i16) {
    out.extend([0x01, rgb[0], rgb[1], rgb[2], 255, 0x02, 0, 0, 0, 0, 0, 0]);
    pt(out, 0x03, x, y);
    pt(out, 0x04, x + w, y);
    pt(out, 0x04, x + w, y + h);
    pt(out, 0x04, x, y + h);
    out.push(0x07);
}

const PANEL: [u8; 3] = [20, 30, 60];
const RED: [u8; 3] = [230, 30, 30];
const GREEN: [u8; 3] = [30, 200, 60];

/// A document of one figure 640 × 480: the panel, layer 1's red square
/// and layer 2's green square, both 40 units at the figure's corner.
fn doc() -> Vec<u8> {
    let mut p = Vec::new();
    rect(&mut p, PANEL, 0, 0, 640, 480);
    p.extend([0x08, 1]);
    rect(&mut p, RED, 0, 0, 40, 40);
    p.extend([0x08, 2]);
    rect(&mut p, GREEN, 0, 0, 40, 40);
    let mut d = vec![0x08];
    d.extend(640u16.to_le_bytes());
    d.extend(480u16.to_le_bytes());
    d.push(0);
    d.extend((p.len() as u16).to_le_bytes());
    d.extend(p);
    d.push(0);
    d
}

fn layer(id: u32, flags: u32, x: i32, y: i32, scale: u32) -> Call {
    Call::six(0x706, [id, flags, (x * 16) as u32, (y * 16) as u32, scale, 0])
}

/// Every frame: the rectangle, the document, and the layers: red moved to
/// (100, 50); green shown over it at (120, 70) (in front: it was painted
/// later); layer 1's second use, none; then red again mirrored at half
/// size about x 400 (it covers 380 … 400, 200 … 220) — but a layer is one
/// state, so the page draws the red square once, where the last call put
/// it: the mirrored half-size one. Green is moved to (390, 210), over a
/// corner of it: in front, as it comes later in the figure.
fn image(dir: &Path) -> PathBuf {
    let mut b = Builder::new();
    let d = doc();
    let at = b.blob(&d);
    b.frame(vec![
        Call::new(0x702, [0, 0, 640, 480]),
        Call::new(0x700, [at, d.len() as u32, 0, 0]),
        layer(1, 1, 100, 50, 256),
        layer(2, 1, 120, 70, 256),
        layer(1, 3, 400, 200, 128),
        layer(2, 1, 390, 210, 256),
        // Rejected, changes nothing.
        layer(64, 0, 0, 0, 256),
    ]);
    let image = b.build("layers", &[DeviceReq::new(2, 0), DeviceReq::new(7, 0)]);
    let path = dir.join("layers.terse");
    std::fs::write(&path, image).unwrap();
    path
}

fn read_png(path: &Path) -> (Vec<u8>, (u32, u32)) {
    let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = d.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let channels = info.color_type.samples();
    let rgb: Vec<u8> = buf.chunks_exact(channels).flat_map(|p| [p[0], p[1], p[2]]).collect();
    (rgb, (info.width, info.height))
}

/// The colour at screen unit (x, y): the screen is where the panel is
/// (the window may show it smaller, with a border).
fn at((rgb, (w, _)): &(Vec<u8>, (u32, u32)), x: u32, y: u32) -> [u8; 3] {
    let w = *w as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, p) in rgb.chunks_exact(3).enumerate() {
        if near([p[0], p[1], p[2]], PANEL) {
            let (px, py) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(px), y0.min(py), x1.max(px + 1), y1.max(py + 1));
        }
    }
    let px = x0 + (x as usize * (x1 - x0)) / 640;
    let py = y0 + (y as usize * (y1 - y0)) / 480;
    let i = (py * w + px) * 3;
    [rgb[i], rgb[i + 1], rgb[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

/// What the picture must show: the base where the squares were (at the
/// corner), the red square mirrored and halved (380 … 400 by 200 … 220),
/// the green one at 390, 210, in front of it.
fn check(shot: &(Vec<u8>, (u32, u32)), what: &str) {
    for (x, y, want, why) in [
        (10, 10, PANEL, "the corner: both squares moved away"),
        (110, 60, PANEL, "red was moved on by a later call"),
        (385, 205, RED, "red, mirrored and half size, at 380-400 x 200-220"),
        (375, 205, PANEL, "left of the mirrored red"),
        (405, 205, PANEL, "right of it"),
        (385, 225, PANEL, "below it: half size"),
        (395, 215, GREEN, "green over red: later in the figure"),
        (425, 245, GREEN, "green at 390, 210"),
        (435, 245, PANEL, "green is 40 wide"),
    ] {
        let got = at(shot, x, y);
        assert!(near(got, want), "{what}: at ({x}, {y}) {got:?}, want {want:?}: {why}");
    }
}

#[test]
fn layers_are_drawn_where_the_calls_put_them_headless() {
    let dir = tempfile::tempdir().unwrap();
    let img = image(dir.path());
    for renderer in ["software", "gpu"] {
        for size in ["640x480", "1280x960"] {
            let out = dir.path().join(format!("{renderer}-{size}.png"));
            let o = host(dir.path())
                .arg(&img)
                .args(["--relay", "none", "--renderer", renderer, "--size", size, "--frames", "10", "--screenshot"])
                .arg(&out)
                .output()
                .unwrap();
            if renderer == "gpu" && !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter") {
                terse_testkit::skip!("gpu", "no graphics adapter");
                continue;
            }
            assert!(o.status.success(), "{renderer} {size}: {}", text(&o));
            keep(&out);
            check(&read_png(&out), &format!("{renderer} {size}"));
        }
    }
}

/// With TERSE_TEST_KEEP set to a directory, a copy of the picture there.
fn keep(shot: &Path) {
    if let Ok(d) = std::env::var("TERSE_TEST_KEEP") {
        let _ = std::fs::copy(shot, Path::new(&d).join(shot.file_name().unwrap()));
    }
}

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

/// The same in the window, with both renderers: the picture settles and
/// stays still (the layers are set again every frame to the same state).
#[test]
fn layers_in_the_window() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = image(dir.path());
    for renderer in ["software", "gpu"] {
        let script = dir.path().join(format!("script-{renderer}.txt"));
        std::fs::write(&script, format!("settle max 300\nexpect idle 30\nshot {renderer}.png\nquit\n")).unwrap();
        let mut c = host(dir.path());
        c.arg(&img).arg("--drive").arg(&script).args(["--renderer", renderer, "--size", "1280x960", "--consent", "deny"]);
        if renderer == "gpu" {
            c.args(["--perf-rule", "off", "--no-shed"]);
        }
        let o = c.output().unwrap();
        if renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter") {
            terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
            continue;
        }
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(o.status.success() && out.contains("drive: done, no expectation failed"), "{renderer}: {}", text(&o));
        keep(&dir.path().join(format!("{renderer}.png")));
        check(&read_png(&dir.path().join(format!("{renderer}.png"))), &format!("{renderer} window"));
    }
}

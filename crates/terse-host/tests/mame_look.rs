//! The phosphor style's default look, MAME's vector CRT (`terse_host::mame`,
//! `soft::mame`): the GPU and software renderers draw the same picture; it
//! is still when nothing moves (the short decay settles); the built-in
//! programs keep up in it; and a window resized quickly, slowly, growing,
//! shrinking or back to a size it had shows the picture for its final size
//! (`--drive`, in a headless compositor; skipped where there is none).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The tests that run a window run one at a time: each runs in real time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn program(name: &str) -> PathBuf {
    root().join(format!("programs/{name}/{name}.terse"))
}

fn text(o: &Output) -> String {
    format!("status {:?}\n--- stdout\n{}--- stderr\n{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// terse-host with scratch directories and no relay.
fn host(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_terse-host"));
    c.env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("TERSE_AUDIO", "none")
        .env_remove("TERSE_DRIVE_DISPLAY")
        .env_remove("TERSE_DRIVE_NESTED")
        .env_remove("TERSE_MAME_PERSISTENCE")
        .current_dir(dir);
    c
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

/// How two pictures differ: the largest difference of a channel, the share
/// of pixels with a channel more than 2 levels off, and the PSNR (dB).
fn differ(a: &[u8], b: &[u8]) -> (u8, f64, f64) {
    assert_eq!(a.len(), b.len());
    let (mut max, mut off, mut sq) = (0u8, 0usize, 0f64);
    for (p, q) in a.chunks_exact(3).zip(b.chunks_exact(3)) {
        let d = [p[0].abs_diff(q[0]), p[1].abs_diff(q[1]), p[2].abs_diff(q[2])];
        let m = d[0].max(d[1]).max(d[2]);
        max = max.max(m);
        off += (m > 2) as usize;
        sq += d.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
    }
    let mse = sq / a.len() as f64;
    (max, off as f64 / (a.len() / 3) as f64, if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64.powi(2) / mse).log10() })
}

/// The software renderer and the GPU renderer (MAME's shaders) draw the
/// same picture of the same frame, to within 16 levels on any channel, at
/// most 0.05% of the pixels more than 2 levels off, 55 dB: the arithmetic
/// is the same, the GPU keeps the light in 16-bit floats.
#[test]
fn the_gpu_and_the_software_renderer_agree() {
    let dir = tempfile::tempdir().unwrap();
    let scenes: [(&str, PathBuf, Vec<String>); 4] = [
        ("sparks", program("sparks"), vec!["--events".into(), root().join("programs/sparks/sessions/phosphor.txt").display().to_string(), "--frames".into(), "300".into()]),
        ("holo", program("holo"), vec!["--frames".into(), "120".into(), "--force-phosphor".into()]),
        ("pattern", root().join("crates/terse-mame-crt/pattern/pattern.terse"), vec!["--frames".into(), "200".into()]),
        ("text", root().join("crates/terse-mame-crt/pattern/text.terse"), vec!["--frames".into(), "30".into()]),
    ];
    for (name, image, args) in scenes {
        for size in ["640x480", "1280x960"] {
            let mut shots = Vec::new();
            for renderer in ["software", "gpu"] {
                let out = dir.path().join(format!("{name}-{size}-{renderer}.png"));
                let o = host(dir.path())
                    .arg(&image)
                    .args(&args)
                    .args(["--relay", "none", "--renderer", renderer, "--size", size, "--screenshot"])
                    .arg(&out)
                    .output()
                    .unwrap();
                if renderer == "gpu" && !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter") {
                    terse_testkit::skip!("gpu", "no graphics adapter");
                    return;
                }
                assert!(o.status.success(), "{name} {size} {renderer}: {}", text(&o));
                shots.push(read_png(&out));
            }
            let (max, off, psnr) = differ(&shots[0].0, &shots[1].0);
            eprintln!("{name} {size}: max {max}, {:.4}% more than 2 off, {psnr:.1} dB", off * 100.0);
            assert!(max <= 16 && off <= 0.0005 && psnr >= 55.0, "{name} {size}: max {max}, {:.4}% more than 2 off, {psnr:.1} dB", off * 100.0);
        }
    }
}

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

/// `terse-host <args> --drive <script>`.
fn drive(dir: &Path, args: &[&str], script: &str, renderer: &str, size: &str) -> Output {
    let s = dir.join("script.txt");
    std::fs::write(&s, script).unwrap();
    host(dir).args(args).arg("--drive").arg(&s).args(["--renderer", renderer, "--size", size, "--consent", "deny"]).output().unwrap()
}

fn no_gpu(renderer: &str, o: &Output) -> bool {
    let none = renderer == "gpu" && String::from_utf8_lossy(&o.stderr).contains("no graphics adapter");
    if none {
        terse_testkit::skip!("gpu", "no graphics adapter in the compositor");
    }
    none
}

/// The Unix time a script holds (`time`, STATE.md 6.5): the shell's clock
/// shows it, whatever the time, so a picture of the shell taken now and one
/// taken in a minute are the same.
const HELD: &str = "time 1800000000\n";

/// In the phosphor style, with the MAME look, a screen whose program shows
/// nothing new is still once the phosphor has settled: fetch, the reader
/// and the shell, with both renderers. The time is held: the shell's clock
/// (the bar's right end) turns a minute while the test looks, which is not
/// a picture that fails to settle.
#[test]
fn the_phosphor_style_is_still_when_idle() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let fetch = program("fetch").display().to_string();
    let reader = program("reader").display().to_string();
    // (hello is not still: its frame counter changes every frame, and with
    // it the length of the text the beam draws, so the scan's timing of
    // every stroke, by a level or so: MAME's model.)
    let cases: [(&str, Vec<&str>, &str); 3] =
        [("fetch", vec![&fetch, "--force-phosphor"], ""), ("reader", vec![&reader, "--force-phosphor"], ""), ("shell", vec!["--shell", "--force-phosphor"], "")];
    for renderer in ["software", "gpu"] {
        for (name, args, tolerance) in &cases {
            let script = format!("{HELD}settle max 300\nexpect idle 120{tolerance}\nquit\n");
            // In the headless compositor the GPU is a software one
            // (llvmpipe), slow enough to shed the look, which changes the
            // picture or warns: what is tested here is that the phosphor
            // settles.
            let mut args = args.clone();
            if renderer == "gpu" {
                args.extend(["--no-shed", "--perf-rule", "off"]);
            }
            let o = drive(dir.path(), &args, &script, renderer, "640x480");
            if no_gpu(renderer, &o) {
                break;
            }
            assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).contains("ok: expect idle 120"), "{name} {renderer}: {}", text(&o));
        }
    }
}

/// A held time is the time the shell shows: two runs holding the same time
/// present the same picture, and one holding a time an hour later another
/// (the clock in the bar), so the held time is what the stillness and
/// resize tests compare with.
#[test]
fn the_shells_clock_shows_the_held_time() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let shot = |name: &str, time: u32| {
        let png = dir.path().join(format!("{name}.png"));
        let script = format!("time {time}\nsettle max 300\nshot {}\nquit\n", png.display());
        let o = drive(dir.path(), &["--shell", "--no-shed", "--perf-rule", "off"], &script, "software", "640x480");
        assert!(o.status.success(), "{name}: {}", text(&o));
        read_png(&png)
    };
    let (a, b, later) = (shot("a", 1_800_000_000), shot("b", 1_800_000_000), shot("later", 1_800_003_600));
    assert_eq!(a, b, "the same held time, the same picture");
    assert_ne!(a.0, later.0, "an hour later, another clock");
}

/// Every built-in program, in the phosphor style with the MAME look, keeps
/// up for 600 frames in a window of 640 × 480 and of 1900 × 1000, software
/// renderer: the rule (PERFORMANCE.md R15) neither warns nor fails (a
/// frame late now and then is shed, as the rule says).
#[test]
fn the_built_in_programs_keep_up_in_the_mame_look() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for name in ["reader", "sparks", "fetch", "hello", "sketch", "holo", "reports", "reef"] {
        let image = program(name).display().to_string();
        for size in ["640x480", "1900x1000"] {
            let o = drive(dir.path(), &[&image, "--force-phosphor"], "wait 600\nquit\n", "software", size);
            let out = String::from_utf8_lossy(&o.stdout);
            assert!(o.status.success() && !out.contains("slow: warned") && !out.contains("slow: failed"), "{name} {size}: {}", text(&o));
        }
    }
}

/// A window resized quickly (several sizes a frame, growing), back and
/// forth to a size it had, and slowly (shrinking): once it keeps still, the
/// picture presented is the one a window started at that size presents
/// (the machines held meanwhile, the last picture shown scaled; the idle
/// shell drawn again at the true size). With both renderers.
#[test]
fn a_resized_window_shows_the_picture_of_its_final_size() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let rapid = "resize 800 600\nresize 900 700\nwait 1\nresize 1000 750\nresize 1100 800\nwait 2\nresize 1300 900\nresize 1900 1000\n";
    let aba = "resize 1200 900\nwait 2\nresize 700 500\nwait 2\nresize 1200 900\nsleep 30\nresize 1900 1000\nsleep 30\nresize 700 500\n";
    let slow = "resize 1700 950\nwait 20\nresize 1400 900\nwait 20\nresize 1100 800\nwait 20\nresize 900 650\n";
    for renderer in ["software", "gpu"] {
        for (name, start, steps, end) in [("rapid", "640x480", rapid, "1900x1000"), ("back", "640x480", aba, "700x500"), ("slow", "1900x1000", slow, "900x650")] {
            let resized = dir.path().join(format!("{name}-{renderer}.png"));
            let fresh = dir.path().join(format!("{name}-{renderer}-fresh.png"));
            let script = format!("{HELD}settle max 200\n{steps}settle max 300\nshot {}\nquit\n", resized.display());
            // --no-shed: the look shed (a slow adapter, a debug build) would
            // differ from a fresh window's, which is not what is tested.
            let o = drive(dir.path(), &["--shell", "--force-phosphor", "--no-shed", "--perf-rule", "off"], &script, renderer, start);
            if no_gpu(renderer, &o) {
                break;
            }
            assert!(o.status.success(), "{name} {renderer}: {}", text(&o));
            let o = drive(dir.path(), &["--shell", "--force-phosphor", "--no-shed", "--perf-rule", "off"], &format!("{HELD}settle max 300\nshot {}\nquit\n", fresh.display()), renderer, end);
            assert!(o.status.success(), "{name} {renderer}: {}", text(&o));
            let (a, sa) = read_png(&resized);
            let (b, sb) = read_png(&fresh);
            let want: Vec<u32> = end.split('x').map(|v| v.parse().unwrap()).collect();
            assert_eq!((sa.0, sa.1), (want[0], want[1]), "{name} {renderer}: the picture's size");
            assert_eq!(sa, sb);
            // The same, pixel for pixel: the time is held (the bar's clock),
            // and nothing else in the picture depends on how it got there.
            let (max, off, _) = differ(&a, &b);
            assert!(max == 0, "{name} {renderer}: {:.3}% of the pixels differ from a fresh window's, by up to {max}", off * 100.0);
        }
    }
}

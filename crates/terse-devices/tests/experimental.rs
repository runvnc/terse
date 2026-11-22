//! Experimental devices on the bus (`spec/experimental.md`, `DEVICES.md`
//! 15), with the example driver `native/driver-experimental/dice.c`: the
//! core charges every call by the driver's table (measured here to the
//! unit: a machine that calls in a loop for a whole frame has counted its
//! calls in a register), the same image is not granted where there is no
//! driver, and a system grants none. `native/driver-experimental/test.sh`
//! checks the rest with a program built by `terse build`. Every directory
//! is a temporary one; there is no network.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// terse or terse-bus (other packages' programs), built from the current
/// sources, beside this package's drivers.
fn bin(name: &str) -> PathBuf {
    terse_testkit::program(name)
}

struct Home {
    tmp: tempfile::TempDir,
}

impl Home {
    fn path(&self, p: &str) -> String {
        self.tmp.path().join(p).display().to_string()
    }

    fn dirs(&self) -> Vec<String> {
        ["--config", "config", "--store", "store", "--data", "data"].iter().map(|s| if s.starts_with("--") { s.to_string() } else { self.path(s) }).collect()
    }

    fn bus(&self, args: &[&str]) -> Output {
        Command::new(bin("terse-bus")).args(args).args(self.dirs()).args(["--relay", "none"]).env_remove("TERSE_DICE_BAD").output().unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(bin("terse")).arg("run").args(args).args(self.dirs()).output().unwrap()
    }

    /// The image of `rom` (raw RISC-V) listing example.dice as optional on slot 240.
    fn image(&self, name: &str, rom: &[u8]) -> String {
        let raw = self.path(&format!("{name}.bin"));
        std::fs::write(&raw, rom).unwrap();
        let out = self.path(&format!("{name}.terse"));
        let o = Command::new(bin("terse")).args(["pack", "--rom", &raw, "--title", name, "--ram-pages", "1", "--device", "2:0", "--optional", "240:0=example.dice", "-o", &out]).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        out
    }
}

/// terse, terse-bus and the dice driver built, or a skip saying what is missing.
fn ready() -> Option<(Home, String)> {
    terse_testkit::programs(&["terse", "terse-bus"]);
    let home = Home { tmp: tempfile::tempdir().unwrap() };
    let dice = home.path("dice");
    let built = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-std=c11", "-O2", "-o", &dice])
        .arg(workspace().join("native/driver-experimental/dice.c"))
        .status()
        .is_ok_and(|s| s.success());
    if !built {
        terse_testkit::skip!("cc", "no C compiler for native/driver-experimental");
        return None;
    }
    Some((home, dice))
}

fn i_type(imm: i32, rs1: u32, rd: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | rs1 << 15 | rd << 7 | 0x13
}

fn lui(rd: u32, imm20: u32) -> u32 {
    imm20 << 12 | rd << 7 | 0x37
}

fn jal_back(bytes: i32) -> u32 {
    let o = (-bytes) as u32;
    ((o >> 20 & 1) << 31) | ((o >> 1 & 0x3FF) << 21) | ((o >> 11 & 1) << 20) | ((o >> 12 & 0xFF) << 12) | 0x6F
}

/// A program that sets `a7` = `call`, then loops for ever: `t0 += 1`, the
/// argument instructions `args`, `ecall`. Returns the ROM and the units of
/// one round but the call's charge.
fn looping(call: u32, args: &[u32]) -> (Vec<u8>, u32) {
    let mut w = vec![lui(17, call >> 12), i_type((call & 0xFFF) as i32, 17, 17), i_type(1, 5, 5)];
    w.extend_from_slice(args);
    w.push(0x73);
    w.push(jal_back(4 * (args.len() as i32 + 2)));
    (w.iter().flat_map(|x| x.to_le_bytes()).collect(), args.len() as u32 + 3)
}

/// How many rounds a machine makes in its first frame (125,000 units,
/// SPEC §frames) when a round costs `round` units and its call `charge`
/// on top (an instruction that starts with budget left runs).
fn rounds(args: u32, charge: u32) -> u32 {
    let mut budget: i64 = 125_000 - 2;
    let mut count = 0;
    'frame: loop {
        // t0 += 1, the arguments, the ecall (1 + its charge), the jump.
        let costs = std::iter::once(1).chain(std::iter::repeat_n(1, args as usize)).chain([1 + charge, 1]);
        for (i, c) in costs.enumerate() {
            if budget <= 0 {
                break 'frame;
            }
            if i == 0 {
                count += 1;
            }
            budget -= c as i64;
        }
    }
    count
}


/// `t0` (x5) after the first frame, from the machine's saved state; `extra`
/// before the image (`--shell`: the image runs as the shell).
fn t0_after_one_frame(h: &Home, extra: &[&str], image: &str, driver: Option<&str>) -> u32 {
    let snap = h.path("one.snap");
    let mut args = extra.to_vec();
    args.extend([image, "--frames", "1", "--save-state", &snap]);
    let d;
    if let Some(x) = driver {
        d = format!("example.dice={x}");
        args.extend(["--driver", &d]);
    }
    let o = h.bus(&args);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let file = std::fs::read(&snap).unwrap();
    // STATE.md 4: 94 bytes of file head, then the serialization: x1 at 49.
    let at = 94 + 49 + 4 * 4;
    u32::from_le_bytes(file[at..at + 4].try_into().unwrap())
}

/// DEVICES.md 15.3: every call is charged by the table: roll (answered,
/// 64 + its declared 16), seed (deferred, 4), history of 16 bytes
/// (answered, 64 + 1 per 4 bytes); and 4 for each when not granted.
#[test]
fn calls_are_charged_by_the_table() {
    let Some((h, dice)) = ready() else { return };
    let cases: [(&str, u32, Vec<u32>, u32); 3] = [
        ("roll", 0xF001, vec![i_type(6, 0, 10)], 64 + 16),
        ("seed", 0xF000, vec![i_type(7, 0, 10)], 4),
        ("history", 0xF002, vec![lui(10, 0x100), i_type(16, 0, 11)], 64 + 4),
    ];
    for (name, call, args, charge) in cases {
        let (rom, _) = looping(call, &args);
        let image = h.image(name, &rom);
        let n = args.len() as u32;
        assert_eq!(t0_after_one_frame(&h, &[], &image, Some(&dice)), rounds(n, charge), "{name}, granted");
        assert_eq!(t0_after_one_frame(&h, &[], &image, None), rounds(n, 4), "{name}, not granted");
    }
}

/// The same image is not granted where there is no driver: terse run and
/// terse-bus without one agree.
#[test]
fn not_granted_without_a_driver() {
    let Some((h, dice)) = ready() else { return };
    let (rom, _) = looping(0xF001, &[i_type(6, 0, 10)]);
    let image = h.image("roll", &rom);
    let a = h.run(&[&image, "--frames", "3", "--hash"]);
    let b = h.bus(&[&image, "--frames", "3", "--hash"]);
    assert!(a.status.success() && b.status.success());
    assert_eq!(a.stdout, b.stdout);
    let d = format!("example.dice={dice}");
    let c = h.bus(&[&image, "--frames", "3", "--hash", "--driver", &d]);
    assert!(c.status.success());
    assert_ne!(a.stdout, c.stdout, "granted, the machine differs");
}

/// spec/experimental.md §experimental.grants: in a system the shell itself
/// is granted the device by the same rule as a single machine, on the
/// slot its image gives it (241 here), and charged by the table; without
/// the driver it is not.
#[test]
fn the_shell_is_granted_too() {
    let Some((h, dice)) = ready() else { return };
    let (rom, _) = looping(0xF101, &[i_type(6, 0, 10)]);
    let raw = h.path("shell241.bin");
    std::fs::write(&raw, &rom).unwrap();
    let image = h.path("shell241.terse");
    let o = Command::new(bin("terse")).args(["pack", "--rom", &raw, "--title", "s", "--ram-pages", "1", "--optional", "241:0=example.dice", "-o", &image]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(t0_after_one_frame(&h, &["--shell"], &image, Some(&dice)), rounds(1, 64 + 16), "granted");
    assert_eq!(t0_after_one_frame(&h, &["--shell"], &image, None), rounds(1, 4), "not granted");
}

/// A driver that serves another name than the image's: refused when named
/// with --driver (status 2).
#[test]
fn a_driver_for_another_name_is_refused() {
    let Some((h, dice)) = ready() else { return };
    let (rom, _) = looping(0xF001, &[i_type(6, 0, 10)]);
    let raw = h.path("other.bin");
    std::fs::write(&raw, &rom).unwrap();
    let image = h.path("other.terse");
    let o = Command::new(bin("terse")).args(["pack", "--rom", &raw, "--title", "o", "--ram-pages", "1", "--optional", "241:0=someone.else", "-o", &image]).output().unwrap();
    assert!(o.status.success());
    let d = format!("someone.else={dice}");
    let o = h.bus(&[&image, "--frames", "1", "--driver", &d]);
    assert_eq!(o.status.code(), Some(2), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("example.dice only"));
}

// --- In a shell -----------------------------------------------------------

/// The demo program (`native/driver-experimental/demo.c`) built three
/// ways, each a built-in of the shell with its own title: example.dice on
/// slot 240 (`dice240`), on slot 241 (`dice241`), and a device no driver
/// serves, someone.else, on slot 242 (`other242`). `None` (a skip) where
/// `terse build` has no compiler for the machine.
fn demos(h: &Home) -> Option<[String; 3]> {
    let src = workspace().join("native/driver-experimental/demo.c").display().to_string();
    let mut out = Vec::new();
    for (title, slot, name) in [("dice240", 240, "example.dice"), ("dice241", 241, "example.dice"), ("other242", 242, "someone.else")] {
        let image = h.path(&format!("{title}.terse"));
        let o = Command::new(bin("terse"))
            .args(["build", &src, "--title", title, "--cflag", &format!("-DDICE={slot}"), "--optional", &format!("{slot}:0={name}"), "-o", &image])
            .output()
            .unwrap();
        if !o.status.success() {
            terse_testkit::skip!("clang", "terse build cannot compile demo.c here: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
            return None;
        }
        out.push(image);
    }
    out.try_into().ok()
}

/// The launcher's script: the first built-in after the eight shipped ones
/// (dice240), then from the home key the two after it (the cursor is then
/// on the focused machine's line, below the programs: dice241 is two up,
/// other242 two up again past the new machine's line).
const THREE: &str = "\
2 press down
2 press down
2 press down
2 press down
3 press down
4 press down
5 press down
6 press down
8 press enter
20 home
21 press up
22 press up
24 press enter
40 home
41 press up
42 press up
44 press enter
";

/// `--dump`'s screens by machine: (header line, text).
fn machines(dump: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in dump.lines() {
        if line.starts_with("-- machine ") {
            out.push((line.to_string(), String::new()));
        } else if let Some((_, text)) = out.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    out
}

/// The rolls a screen shows, by turn.
fn turns(screen: &str) -> Vec<&str> {
    screen.lines().filter(|l| l.starts_with("turn ")).collect()
}

/// A shell's machines on the bus, from its built-ins with a script, and
/// their `--dump`.
fn shell(h: &Home, demos: &[String; 3], script: &str, frames: u32, driver: Option<&str>) -> (String, String) {
    let events = h.path("events");
    std::fs::write(&events, script).unwrap();
    let frames = frames.to_string();
    let mut args = vec!["--shell", "--builtin", &demos[0], "--builtin", &demos[1], "--builtin", &demos[2], "--events", &events, "--frames", &frames, "--dump"];
    let d;
    if let Some(x) = driver {
        d = format!("example.dice={x}");
        args.extend(["--driver", &d]);
    }
    let o = h.bus(&args);
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(o.status.success(), "{err}");
    (String::from_utf8_lossy(&o.stdout).into_owned(), err)
}

/// spec/experimental.md §experimental.grants: the demo started from the
/// shell is granted the device and rolls exactly as it does run alone;
/// a second program that calls the same device on slot 241 has its own
/// state (its dice are the same as the first's, not the next ones of one
/// shared generator); a third, whose image names a device no driver
/// serves, runs beside them without it. One driver process serves both.
#[test]
fn machines_of_a_shell_are_granted_each_its_own() {
    let Some((h, dice)) = ready() else { return };
    let Some(demos) = demos(&h) else { return };
    let d = format!("example.dice={dice}");
    let alone = h.bus(&[&demos[0], "--frames", "300", "--dump", "--driver", &d]);
    assert!(alone.status.success());
    let alone = String::from_utf8_lossy(&alone.stdout).into_owned();
    let alone = turns(&alone);
    assert_eq!(alone.len(), 9, "{alone:?}");

    let (dump, _) = shell(&h, &demos, THREE, 300, Some(&dice));
    let m = machines(&dump);
    assert_eq!(m.len(), 3, "{dump}");
    for (i, (title, slot)) in [("dice240", 240), ("dice241", 241)].into_iter().enumerate() {
        let (head, screen) = &m[i];
        assert!(head.contains("running") && head.ends_with(&format!("{title} --")), "{dump}");
        assert!(screen.contains(&format!("Granted, version 0, on slot {slot}.")), "{screen}");
        let t = turns(screen);
        assert!(t.len() >= 7, "{screen}");
        assert!(t.iter().all(|l| alone.contains(l)), "{title}: the same dice as alone: {t:?} {alone:?}");
    }
    let (head, screen) = &m[2];
    assert!(head.contains("running") && head.ends_with("other242 --"), "{dump}");
    assert!(screen.contains("Not granted here") && screen.contains("A call to it now gives FFFFFFFF."), "{screen}");

    // Without the driver, none of them is granted.
    let (dump, _) = shell(&h, &demos, THREE, 60, None);
    let m = machines(&dump);
    assert_eq!(m.len(), 3, "{dump}");
    assert!(m.iter().all(|(_, s)| s.contains("Not granted here")), "{dump}");
}

/// spec/experimental.md §experimental.conformance, in a system: the shell
/// saves its session (home, 1, S); the snapshots of the two granted
/// machines are marked experimental (version 1), the other's is not. A new
/// run with the driver restores them, and they go on rolling the dice of
/// the run that never stopped; a run without it (terse-bus, and terse run
/// --shell, whose devices are terse-host's) fails just those two, with
/// the third restored.
#[test]
fn a_session_with_granted_machines() {
    let Some((h, dice)) = ready() else { return };
    let Some(demos) = demos(&h) else { return };
    let d = format!("example.dice={dice}");
    let alone = h.bus(&[&demos[0], "--frames", "520", "--dump", "--driver", &d]);
    let alone = String::from_utf8_lossy(&alone.stdout).into_owned();
    let alone = turns(&alone);

    let (dump, _) = shell(&h, &demos, &format!("{THREE}200 home\n201 press 1\n202 press s\n"), 210, Some(&dice));
    assert!(dump.contains("Session saved"), "{dump}");
    let dir = std::fs::read_dir(h.tmp.path().join("data/snapshots/program")).unwrap().flatten().next().unwrap().path();
    let dir = std::fs::read_dir(dir).unwrap().flatten().next().unwrap().path();
    let versions: Vec<u8> = ["00", "01", "02"].iter().map(|s| std::fs::read(dir.join(s)).unwrap()[4]).collect();
    assert_eq!(versions, [1, 1, 0], "STATE.md 4: version 1 for a machine that has an experimental device");

    let restore = "4 press enter\n";
    let (dump, _) = shell(&h, &demos, restore, 300, Some(&dice));
    let m = machines(&dump);
    assert_eq!(m.len(), 3, "{dump}");
    for (i, (head, screen)) in m.iter().take(2).enumerate() {
        assert!(head.contains("running"), "{dump}");
        let t = turns(screen);
        assert!(t.len() > 12, "machine {}: it went on: {screen}", i + 1);
        assert!(t.iter().all(|l| alone.contains(l)), "machine {}: as the run that never stopped: {t:?} {alone:?}", i + 1);
    }

    let (dump, _) = shell(&h, &demos, restore, 20, None);
    let m = machines(&dump);
    let states: Vec<bool> = m.iter().map(|(head, _)| head.contains(" failed ")).collect();
    assert_eq!(states, [true, true, false], "{dump}");
    assert!(m[2].1.contains("Not granted here"), "{dump}");

    let events = h.path("events");
    std::fs::write(&events, restore).unwrap();
    let o = h.run(&["--shell", "--builtin", &demos[0], "--builtin", &demos[1], "--builtin", &demos[2], "--events", &events, "--frames", "20", "--dump", "--relay", "none"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let dump = String::from_utf8_lossy(&o.stdout).into_owned();
    let states: Vec<bool> = machines(&dump).iter().map(|(head, _)| head.contains(" failed ")).collect();
    assert_eq!(states, [true, true, false], "terse run --shell (terse-host's devices): {dump}");
}

/// DEVICES.md 15.3, PERFORMANCE.md R15: a slow experimental driver
/// (TERSE_DICE_SLOW: 30 ms in each machine's END from its frame 60 on) is
/// blamed as `dev240`, its `device` line with no slots; at the warning
/// both machines that use it, on slots 240 and 241, are saved (automatic
/// slots 16 and 17), and at the stop both are saved and halted, with a
/// `stop` line each; the third machine, which does not use it, and the
/// shell go on.
#[test]
fn a_slow_driver_stops_every_machine_that_uses_it() {
    use terse_perflog::record::{self, Line};
    use terse_perflog::Component;
    let Some((h, dice)) = ready() else { return };
    let Some(demos) = demos(&h) else { return };
    let events = h.path("events");
    std::fs::write(&events, THREE).unwrap();
    let perf = h.path("perf");
    let d = format!("example.dice={dice}");
    let o = Command::new(bin("terse-bus"))
        .args(["--shell", "--builtin", &demos[0], "--builtin", &demos[1], "--builtin", &demos[2], "--events", &events, "--frames", "200", "--dump"])
        .args(["--driver", &d, "--perf-log", &perf, "--perf-rule", "on"])
        .args(h.dirs())
        .args(["--relay", "none"])
        .env("TERSE_DICE_SLOW", "sleep:30:60")
        .env_remove("TERSE_DICE_BAD")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(o.status.success(), "{err}");
    let mut lines = Vec::new();
    for e in std::fs::read_dir(&perf).unwrap().flatten() {
        lines.extend(record::parse(&std::fs::read_to_string(e.path()).unwrap()));
    }
    let dev = Component::Device(240);
    assert!(lines.iter().any(|l| matches!(l, Line::Device { component, slots, identity } if *component == dev && slots.is_empty() && identity.name == "dice (example)")), "{lines:?}");
    // Where the system gives no CPU-wait readings the rule only warns
    // (RECORD.md 4).
    let waits = lines.iter().any(|l| matches!(l, Line::Readings { component, readings } if *component == dev && readings.wait));
    assert!(err.contains("the experimental device example.dice (dice (example) 0.1.0 unknown) is slow"), "{err}");
    assert!(err.contains("machine 1: snapshot slot 16; machine 2: snapshot slot 17"), "both saved at the warning: {err}");
    assert!(lines.iter().any(|l| matches!(l, Line::Warn { component, .. } if *component == dev)), "{lines:?}");
    if !waits {
        return;
    }
    let stopped: Vec<(u32, String)> = lines.iter().filter_map(|l| if let Line::Stop { component, machine, saved, .. } = l { (*component == dev).then(|| (*machine, saved.clone())) } else { None }).collect();
    assert_eq!(stopped, [(1, "slot16".to_string()), (2, "slot17".to_string())], "{lines:?}");
    let dump = String::from_utf8_lossy(&o.stdout).into_owned();
    let m = machines(&dump);
    assert!(m.len() == 1 && m[0].0.contains("running") && m[0].0.ends_with("other242 --"), "only the machine that does not use it is shown running: {dump}");
    assert!(dump.contains("halted: stopped by Tersenet"), "{dump}");
    // Machine 2 was halted with its own slot for the device, 241: the
    // shell's bar shows the last stop's slot with two digits, so 241 as
    // "H1" (programs/shell's halted_msg; 240 would be "H0").
    assert!(dump.contains("dice241: device H1 was too slow"), "{dump}");
}

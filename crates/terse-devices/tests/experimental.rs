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

fn bin(name: &str) -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_terse-dev-terminal")).parent().unwrap().join(name)
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

/// terse, terse-bus and the dice driver built, or a line saying what is missing.
fn ready() -> Option<(Home, String)> {
    for b in ["terse", "terse-bus"] {
        if !bin(b).exists() {
            eprintln!("skipped: {b} is not built (scripts/cargo build -p terse-cli -p terse-bus)");
            return None;
        }
    }
    let home = Home { tmp: tempfile::tempdir().unwrap() };
    let dice = home.path("dice");
    let built = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-std=c11", "-O2", "-o", &dice])
        .arg(workspace().join("native/driver-experimental/dice.c"))
        .status()
        .is_ok_and(|s| s.success());
    if !built {
        eprintln!("skipped: no C compiler for native/driver-experimental");
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

/// `t0` (x5) after the first frame, from the machine's saved state.
fn t0_after_one_frame(h: &Home, image: &str, driver: Option<&str>) -> u32 {
    let snap = h.path("one.snap");
    let mut args = vec![image, "--frames", "1", "--save-state", &snap];
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
        assert_eq!(t0_after_one_frame(&h, &image, Some(&dice)), rounds(n, charge), "{name}, granted");
        assert_eq!(t0_after_one_frame(&h, &image, None), rounds(n, 4), "{name}, not granted");
    }
}

/// The same image is not granted where there is no driver: terse run and
/// terse-bus without one agree; a system (--shell) grants none.
#[test]
fn not_granted_without_a_driver_or_in_a_system() {
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
    let s = h.bus(&["--shell", &image, "--frames", "3", "--hash", "--driver", &d]);
    assert!(s.status.success(), "{}", String::from_utf8_lossy(&s.stderr));
    assert_eq!(s.stdout, b.stdout, "the shell is not granted it");
    assert!(String::from_utf8_lossy(&s.stderr).contains("not granted in a system"));
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

//! Helpers for the program tests: finding the RISC-V tools (staleness tests
//! are skipped with a message when they are missing), building a program
//! exactly as its `build.sh` does, golden files, and frame statistics.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub mod twin;

use terse_core::{BUDGET_PER_FRAME, Image};

pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

pub fn programs() -> PathBuf {
    workspace().join("programs")
}

pub fn lib() -> PathBuf {
    programs().join("lib")
}

/// Whether clang (with the riscv32 target), ld.lld and llvm-objcopy work.
/// Prints why not, once, when they do not.
pub fn have_tools() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("t.c");
        std::fs::write(&src, "int f(int a) { return a * 3; }\n").unwrap();
        let cc = Command::new("clang")
            .args(["--target=riscv32-unknown-elf", "-march=rv32imc", "-mabi=ilp32", "-c"])
            .arg(&src)
            .arg("-o")
            .arg(dir.path().join("t.o"))
            .output();
        let ok = cc.is_ok_and(|o| o.status.success())
            && ["ld.lld", "llvm-objcopy"]
                .iter()
                .all(|t| Command::new(t).arg("--version").output().is_ok_and(|o| o.status.success()));
        if !ok {
            eprintln!("skipped: needs clang with the riscv32 target, ld.lld and llvm-objcopy");
        }
        ok
    })
}

fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(out.status.success(), "{cmd:?}\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

/// The compiler flags of every programs/*/build.sh.
pub const CFLAGS: [&str; 9] = [
    "--target=riscv32-unknown-elf",
    "-march=rv32imc",
    "-mabi=ilp32",
    "-Os",
    "-ffreestanding",
    "-nostdlib",
    "-fno-builtin",
    "-ffunction-sections",
    "-fdata-sections",
];

/// Compiles `sources` (with crt0.S first and terse.c last when `with_lib`)
/// and links them with programs/lib/terse.ld; returns the raw ROM. The
/// flags and object order are those of the build.sh scripts.
pub fn build_rom(sources: &[PathBuf], with_lib: bool) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let lib = lib();
    let mut all: Vec<PathBuf> = sources.to_vec();
    if with_lib {
        all.insert(0, lib.join("crt0.S"));
        all.push(lib.join("terse.c"));
    }
    let mut objs = Vec::new();
    for (i, src) in all.iter().enumerate() {
        let obj = dir.path().join(format!("{i}.o"));
        run(Command::new("clang").args(CFLAGS).arg("-I").arg(&lib).arg("-c").arg(src).arg("-o").arg(&obj));
        objs.push(obj);
    }
    let elf = dir.path().join("out.elf");
    let bin = dir.path().join("out.bin");
    run(Command::new("ld.lld").arg("-T").arg(lib.join("terse.ld")).arg("--gc-sections").args(&objs).arg("-o").arg(&elf));
    run(Command::new("llvm-objcopy").args(["-O", "binary"]).arg(&elf).arg(&bin));
    std::fs::read(&bin).unwrap()
}

/// Checks that the checked-in image is what its build.sh makes from the
/// sources: the same ROM, and the header given. Skipped without the tools.
pub fn check_up_to_date(image: &[u8], sources: &[&str], with_lib: bool, title: &str, ram_pages: u8, devices: &[(u8, u8)]) {
    let image = Image::parse(image).unwrap();
    assert_eq!(image.title, title.as_bytes());
    assert_eq!(image.ram_pages, ram_pages, "{title}: RAM pages");
    let devs: Vec<(u8, u8)> = image.devices.iter().map(|d| (d.slot, d.version)).collect();
    assert_eq!(devs, devices, "{title}: devices");
    if !have_tools() {
        return;
    }
    let srcs: Vec<PathBuf> = sources.iter().map(|s| programs().join(s)).collect();
    let rom = build_rom(&srcs, with_lib);
    assert!(rom == image.rom, "programs/{title}/{title}.terse is out of date: run programs/{title}/build.sh");
}

/// Compares `got` with the golden file at `path` (relative to programs/);
/// UPDATE_GOLDEN=1 rewrites the file instead.
pub fn golden(path: &str, got: &str) {
    let path = programs().join(path);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e} (UPDATE_GOLDEN=1 writes it)", path.display()));
    assert!(want == got, "{} differs; got:\n{got}", path.display());
}

/// Busiest and typical (median) frame of a run, in budget units.
pub struct FrameStats {
    pub busiest_frame: usize,
    pub busiest: u64,
    pub median: u64,
}

impl FrameStats {
    pub fn new(used: &[u64]) -> Self {
        let (busiest_frame, busiest) = used.iter().copied().enumerate().max_by_key(|&(_, v)| v).unwrap();
        let mut sorted = used.to_vec();
        sorted.sort_unstable();
        FrameStats { busiest_frame, busiest, median: sorted[sorted.len() / 2] }
    }

    pub fn describe(&self) -> String {
        format!(
            "busiest frame {} with {} units ({:.2}% of {BUDGET_PER_FRAME}); typical (median) frame {} units ({:.2}%)",
            self.busiest_frame,
            self.busiest,
            pct(self.busiest),
            self.median,
            pct(self.median),
        )
    }
}

pub fn pct(units: u64) -> f64 {
    units as f64 * 100.0 / BUDGET_PER_FRAME as f64
}

/// The `terse` binary, isolated from the user's own setup: its default
/// config, cache and data directories are in a temporary directory, and the
/// local relay it looks for is on a port nothing listens on (never 7878).
pub fn terse_command() -> Command {
    static ENV: OnceLock<(tempfile::TempDir, u16)> = OnceLock::new();
    let (home, port) = ENV.get_or_init(|| {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        (tempfile::tempdir().unwrap(), port)
    });
    // Compiled into terse-devices' tests (common/twin.rs), the terse
    // program is the one cargo built beside the drivers.
    let exe = option_env!("CARGO_BIN_EXE_terse").map(PathBuf::from).unwrap_or_else(|| {
        let driver = option_env!("CARGO_BIN_EXE_terse-dev-terminal").expect("built in terse-cli or terse-devices");
        Path::new(driver).parent().unwrap().join("terse")
    });
    let mut cmd = Command::new(exe);
    for var in ["XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"] {
        cmd.env(var, home.path().join(var));
    }
    cmd.env(terse_net::local::PORT_ENV, port.to_string());
    cmd
}

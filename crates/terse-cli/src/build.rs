//! `terse build`: C, assembly or Rust source to a `.terse` image, and
//! `terse new`: a starter program. Everything the build needs from this
//! project (programs/lib and its Rust runtime) is built into `terse`; the
//! compiler and linker are the user's own. docs/toolchain.md explains each
//! command and flag.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use terse_core::image::{RAM_PAGES_MAX, TITLE_LEN};
use terse_core::{DeviceReq, Image};

use crate::{Args, parse_device};

/// programs/lib, as `terse build` writes it for the compiler.
const LIB: [(&str, &str); 5] = [
    ("terse.h", include_str!("../../../programs/lib/terse.h")),
    ("terse.c", include_str!("../../../programs/lib/terse.c")),
    ("crt0.S", include_str!("../../../programs/lib/crt0.S")),
    ("terse.ld", include_str!("../../../programs/lib/terse.ld")),
    ("terse.rs", include_str!("../../../programs/lib/rust/terse.rs")),
];

/// clang's flags for C and assembly: those of every programs/*/build.sh.
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

/// Rust's target for Terse32 (RV32IMC, no operating system).
pub const RUST_TARGET: &str = "riscv32imc-unknown-none-elf";

/// rustc's flags for the runtime and the program alike.
const RUSTFLAGS: [&str; 7] = ["--edition", "2024", "--target", RUST_TARGET, "-C", "opt-level=s", "-Cpanic=abort"];

/// The devices of the program profile (SPEC §profiles) besides System,
/// which is always there: the terminal and the 2D vector display.
const PROFILE_DEVICES: [(u8, u8); 2] = [(2, 0), (3, 0)];

/// RAM left for the stack above .data and .bss when --ram-pages is not
/// given: the RAM is the program's data plus this, in whole pages.
const STACK: u32 = 8192;

const ROM_BASE: u32 = 0x0001_0000;
const RAM_BASE: u32 = 0x0010_0000;
const ROM_MAX: u32 = terse_core::image::ROM_MAX as u32;
/// SPEC §profile: a RAM page is 8 KiB.
const PAGE: u32 = 8192;

const BUILD_USAGE: &str = "usage:
  terse build <source>... [-o <out.terse>] [--title <t>] [--ram-pages <n>] [--device <slot:version>]...
              [--optional <slot:version>]... [--cflag <flag>]... [--clang <path>] [--lld <path>] [--rustc <path>]
              [--show] [--run [the options of terse run]...]
  terse build --write-lib <dir>
  terse new <file.c|file.rs|file.S>
Sources: C (.c) and assembly (.S, .s), several of them, or one Rust crate root (.rs).
-o: default <first source's stem>.terse in the current directory. --title: default that stem
(at most 16 bytes). --device and --optional as in `terse pack`; without --device the image requires
the program profile's devices, 2:0 (terminal) and 3:0 (2D vector display). --ram-pages: default the
program's data and .bss plus 8 KiB for the stack, in 8 KiB pages. --show prints each command as it
runs it. --run then runs the image as `terse run` does, with the options after it.
--write-lib writes the files built into terse (programs/lib and its Rust runtime) to <dir>, for
building by hand (docs/toolchain.md). Tools: clang, ld.lld (C and assembly) and rustc with the
riscv32imc-unknown-none-elf target (Rust); also named by the CLANG, LLD and RUSTC variables.";

/// A temporary directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!("terse-build-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(TempDir(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_lib(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, text) in LIB {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// A tool: what the user named (option, then variable), else the default.
struct Tool {
    program: OsString,
    /// One sentence: what to install.
    missing: &'static str,
}

impl Tool {
    fn new(args: &Args, option: &str, var: &str, default: &str, missing: &'static str) -> Tool {
        let program = match args.get(option) {
            Some(p) => p.into(),
            None => std::env::var_os(var).filter(|v| !v.is_empty()).unwrap_or_else(|| default.into()),
        };
        Tool { program, missing }
    }

    fn not_found(&self) -> String {
        format!("{} was not found: {}", self.program.to_string_lossy(), self.missing)
    }

    fn command(&self) -> Command {
        Command::new(&self.program)
    }
}

const CLANG_MISSING: &str = "install clang and lld (LLVM's C compiler and linker; the packages of most systems include the RISC-V target), or name clang with --clang <path> or CLANG.";
const LLD_MISSING: &str = "install lld (LLVM's linker), or name it with --lld <path> or LLD.";
const RUSTC_MISSING: &str = "install Rust with rustup (rustup.rs), then run `rustup target add riscv32imc-unknown-none-elf`; or name rustc with --rustc <path> or RUSTC.";

/// A command line as it could be typed into a shell.
fn shown(cmd: &Command) -> String {
    let quote = |s: &std::ffi::OsStr| {
        let s = s.to_string_lossy();
        if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_=./,:+@%".contains(c)) {
            s.into_owned()
        } else {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
    };
    std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(quote).collect::<Vec<_>>().join(" ")
}

/// Runs a tool; its messages (the compiler's errors and warnings) go to
/// stderr. A tool that is not there, or cannot do the job for a reason the
/// user fixes by installing something, is one sentence saying what.
fn exec(tool: &Tool, cmd: &mut Command, show: bool) -> Result<(), String> {
    if show {
        println!("{}", shown(cmd));
    }
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => tool.not_found(),
        _ => format!("{}: {e}", tool.program.to_string_lossy()),
    })?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        if stderr.contains("unable to create target") || stderr.contains("No available targets") || stderr.contains("unknown target triple") {
            return Err(format!(
                "{} cannot compile for RISC-V: install a clang built with the RISC-V target (most systems' packages are), or name it with --clang <path> or CLANG.",
                tool.program.to_string_lossy()
            ));
        }
        if stderr.contains("linker `rust-lld` not found") {
            return Err("rustc's linker, rust-lld, was not found: install Rust with rustup (rustup.rs), whose toolchains include it.".into());
        }
    }
    eprint!("{stderr}");
    std::io::Write::write_all(&mut std::io::stdout(), &out.stdout).ok();
    if out.status.success() { Ok(()) } else { Err(format!("{} failed (its messages are above)", tool.program.to_string_lossy())) }
}

/// The source's language.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    C,
    Asm,
    Rust,
}

fn lang(path: &str) -> Result<Lang, String> {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("c") => Ok(Lang::C),
        Some("S" | "s") => Ok(Lang::Asm),
        Some("rs") => Ok(Lang::Rust),
        _ => Err(format!("{path}: not a source terse build knows (.c, .S or .s, .rs)")),
    }
}

fn stem(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The first `max` bytes of `s`, cut at a character boundary.
fn truncate(s: &str, max: usize) -> &str {
    let mut n = s.len().min(max);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

pub fn build(all: &[String]) -> Result<ExitCode, String> {
    // Everything after --run is for `terse run`.
    let (ours, run_args) = match all.iter().position(|a| a == "--run") {
        Some(i) => (&all[..i], Some(&all[i + 1..])),
        None => (all, None),
    };
    let args = Args::parse(ours, &["--show"])?;
    if let Some((n, _)) = args.options.iter().find(|(n, _)| !BUILD_OPTIONS.contains(&n.as_str())) {
        return Err(format!("unknown option {n}\n{BUILD_USAGE}"));
    }
    if let Some(dir) = args.get("--write-lib") {
        if !args.positional.is_empty() || args.options.len() > 1 {
            return Err(format!("--write-lib takes nothing else\n{BUILD_USAGE}"));
        }
        write_lib(Path::new(dir))?;
        eprintln!("{dir}: {}", LIB.map(|(n, _)| n).join(", "));
        return Ok(ExitCode::SUCCESS);
    }
    let sources = &args.positional;
    let Some(first) = sources.first() else {
        return Err(BUILD_USAGE.to_string());
    };
    let langs = sources.iter().map(|s| lang(s)).collect::<Result<Vec<_>, _>>()?;
    let rust = langs.contains(&Lang::Rust);
    if rust && sources.len() > 1 {
        return Err("a Rust program is one .rs file, the crate root (its modules are found from it); C and assembly may be several files".into());
    }
    if rust && args.has("--cflag") {
        return Err("--cflag is for C and assembly".into());
    }
    for s in sources {
        if !Path::new(s).is_file() {
            return Err(format!("{s}: no such file"));
        }
    }
    let name = stem(first);
    let out = args.get("-o").map(String::from).unwrap_or(format!("{name}.terse"));
    let title = match args.get("--title") {
        Some(t) if t.len() > TITLE_LEN => return Err(format!("title is {} bytes, at most {TITLE_LEN} allowed", t.len())),
        Some(t) => t.to_string(),
        None => truncate(&name, TITLE_LEN).to_string(),
    };
    let ram_pages = match args.get("--ram-pages") {
        None => None,
        Some(n) => Some(n.parse().ok().filter(|n| (1..=RAM_PAGES_MAX).contains(n)).ok_or(format!("bad RAM page count {n} (1 to {RAM_PAGES_MAX})"))?),
    };
    let devices = devices(&args)?;
    let show = args.has("--show");

    let tmp = TempDir::new()?;
    let lib = tmp.0.join("lib");
    write_lib(&lib)?;
    if show {
        println!("# {}: the files built into terse (terse build --write-lib <dir> writes them)", lib.display());
    }
    let elf = tmp.0.join("out.elf");
    if rust {
        build_rust(&args, first, &name, &lib, &tmp.0, &elf, show)?;
    } else {
        build_c(&args, sources, &langs, &lib, &tmp.0, &elf, show)?;
    }
    let elf_bytes = std::fs::read(&elf).map_err(|e| format!("{}: {e}", elf.display()))?;
    let (rom, ram_used) = rom_from_elf(&elf_bytes)?;
    let ram_pages = match ram_pages {
        Some(n) if (n as u32) * PAGE < ram_used => {
            return Err(format!("the program's data and .bss take {ram_used} bytes, more than {n} RAM pages ({} bytes)", n as u32 * PAGE));
        }
        Some(n) => n,
        None => {
            let n = (ram_used + STACK).div_ceil(PAGE);
            u8::try_from(n).ok().filter(|&n| n <= RAM_PAGES_MAX).ok_or(format!(
                "the program's data and .bss take {ram_used} bytes: with 8 KiB of stack that is more than {RAM_PAGES_MAX} RAM pages ({} KiB)",
                RAM_PAGES_MAX as u32 * 8
            ))?
        }
    };
    if show {
        println!(
            "# the ROM is the ELF file's loaded bytes from {ROM_BASE:08x}h (llvm-objcopy -O binary {} rom.bin makes the same), then:",
            elf.display()
        );
        let mut pack = vec!["terse".to_string(), "pack".into(), "--rom".into(), "rom.bin".into(), "--title".into(), title.clone()];
        pack.extend(["--ram-pages".into(), ram_pages.to_string()]);
        for d in &devices {
            let name = crate::experimental_names(&args).into_iter().find(|(s, _)| *s == d.slot).map_or(String::new(), |(_, n)| format!("={n}"));
            pack.extend([if d.optional { "--optional" } else { "--device" }.into(), format!("{}:{}{name}", d.slot, d.version)]);
        }
        pack.extend(["-o".into(), out.clone()]);
        let mut cmd = Command::new(&pack[0]);
        cmd.args(&pack[1..]);
        println!("{}", shown(&cmd));
    }
    // spec/experimental.md: the names of experimental devices end the ROM.
    let rom = crate::with_experimental_table(&args, &devices, rom)?;
    let file = Image::build(&title, ram_pages, &devices, &rom);
    let image = Image::parse(&file).map_err(|e| e.to_string())?;
    crate::write(&out, &file)?;
    eprintln!(
        "{out}: {} bytes (ROM {}), RAM {ram_pages} page{} ({ram_used} bytes of data, the rest stack), id {}",
        file.len(),
        rom.len(),
        if ram_pages == 1 { "" } else { "s" },
        image.id_hex()
    );
    drop(tmp);
    match run_args {
        Some(rest) => {
            let mut run = vec![out];
            run.extend(rest.iter().cloned());
            crate::run(&run)
        }
        None => Ok(ExitCode::SUCCESS),
    }
}

/// The options before --run (everything after it is terse run's).
const BUILD_OPTIONS: [&str; 11] = [
    "-o",
    "--title",
    "--ram-pages",
    "--device",
    "--optional",
    "--cflag",
    "--clang",
    "--lld",
    "--rustc",
    "--show",
    "--write-lib",
];

/// The header's devices: --device as given, else the program profile's;
/// then --optional (each slot once: an --optional slot is not also required).
fn devices(args: &Args) -> Result<Vec<DeviceReq>, String> {
    let optional = args.all("--optional").map(|d| parse_device(d, true)).collect::<Result<Vec<_>, _>>()?;
    let mut required = args.all("--device").map(|d| parse_device(d, false)).collect::<Result<Vec<_>, _>>()?;
    if !args.has("--device") {
        required = PROFILE_DEVICES
            .iter()
            .filter(|(slot, _)| !optional.iter().any(|o| o.slot == *slot))
            .map(|&(slot, version)| DeviceReq { slot, version, optional: false })
            .collect();
    }
    let all: Vec<DeviceReq> = required.into_iter().chain(optional).collect();
    if all.len() > terse_core::image::MAX_DEVICES {
        return Err("more than 16 devices".into());
    }
    Ok(all)
}

/// C and assembly: crt0.S first (when there is C: an assembly-only program
/// starts itself, at _start), the sources in order, terse.c last, as the
/// build.sh scripts do; --gc-sections drops what nothing uses.
fn build_c(args: &Args, sources: &[String], langs: &[Lang], lib: &Path, tmp: &Path, elf: &Path, show: bool) -> Result<(), String> {
    let clang = Tool::new(args, "--clang", "CLANG", "clang", CLANG_MISSING);
    let lld = Tool::new(args, "--lld", "LLD", "ld.lld", LLD_MISSING);
    let mut inputs: Vec<PathBuf> = Vec::new();
    if langs.contains(&Lang::C) {
        inputs.push(lib.join("crt0.S"));
    }
    inputs.extend(sources.iter().map(PathBuf::from));
    inputs.push(lib.join("terse.c"));
    let mut objs = Vec::new();
    for (i, src) in inputs.iter().enumerate() {
        let obj = tmp.join(format!("{i}.o"));
        let mut cmd = clang.command();
        cmd.args(CFLAGS).arg("-I").arg(lib).args(args.all("--cflag")).arg("-c").arg(src).arg("-o").arg(&obj);
        exec(&clang, &mut cmd, show)?;
        objs.push(obj);
    }
    let mut cmd = lld.command();
    cmd.arg("-T").arg(lib.join("terse.ld")).arg("--gc-sections").args(&objs).arg("-o").arg(elf);
    exec(&lld, &mut cmd, show)
}

/// Rust: the runtime (terse.rs) as an rlib, then the program against it,
/// linked by rustc's own rust-lld with programs/lib's linker script.
fn build_rust(args: &Args, source: &str, name: &str, lib: &Path, tmp: &Path, elf: &Path, show: bool) -> Result<(), String> {
    let rustc = Tool::new(args, "--rustc", "RUSTC", "rustc", RUSTC_MISSING);
    check_rust_target(&rustc)?;
    let rlib = tmp.join("libterse.rlib");
    // The runtime's panics name it as terse.rs, not by the temporary
    // directory, so a build gives the same image every time.
    let remap = format!("--remap-path-prefix={}/=", lib.display());
    let mut cmd = rustc.command();
    cmd.args(RUSTFLAGS).arg(&remap).args(["--crate-type", "rlib", "--crate-name", "terse", "-o"]).arg(&rlib).arg(lib.join("terse.rs"));
    exec(&rustc, &mut cmd, show)?;
    // A crate name is an identifier: the file's stem with anything else as _.
    let mut crate_name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if !crate_name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        crate_name.insert_str(0, "program_");
    }
    let mut cmd = rustc.command();
    cmd.args(RUSTFLAGS)
        .arg(&remap)
        .args(["-C", "lto", "-C", "strip=debuginfo", "--crate-type", "bin", "--crate-name", &crate_name])
        .arg("-C")
        .arg(format!("link-arg=-T{}", lib.join("terse.ld").display()))
        .arg("--extern")
        .arg(format!("terse={}", rlib.display()))
        .arg("-o")
        .arg(elf)
        .arg(source);
    exec(&rustc, &mut cmd, show)
}

/// Whether rustc has the RISC-V target's libraries (rustup installs them).
fn check_rust_target(rustc: &Tool) -> Result<(), String> {
    let out = rustc.command().args(["--print", "target-libdir", "--target", RUST_TARGET]).output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => rustc.not_found(),
        _ => format!("{}: {e}", rustc.program.to_string_lossy()),
    })?;
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let has_core = std::fs::read_dir(&dir).is_ok_and(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("libcore-")));
    if out.status.success() && !has_core {
        return Err(format!("Rust's {RUST_TARGET} target is not installed: run `rustup target add {RUST_TARGET}`."));
    }
    Ok(())
}

/// The ROM and the RAM the program's data and .bss take, from the linked
/// ELF file: each loaded segment's file bytes at its load (physical)
/// address, from 00010000h on, gaps zero (what `llvm-objcopy -O binary`
/// writes); the RAM from the segments' run (virtual) addresses.
pub fn rom_from_elf(elf: &[u8]) -> Result<(Vec<u8>, u32), String> {
    let bad = |why: &str| format!("the linker's output is not a Terse32 program: {why}");
    let u16_at = |o: usize| elf.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| elf.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if elf.get(..6) != Some(b"\x7fELF\x01\x01") || u16_at(18) != Some(0xF3) {
        return Err(bad("not a 32-bit little-endian RISC-V ELF file"));
    }
    let entry = u32_at(24).ok_or_else(|| bad("truncated"))?;
    let phoff = u32_at(28).ok_or_else(|| bad("truncated"))? as usize;
    let phentsize = u16_at(42).ok_or_else(|| bad("truncated"))? as usize;
    let phnum = u16_at(44).ok_or_else(|| bad("truncated"))? as usize;
    let mut rom: Vec<u8> = Vec::new();
    let mut ram_end = RAM_BASE;
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let field = |n: usize| u32_at(ph + 4 * n).ok_or_else(|| bad("truncated program header"));
        let (kind, offset, vaddr, paddr, filesz, memsz) = (field(0)?, field(1)?, field(2)?, field(3)?, field(4)?, field(5)?);
        if kind != 1 {
            continue; // not PT_LOAD
        }
        if vaddr >= RAM_BASE && memsz > 0 {
            ram_end = ram_end.max(vaddr.saturating_add(memsz));
        }
        if filesz == 0 {
            continue;
        }
        if paddr < ROM_BASE || paddr.saturating_add(filesz) > ROM_BASE + ROM_MAX {
            return Err(format!(
                "{filesz} bytes to load at {paddr:08x}h, outside the ROM (00010000h, at most {} KiB): is the program larger than {} KiB?",
                ROM_MAX / 1024,
                ROM_MAX / 1024
            ));
        }
        let at = (paddr - ROM_BASE) as usize;
        let bytes = elf.get(offset as usize..offset as usize + filesz as usize).ok_or_else(|| bad("truncated segment"))?;
        if rom.len() < at + bytes.len() {
            rom.resize(at + bytes.len(), 0);
        }
        rom[at..at + bytes.len()].copy_from_slice(bytes);
    }
    if rom.is_empty() {
        return Err(bad("nothing to load"));
    }
    if entry != ROM_BASE {
        return Err(format!(
            "the entry point _start is at {entry:08x}h, not at the start of ROM (00010000h): put it in the section .text.start"
        ));
    }
    Ok((rom, ram_end - RAM_BASE))
}

/// `terse new <file>`: a starter program terse build takes unchanged.
pub fn new(args: &[String]) -> Result<ExitCode, String> {
    let [path] = args else {
        return Err(BUILD_USAGE.to_string());
    };
    let text = match lang(path)? {
        Lang::C => NEW_C,
        Lang::Rust => NEW_RS,
        Lang::Asm => NEW_S,
    };
    let name = Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let text = text.replace("{file}", &name).replace("{stem}", &stem(path));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{path} already exists: terse new does not overwrite a file"),
        _ => format!("{path}: {e}"),
    })?;
    std::io::Write::write_all(&mut file, text.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
    eprintln!("{path}: written; build and run it with\n  terse build {path}\n  terse run {}.terse --frames 60 --dump", stem(path));
    Ok(ExitCode::SUCCESS)
}

const NEW_C: &str = r#"/* {file}: a Tersenet program in C. Build it and run it with
 *     terse build {file}
 *     terse run {stem}.terse --frames 60 --dump
 * The machine's calls are in terse.h, which terse build provides (terse
 * build --write-lib <dir> writes it out to read). */

#include "terse.h"

int main(void)
{
    term_color(15, 4); /* white on blue */
    term_puts(" Hello from {stem}! ");
    term_color(7, 0);
    term_puts("\n\nPress Esc to exit.\n");

    for (;;) {
        terse_event e;

        /* The machine runs 60 frames a second; a program does its work
         * for one frame, then waits for the next. */
        terse_wait_frame();

        /* Keys, typed text and the pointer arrive as events. */
        while (terse_event_pop(&e)) {
            if (e.type == EV_KEY_DOWN && e.a == KEY_ESCAPE)
                return 0; /* the exit code */
        }
    }
}
"#;

const NEW_RS: &str = r#"//! {file}: a Tersenet program in Rust. Build it and run it with
//!     terse build {file}
//!     terse run {stem}.terse --frames 60 --dump
//! The machine's calls are in the crate `terse`, which terse build provides
//! (terse build --write-lib <dir> writes its source, terse.rs, out to read).
#![no_std]
#![no_main]

use terse::*;

// The function the machine starts with.
entry!(main);

fn main() {
    term_color(15, 4); // white on blue
    term_puts(" Hello from {stem}! ");
    term_color(7, 0);
    term_puts("\n\nPress Esc to exit.\n");

    loop {
        // The machine runs 60 frames a second; a program does its work for
        // one frame, then waits for the next.
        wait_frame();

        // Keys, typed text and the pointer arrive as events.
        while let Some(e) = next_event() {
            if e.kind == EV_KEY_DOWN && e.key() == KEY_ESCAPE {
                exit(0);
            }
        }
    }
}
"#;

const NEW_S: &str = r#"/* {file}: a Tersenet program in RISC-V assembly (RV32IMC). Build it and
 * run it with
 *     terse build {file}
 *     terse run {stem}.terse --frames 60 --dump
 * The machine starts at _start, the first byte of ROM, with sp at the end
 * of RAM. A device call is an ecall: a7 = (slot << 8) | function, the
 * arguments in a0-a5, the result in a0 (SPEC.md §calls). */

    .equ WAIT_FRAME,    0x100
    .equ NEXT_EVENT,    0x101
    .equ EXIT,          0x106
    .equ TERM_WRITE,    0x200
    .equ EV_KEY_DOWN,   1
    .equ KEY_ESCAPE,    0x29

    .section .text.start, "ax"
    .globl _start
_start:
    la      a0, hello
    li      a1, hello_end - hello
    li      a7, TERM_WRITE
    ecall

frame:
    li      a7, WAIT_FRAME          /* one pass per frame, 60 a second */
    ecall
events:
    la      a0, event
    li      a7, NEXT_EVENT          /* 0 when there are no more */
    ecall
    beqz    a0, frame
    la      t0, event
    lbu     t1, 0(t0)               /* the event's type */
    lbu     t2, 1(t0)               /* a key's code */
    li      t3, EV_KEY_DOWN
    bne     t1, t3, events
    li      t3, KEY_ESCAPE
    bne     t2, t3, events
    li      a0, 0                   /* Esc: exit with code 0 */
    li      a7, EXIT
    ecall

    .section .rodata
hello:  .ascii "Hello from {stem}!\n\nPress Esc to exit.\n"
hello_end:

    .section .bss
    .balign 4
event:  .space 4
"#;

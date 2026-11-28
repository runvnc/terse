//! `terse build` and `terse new`: hello in C, assembly and Rust built and run
//! headlessly to the same screen as programs/hello; the programs of
//! programs/ that use programs/lib built byte for byte as their build.sh
//! builds them; the starters of `terse new`; and the one-sentence messages
//! for a missing tool. Tests that need clang and ld.lld, or rustc with the
//! riscv32imc-unknown-none-elf target, are skipped with a line saying so.

mod common;

use std::path::Path;
use std::process::Output;
use std::sync::OnceLock;

use terse_core::Image;

const TARGET: &str = "riscv32imc-unknown-none-elf";

/// Whether rustc has the RISC-V target (rustup target add ...); a skip for
/// the calling test when it has not.
fn have_rust() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    let ok = *OK.get_or_init(|| {
        let dir = std::process::Command::new("rustc")
            .args(["--print", "target-libdir", "--target", TARGET])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let ok = dir.is_some_and(|d| {
            std::fs::read_dir(d).is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("libcore-"))))
        });
        ok
    });
    if !ok {
        terse_testkit::skip!("rust-target", "needs rustc with the {TARGET} target (rustup target add {TARGET})");
    }
    ok
}

fn terse_in(dir: &Path, args: &[&str]) -> Output {
    common::terse_command().current_dir(dir).args(args).output().unwrap()
}

fn ok(out: Output, what: &str) -> Output {
    assert!(out.status.success(), "{what}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    out
}

fn program(path: &str) -> String {
    common::programs().join(path).to_str().unwrap().to_string()
}

/// The typed input of tests/hello.rs, as scripted events.
const HELLO_EVENTS: &str = "5 text Hi wörld \n6 text 你好\n7 press enter\n8 text 😀x\n9 down backspace\n10 text ok\n";

/// Builds `source` (with `extra` options) to hello.terse in `dir`, runs it
/// 30 frames with hello's typed input and compares the screen with
/// programs/hello/expected-30-frames.txt; returns the image.
fn hello_screen(dir: &Path, source: &str, extra: &[&str]) -> Vec<u8> {
    let mut args = vec!["build", source];
    args.extend(extra);
    let out = ok(terse_in(dir, &args), source);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.starts_with("hello.terse: "), "{said}");
    eprintln!("{said}");
    std::fs::write(dir.join("hello.events"), HELLO_EVENTS).unwrap();
    let out = ok(terse_in(dir, &["run", "hello.terse", "--frames", "30", "--dump", "--events", "hello.events"]), "run");
    let want = std::fs::read_to_string(common::programs().join("hello/expected-30-frames.txt")).unwrap();
    assert!(String::from_utf8_lossy(&out.stdout) == want, "{source}: screen differs:\n{}", String::from_utf8_lossy(&out.stdout));
    std::fs::read(dir.join("hello.terse")).unwrap()
}

#[test]
fn hello_in_c_and_assembly() {
    if !common::have_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Defaults: the stem as title, the program profile's devices, RAM for
    // the data and .bss plus 8 KiB of stack.
    let image = hello_screen(dir.path(), &program("hello/hello.c"), &[]);
    let image = Image::parse(&image).unwrap();
    assert_eq!(image.title, b"hello");
    let devs: Vec<(u8, u8)> = image.devices.iter().map(|d| (d.slot, d.version_byte())).collect();
    assert_eq!(devs, [(2, 0), (3, 0)]);
    assert_eq!(image.ram_pages, 2, "4 bytes of .bss and 8 KiB of stack");

    // hello.S with build.sh's header: the checked-in image, byte for byte.
    let image = hello_screen(dir.path(), &program("hello/hello.S"), &["--device", "2:0", "--optional", "10:0", "--ram-pages", "1"]);
    assert!(image == std::fs::read(common::programs().join("hello/hello.terse")).unwrap(), "hello.S: not hello.terse");
}

#[test]
fn hello_in_rust() {
    if !have_rust() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let first = hello_screen(dir.path(), &program("hello-rs/hello.rs"), &["--optional", "10:0"]);
    let image = Image::parse(&first).unwrap();
    let devs: Vec<(u8, u8)> = image.devices.iter().map(|d| (d.slot, d.version_byte())).collect();
    assert_eq!(devs, [(2, 0), (3, 0), (10, 0x80)]);
    assert!(image.rom.len() < 4096, "hello in Rust: {} bytes of ROM", image.rom.len());
    // The same image again: nothing of the temporary directory is in it.
    let again = hello_screen(dir.path(), &program("hello-rs/hello.rs"), &["--optional", "10:0"]);
    assert!(first == again, "two builds of hello.rs differ");
}

/// Each program of programs/ that uses programs/lib, built by `terse
/// build` with its build.sh's title, RAM and devices: its checked-in image.
#[test]
fn programs_build_as_their_build_sh_does() {
    if !common::have_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let programs: [(&str, &str, &[&str]); 10] = [
        ("conform-perf", "conform-perf.c", &["--ram-pages", "1", "--device", "2:0", "--device", "4:0"]),
        ("fetch", "fetch.c", &["--ram-pages", "1", "--device", "2:0", "--device", "4:0", "--optional", "10:0"]),
        ("holo", "holo.c", &["--ram-pages", "2", "--device", "2:0", "--device", "9:0"]),
        ("hostile", "hostile.S", &["--ram-pages", "2", "--device", "2:0", "--device", "3:0", "--device", "7:0"]),
        ("reader", "reader.c", &["--ram-pages", "45", "--device", "2:0", "--device", "4:0", "--device", "7:0", "--device", "10:0"]),
        ("reports", "reports.c", &["--ram-pages", "32", "--device", "2:0", "--device", "4:0", "--optional", "10:0"]),
        (
            "shell",
            "shell.c",
            &[
                "--ram-pages", "3", "--device", "2:0", "--device", "3:0", "--device", "4:0", "--device", "7:0", "--device", "8:0",
                "--device", "9:0", "--optional", "10:0",
            ],
        ),
        ("sketch", "sketch.c", &["--ram-pages", "4", "--device", "2:0", "--device", "3:0", "--optional", "4:0"]),
        ("sparks", "sparks.c", &["--ram-pages", "1", "--device", "2:0", "--device", "3:0", "--optional", "4:0", "--optional", "10:0"]),
        ("tune", "tune.c", &["--ram-pages", "1", "--device", "2:0", "--device", "6:0"]),
    ];
    for (name, source, flags) in programs {
        let out = format!("{name}.terse");
        let mut args = vec!["build", "--title", name, "-o", &out];
        let source = program(&format!("{name}/{source}"));
        args.push(&source);
        args.extend(flags);
        ok(terse_in(dir.path(), &args), name);
        let got = std::fs::read(dir.path().join(&out)).unwrap();
        let want = std::fs::read(common::programs().join(format!("{name}/{name}.terse"))).unwrap();
        assert!(got == want, "terse build {name}: not the checked-in {name}.terse");
    }
}

#[test]
fn run_and_show() {
    if !common::have_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = program("hello/hello.S");
    let out = ok(terse_in(dir.path(), &["build", &src, "-o", "h.terse", "--show", "--run", "--frames", "3", "--dump"]), "build --run");
    let text = String::from_utf8(out.stdout).unwrap();
    // The commands first, then the run's screen.
    let commands: Vec<&str> = text.lines().take(4).collect();
    assert!(commands[0].starts_with("# ") && commands[0].ends_with("/lib: the files built into terse (terse build --write-lib <dir> writes them)"));
    assert!(commands[1].starts_with("clang --target=riscv32-unknown-elf -march=rv32imc -mabi=ilp32 -Os "), "{}", commands[1]);
    assert!(commands[1].contains("hello.S -o "), "{}", commands[1]);
    assert!(commands[2].starts_with("clang ") && commands[2].contains("/lib/terse.c -o "), "{}", commands[2]);
    assert!(commands[3].starts_with("ld.lld -T ") && commands[3].contains("/lib/terse.ld --gc-sections "), "{}", commands[3]);
    assert!(text.contains("\nterse pack --rom rom.bin --title hello --ram-pages 2 --device 2:0 --device 3:0 -o h.terse\n"), "{text}");
    assert!(text.contains(" Tersenet · hello"), "{text}");
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("h.terse: "));

    // --write-lib: the files built into terse, which are programs/lib's.
    ok(terse_in(dir.path(), &["build", "--write-lib", "lib"]), "--write-lib");
    for (name, from) in
        [("terse.h", "lib/terse.h"), ("terse.c", "lib/terse.c"), ("crt0.S", "lib/crt0.S"), ("terse.ld", "lib/terse.ld"), ("terse.rs", "lib/rust/terse.rs")]
    {
        assert!(std::fs::read(dir.path().join("lib").join(name)).unwrap() == std::fs::read(common::programs().join(from)).unwrap(), "{name}");
    }
}

/// `terse new` writes a starter that builds and runs unchanged, and never
/// overwrites a file.
#[test]
fn new_starters_build_and_run() {
    let dir = tempfile::tempdir().unwrap();
    for (file, needs_rust) in [("game.c", false), ("game.S", false), ("game.rs", true)] {
        ok(terse_in(dir.path(), &["new", file]), file);
        let again = terse_in(dir.path(), &["new", file]);
        assert_eq!(again.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&again.stderr).contains(&format!("{file} already exists")));
        if if needs_rust { !have_rust() } else { !common::have_tools() } {
            continue;
        }
        let out = ok(terse_in(dir.path(), &["build", file, "--run", "--frames", "5", "--dump"]), file);
        let screen = String::from_utf8(out.stdout).unwrap();
        assert!(screen.contains("Hello from game!") && screen.contains("Press Esc to exit."), "{file}:\n{screen}");
        // Esc exits with code 0.
        std::fs::write(dir.path().join("esc.events"), "3 press esc\n").unwrap();
        let out = terse_in(dir.path(), &["run", "game.terse", "--events", "esc.events", "--frames", "100"]);
        assert_eq!(out.status.code(), Some(0), "{file}");
        std::fs::remove_file(dir.path().join("game.terse")).unwrap();
    }
}

/// A Rust panic shows where it happened and stops the machine with a fault.
#[test]
fn rust_panic_is_a_fault() {
    if !have_rust() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("oops.rs"),
        "#![no_std]\n#![no_main]\nuse terse::*;\nentry!(main);\nfn main() {\n    let v = [1u8, 2, 3];\n    term_put_dec(v[time() as usize + 7] as u32);\n}\n",
    )
    .unwrap();
    let out = terse_in(dir.path(), &["build", "oops.rs", "--run", "--dump"]);
    let screen = String::from_utf8_lossy(&out.stdout);
    assert!(screen.contains("panic at oops.rs:7"), "{screen}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("ebreak"), "{}", String::from_utf8_lossy(&out.stderr));
    #[cfg(unix)]
    assert_eq!(std::os::unix::process::ExitStatusExt::signal(&out.status), Some(15), "a fault ends by SIGTERM");
}

/// A missing tool is one sentence saying what to install, not an error dump.
#[test]
fn missing_tools() {
    let dir = tempfile::tempdir().unwrap();
    ok(terse_in(dir.path(), &["new", "a.c"]), "new");
    ok(terse_in(dir.path(), &["new", "b.rs"]), "new");
    let cases = [
        (vec!["build", "a.c", "--clang", "/nonexistent/clang"], "terse: /nonexistent/clang was not found: install clang and lld"),
        (vec!["build", "b.rs", "--rustc", "/nonexistent/rustc"], "terse: /nonexistent/rustc was not found: install Rust with rustup"),
    ];
    for (args, want) in cases {
        let out = terse_in(dir.path(), &args);
        assert_eq!(out.status.code(), Some(2));
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.starts_with(want) && said.lines().count() == 1, "{args:?}: {said}");
    }
    if common::have_tools() {
        let out = terse_in(dir.path(), &["build", "a.c", "--lld", "/nonexistent/ld.lld"]);
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.starts_with("terse: /nonexistent/ld.lld was not found: install lld") && said.lines().count() == 1, "{said}");
    }
    // A rustc without the RISC-V target: here, one whose target directory
    // does not exist.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let fake = dir.path().join("rustc");
        std::fs::write(&fake, "#!/bin/sh\necho /nonexistent/lib\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = terse_in(dir.path(), &["build", "b.rs", "--rustc", fake.to_str().unwrap()]);
        let said = String::from_utf8_lossy(&out.stderr);
        assert_eq!(said, format!("terse: Rust's {TARGET} target is not installed: run `rustup target add {TARGET}`.\n"));
    }
    // The compiler's own errors are passed on.
    if common::have_tools() {
        std::fs::write(dir.path().join("bad.c"), "int main(void) { return nope; }\n").unwrap();
        let out = terse_in(dir.path(), &["build", "bad.c"]);
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.contains("nope") && said.ends_with("terse: clang failed (its messages are above)\n"), "{said}");
    }
}

//! The implementation guide's example programs (`examples/host`,
//! `examples/device`), in Go and in Rust, built with their own toolchains
//! and checked against the reference:
//!
//! - the listings in the guide's articles (a fence opening with "```go
//!   from <file>" or "```rust from <file>") are lines of the files, as
//!   they are (a Go file's tabs shown as four spaces), and every line of
//!   an example and of these articles' fences fits the manual's 64
//!   columns (scripts/manual-listings.py's whole-file listings are
//!   another form, checked by the examples test);
//! - each step of each host gives what its article says: exit statuses 7,
//!   186 and 6 for the tiny programs written by the articles' own `xxd`
//!   commands, `hello`'s screen as `terse run --dump` shows it, and the
//!   state hashes of `terse run --hash` (and of programs that fault);
//! - each minimal terminal driver gives the reference terminal's hash after
//!   every frame of `hello` under `terse-bus`, and works under the other
//!   language's host (crosswise).
//!
//! A test that lacks Go or cargo, or cannot fetch an example's dependency,
//! skips itself (terse-testkit). Every run uses temporary configuration
//! and data directories.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

fn workspace() -> PathBuf {
    terse_testkit::workspace().canonicalize().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A command with the user's configuration and data out of reach.
fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    let home = HOME.get_or_init(|| tempfile::tempdir().unwrap()).path();
    let mut c = Command::new(program);
    for (var, dir) in [("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")] {
        c.env(var, home.join(dir));
    }
    c
}

fn run(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Output {
    command(program).args(args).output().unwrap()
}

/// The reference programs, built from the current sources.
fn reference(name: &str) -> PathBuf {
    terse_testkit::programs(&["terse", "terse-bus", "terse-dev-terminal"]);
    terse_testkit::program(name)
}

fn hello() -> String {
    workspace().join("programs/hello/hello.terse").display().to_string()
}

const HOST_ARTICLES: [&str; 8] = [
    "guide-host-go",
    "guide-host-go-frames",
    "guide-host-go-driver",
    "guide-host-go-state",
    "guide-host-rust",
    "guide-host-rust-frames",
    "guide-host-rust-driver",
    "guide-host-rust-state",
];
const DEVICE_ARTICLES: [&str; 2] = ["guide-device-go", "guide-device-rust"];

fn article(name: &str) -> String {
    fs::read_to_string(workspace().join(format!("docs/manual/{name}.txt"))).unwrap()
}

/// The fences of an article: (info string, lines).
fn fences(article: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut lines = article.lines();
    while let Some(line) = lines.next() {
        if let Some(info) = line.strip_prefix("```") {
            let body = lines.by_ref().take_while(|l| l.trim_end() != "```").map(str::to_string).collect();
            out.push((info.trim().to_string(), body));
        }
    }
    out
}

/// The tiny programs the articles make with `xxd -r -p > <name> <<EOF`.
fn tiny_programs(dir: &Path) -> Vec<PathBuf> {
    let mut made = Vec::new();
    for name in ["guide-host-go", "guide-host-go-frames"] {
        for (_, body) in fences(&article(name)) {
            let Some(first) = body.first() else { continue };
            let file = first.strip_prefix("xxd -r -p > ").and_then(|r| r.strip_suffix(" <<EOF"));
            let Some(file) = file else { continue };
            let hex: String = body[1..].iter().take_while(|l| *l != "EOF").map(|l| l.trim()).collect();
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
            let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(byte).collect();
            fs::write(dir.join(file), bytes).unwrap();
            made.push(dir.join(file));
        }
    }
    made
}

#[test]
fn listings_are_the_files_and_fit() {
    let mut listed = 0;
    for name in HOST_ARTICLES.iter().chain(&DEVICE_ARTICLES) {
        for (info, body) in fences(&article(name)) {
            for line in &body {
                assert!(line.chars().count() <= 64, "{name}: a line wider than 64: {line}");
            }
            let words: Vec<&str> = info.split_whitespace().collect();
            let [_, "from", path] = words[..] else { continue };
            let file = fs::read_to_string(workspace().join(path)).unwrap_or_else(|e| panic!("{name}: {path}: {e}"));
            let file: Vec<String> = file.lines().map(|l| l.replace('\t', "    ")).collect();
            let found = file.windows(body.len()).any(|w| w == body.as_slice());
            assert!(found, "{name}: a listing that is not lines of {path}:\n{}", body.join("\n"));
            listed += 1;
        }
    }
    assert!(listed >= 20, "only {listed} listings");
    for dir in ["examples/host/go", "examples/host/rust/src", "examples/device/go", "examples/device/rust/src"] {
        let mut stack = vec![workspace().join(dir)];
        while let Some(d) = stack.pop() {
            for entry in fs::read_dir(&d).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|e| e == "go" || e == "rs") {
                    for (i, line) in fs::read_to_string(&p).unwrap().lines().enumerate() {
                        let width = line.replace('\t', "    ").chars().count();
                        assert!(width <= 64, "{}:{}: {width} columns", p.display(), i + 1);
                    }
                }
            }
        }
    }
}

/// The example programs, built once per test process: `Ok` the directory
/// of the executables, `Err` why they could not be built here (a skip).
struct Built {
    go: Result<PathBuf, String>,
    rust: Result<PathBuf, String>,
}

fn go_build(module: &Path, out: &Path) -> Result<(), String> {
    if !Command::new("go").arg("version").output().is_ok_and(|o| o.status.success()) {
        return Err("go not found".into());
    }
    let o = command("go")
        .args(["build", "-o"])
        .arg(format!("{}/", out.display()))
        .arg("./...")
        .current_dir(module)
        .env("CGO_ENABLED", "0")
        .env("GOFLAGS", "-mod=readonly")
        .output()
        .unwrap();
    let err = text(&o.stderr);
    if o.status.success() {
        Ok(())
    } else if err.contains("dial tcp") || err.contains("lookup") || err.contains("GOPROXY") || err.contains("GOFLAGS") {
        Err(format!("cannot fetch a Go module: {err}"))
    } else {
        panic!("go build in {}: {err}", module.display())
    }
}

fn cargo_build(package: &Path, target: &Path) -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let o = command(cargo)
        .args(["build", "--release", "--offline", "--locked", "--manifest-path"])
        .arg(package.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .output()
        .unwrap();
    let err = text(&o.stderr);
    if o.status.success() {
        Ok(())
    } else if err.contains("offline") || err.contains("download") {
        Err(format!("a crate is not in cargo's cache: {err}"))
    } else {
        panic!("cargo build of {}: {err}", package.display())
    }
}

fn built() -> &'static Built {
    static BUILT: OnceLock<(tempfile::TempDir, Built)> = OnceLock::new();
    &BUILT
        .get_or_init(|| {
            let tmp = tempfile::tempdir().unwrap();
            let examples = workspace().join("examples");
            let go_out = tmp.path().join("go");
            let go = go_build(&examples.join("host/go"), &go_out)
                .and_then(|()| go_build(&examples.join("device/go"), &go_out))
                .map(|()| go_out);
            let target = workspace().join("target/examples");
            let rust = cargo_build(&examples.join("host/rust"), &target)
                .and_then(|()| cargo_build(&examples.join("device/rust"), &target))
                .map(|()| target.join("release"));
            (tmp, Built { go, rust })
        })
        .1
}

fn go() -> Option<&'static Path> {
    built().go.as_deref().map_err(|e| terse_testkit::skip!("go", "{e}")).ok()
}

fn rust() -> Option<&'static Path> {
    built().rust.as_deref().map_err(|e| terse_testkit::skip!("cargo", "{e}")).ok()
}

fn status(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

/// Steps 1 to 4 of one language's host (executables `step1` ... `step4` in
/// `bin`), and their driver `terminal` crosswise with the other's.
fn check_host(bin: &Path, other_terminal: Option<&Path>) {
    let tmp = tempfile::tempdir().unwrap();
    let tiny = tiny_programs(tmp.path());
    assert_eq!(tiny.len(), 3, "the articles make exit.terse, sum.terse and frames.terse");
    let path = |p: &PathBuf| p.display().to_string();
    let step = |n: u32| bin.join(format!("step{n}"));
    let terminal = reference("terse-dev-terminal").display().to_string();

    for (program, expect) in tiny.iter().zip([7, 186, 6]) {
        if expect != 6 {
            assert_eq!(status(&run(step(1), &[&path(program)])), expect, "step 1, {}", path(program));
        }
        assert_eq!(status(&run(step(2), &[&path(program), "10"])), expect, "step 2, {}", path(program));
    }

    let screen = run(reference("terse"), &["run", &hello(), "--frames", "30", "--dump"]);
    assert!(screen.status.success(), "{}", text(&screen.stderr));
    let mut drivers = vec![terminal.clone(), bin.join("terminal").display().to_string()];
    drivers.extend(other_terminal.map(|p| p.display().to_string()));
    for driver in &drivers {
        let o = run(step(3), &[&hello(), "30", driver]);
        assert!(o.status.success(), "step 3 with {driver}: {}", text(&o.stderr));
        assert_eq!(text(&o.stdout), text(&screen.stdout), "step 3's screen with {driver}");
    }

    let faults = ["ebreak", "illegal", "misaligned", "load", "storerom", "fetch", "fetch2", "csr", "div", "hint"];
    let mut programs: Vec<String> = tiny.iter().map(path).collect();
    let fault_program = |f: &&str| workspace().join(format!("native/core-c/tests/{f}.terse")).display().to_string();
    programs.extend(faults.iter().map(fault_program));
    for program in &programs {
        let expected = run(reference("terse"), &["run", program, "--frames", "10", "--hash"]);
        let ours = run(step(4), &[program, "10"]);
        assert_eq!(text(&ours.stdout), text(&expected.stdout), "step 4's hash of {program}: {}", text(&ours.stderr));
    }
    let reference_hash = run(reference("terse"), &["run", &hello(), "--frames", "30", "--hash"]);
    for driver in &drivers {
        let o = run(step(4), &[&hello(), "30", driver]);
        assert_eq!(text(&o.stdout), text(&reference_hash.stdout), "step 4's hash of hello with {driver}");
    }
}

#[test]
fn go_host() {
    let Some(go) = go() else { return };
    check_host(go, rust().map(|r| r.join("terminal")).as_deref());
}

#[test]
fn rust_host() {
    let Some(rust) = rust() else { return };
    check_host(rust, go().map(|g| g.join("terminal")).as_deref());
}

/// Each driver under the reference core gives the reference terminal's
/// hash after every frame, and the articles state that hash.
#[test]
fn drivers_under_terse_bus() {
    let hashes = |driver: Option<&Path>| {
        let mut args = vec![hello(), "--frames".into(), "30".into(), "--hash-every".into(), "1".into()];
        if let Some(d) = driver {
            args.push("--driver".into());
            args.push(format!("2={}", d.display()));
        }
        let o = command(reference("terse-bus")).args(&args).output().unwrap();
        assert!(o.status.success(), "{}", text(&o.stderr));
        text(&o.stdout)
    };
    let expected = hashes(None);
    let last = expected.lines().last().unwrap().split_whitespace().nth(1).unwrap().to_string();
    for name in DEVICE_ARTICLES.iter().chain(["guide-host-go-state", "guide-host-rust-state"].iter()) {
        assert!(article(name).contains(&last), "{name} does not show hello's hash {last}");
    }
    for bin in [go(), rust()].into_iter().flatten() {
        assert_eq!(hashes(Some(&bin.join("terminal"))), expected, "{}", bin.join("terminal").display());
    }
}

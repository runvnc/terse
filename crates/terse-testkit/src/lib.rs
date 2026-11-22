//! What the workspace's tests share.
//!
//! **Programs built from the sources being tested.** A test that runs a
//! program of another package (terse-devices' tests run `terse-bus` and
//! `terse`, terse-mame-crt's run `terse-bus` and the drivers) cannot count
//! on `cargo test` to have built it: cargo builds the binaries of the
//! packages whose tests it builds, and only those that have integration
//! tests, so a program found by path may be one built before the last change
//! of its sources, or before a merge. [`program`] asks cargo to build it
//! first (`cargo build --workspace --bin <name>`, with the test's profile,
//! once per test process) and returns the path cargo reports. When the
//! program is up to date that build does nothing; when it is not, it is
//! rebuilt and a line says so; when it cannot be built, the test fails.
//! Cargo itself serialises such builds with any other build of the same
//! target directory; it does not hold that lock while tests run.
//!
//! **Skips that are seen.** A test that cannot run here (no GPU, no Go, no
//! C compiler, ...) calls [`skip!`] with the thing it needs and why, and
//! returns. The line goes straight to the standard error, past the test
//! harness's capture, and is added to the skip log (`target/test-skips.log`,
//! or the file `TERSE_TEST_SKIPS` names), which `scripts/test-skips` prints
//! at the end of a `scripts/cargo test` run. With `TERSE_TEST_NO_SKIP=1` a
//! skip is a failure instead, except for the needs listed (comma-separated)
//! in `TERSE_TEST_ALLOW_SKIP`: for runs meant to cover everything.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// The workspace's root directory.
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The program `name` (`terse`, `terse-bus`, `terse-dev-vector`, ...) of
/// this workspace, built from the current sources; see the crate's
/// documentation. Panics if it cannot be built.
pub fn program(name: &str) -> PathBuf {
    programs(&[name]).remove(0)
}

/// [`program`] for several, built with one cargo command.
pub fn programs(names: &[&str]) -> Vec<PathBuf> {
    static BUILT: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    let wanted: Vec<&str> = names.iter().copied().filter(|n| !built.contains_key(*n)).collect();
    if !wanted.is_empty() {
        let cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        match build(cargo, &workspace().join("Cargo.toml"), profile().as_deref(), &wanted) {
            Ok(Built { programs, rebuilt }) => {
                if !rebuilt.is_empty() {
                    say(&format!("terse-testkit: built {} (older than its sources, or not built)", rebuilt.join(", ")));
                }
                built.extend(programs);
            }
            Err(e) => panic!("{e}"),
        }
    }
    names.iter().map(|n| built[*n].clone()).collect()
}

/// The directory of a program the tests build, so that a test can use the
/// programs beside it (terse-bus starts the drivers in its own directory).
pub fn program_dir(name: &str) -> PathBuf {
    program(name).parent().unwrap().to_path_buf()
}

/// What [`build`] did.
#[derive(Debug)]
pub struct Built {
    /// Each program asked for, and its path.
    pub programs: Vec<(String, PathBuf)>,
    /// The programs that were compiled (not already up to date).
    pub rebuilt: Vec<String>,
}

/// Builds the binaries `names` of the workspace `manifest` with `cmd` (the
/// cargo program, with any environment it should have; `profile`: cargo's
/// `--profile`, `None` for its default) and returns where they are, as
/// cargo reports it. Public for the test of this mechanism.
pub fn build(mut cmd: Command, manifest: &Path, profile: Option<&str>, names: &[&str]) -> Result<Built, String> {
    // --locked: the test's own build has already resolved Cargo.lock.
    cmd.args(["build", "--locked", "--workspace", "--message-format=json-render-diagnostics", "--manifest-path"]);
    cmd.arg(manifest);
    if let Some(p) = profile {
        cmd.args(["--profile", p]);
    }
    for n in names {
        cmd.args(["--bin", n]);
    }
    let dir = manifest.parent().unwrap_or(Path::new("."));
    cmd.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().map_err(|e| format!("terse-testkit: cannot run {cmd:?} to build {names:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "terse-testkit: building {} for this test failed ({cmd:?}):\n{}",
            names.join(", "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut built = Built { programs: Vec::new(), rebuilt: Vec::new() };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if !line.starts_with("{\"reason\":\"compiler-artifact\"") {
            continue;
        }
        let Some(exe) = json_string(line, "executable") else { continue };
        let exe = PathBuf::from(exe);
        let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) else { continue };
        if names.contains(&stem) && !built.programs.iter().any(|(n, _)| n == stem) {
            if line.contains("\"fresh\":false") {
                built.rebuilt.push(stem.to_string());
            }
            built.programs.push((stem.to_string(), exe));
        }
    }
    let missing: Vec<&&str> = names.iter().filter(|n| !built.programs.iter().any(|(b, _)| b == *n)).collect();
    if !missing.is_empty() {
        return Err(format!("terse-testkit: cargo built no program {missing:?} ({cmd:?})"));
    }
    Ok(built)
}

/// The cargo profile this test was built with, from where cargo put it:
/// `<target>/<profile directory>/...` (`debug` is the `dev` profile; with
/// `--target`, `<target>/<triple>/<profile directory>/...`). `None` when it
/// cannot be told, for cargo's default.
fn profile() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let root = exe.ancestors().find(|a| a.join("CACHEDIR.TAG").is_file())?;
    let mut below = exe.strip_prefix(root).ok()?.components().map(|c| c.as_os_str().to_str().unwrap_or(""));
    let first = below.next()?;
    // A target triple has a directory for each profile under it.
    let dir = if first.matches('-').count() < 2 { first } else { below.next()? };
    Some(if dir == "debug" { "dev" } else { dir }.to_string())
}

/// The string value of `"key":"..."` in one line of JSON (cargo's messages
/// have no spaces around the colon).
fn json_string(line: &str, key: &str) -> Option<String> {
    let start = line.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut out = String::new();
    let mut chars = line[start..].chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
}

/// Writes a line to the standard error directly: the test harness captures
/// `eprintln!` and shows it only for a failed test.
fn say(line: &str) {
    let _ = std::io::stderr().write_all(format!("{line}\n").as_bytes());
}

/// Records that the calling test does not run here, or not all of it: `need`
/// is what it lacks, in one word (`gpu`, `go`, `cc`, `clang`, `fonts`, ...),
/// the rest why. Returns; the test then returns or carries on without that
/// part. With `TERSE_TEST_NO_SKIP` set, panics instead (unless `need` is
/// listed in `TERSE_TEST_ALLOW_SKIP`).
#[macro_export]
macro_rules! skip {
    ($need:expr, $($why:tt)+) => {
        $crate::skipped(env!("CARGO_PKG_NAME"), module_path!(), $need, &format!($($why)+))
    };
}

/// [`skip!`], with where it was called from given.
pub fn skipped(package: &str, module: &str, need: &str, why: &str) {
    let test = test_name(module);
    let env = |v: &str| std::env::var(v).ok();
    if refuses(env("TERSE_TEST_NO_SKIP").as_deref(), env("TERSE_TEST_ALLOW_SKIP").as_deref(), need) {
        panic!("not skipped, as TERSE_TEST_NO_SKIP asks: {test} needs {need}: {why}");
    }
    say(&format!("SKIPPED {package} {test} [{need}]: {why}"));
    let log = env("TERSE_TEST_SKIPS").map(PathBuf::from).unwrap_or_else(skip_log);
    record(&log, package, &test, need, why);
}

/// Whether a skip for `need` is refused, given the values of
/// `TERSE_TEST_NO_SKIP` and `TERSE_TEST_ALLOW_SKIP`.
pub fn refuses(no_skip: Option<&str>, allow: Option<&str>, need: &str) -> bool {
    let on = no_skip.is_some_and(|v| !v.is_empty() && v != "0");
    on && !allow.unwrap_or("").split(',').any(|a| a.trim() == need)
}

/// The default skip log: `test-skips.log` in the target directory
/// (`CARGO_TARGET_DIR`, else the workspace's `target`).
pub fn skip_log() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("target"));
    let target = if target.is_absolute() { target } else { workspace().join(target) };
    target.join("test-skips.log")
}

/// Appends one skip to `log`: package, test, need and reason, tab-separated
/// (tabs and newlines in them become spaces).
pub fn record(log: &Path, package: &str, test: &str, need: &str, why: &str) {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let line = format!("{}\t{}\t{}\t{}\n", clean(package), clean(test), clean(need), clean(why));
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // One write of a short line in append mode: whole lines, even from
    // several test processes at once.
    let file = std::fs::OpenOptions::new().create(true).append(true).open(log);
    if let Ok(mut f) = file {
        let _ = f.write_all(line.as_bytes());
    }
}

/// `<test binary>::<test>`: the harness names each test's thread after the
/// test; on another thread, the calling module.
fn test_name(module: &str) -> String {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .map(|s| s.rsplit_once('-').map_or(s.clone(), |(b, _)| b.to_string()))
        .unwrap_or_default();
    let thread = std::thread::current();
    let test = thread.name().filter(|n| *n != "main").unwrap_or(module);
    format!("{binary}::{test}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusing_skips() {
        assert!(!refuses(None, None, "gpu"));
        assert!(!refuses(Some(""), None, "gpu"));
        assert!(!refuses(Some("0"), None, "gpu"));
        assert!(refuses(Some("1"), None, "gpu"));
        assert!(refuses(Some("1"), Some("go"), "gpu"));
        assert!(!refuses(Some("1"), Some("go, gpu"), "gpu"));
    }

    #[test]
    fn json_strings() {
        let line = r#"{"reason":"compiler-artifact","executable":"C:\\t\"x\"\u00e9\\terse-bus.exe","fresh":true}"#;
        assert_eq!(json_string(line, "executable").as_deref(), Some("C:\\t\"x\"\u{e9}\\terse-bus.exe"));
        assert_eq!(json_string(r#"{"executable":null}"#, "executable"), None);
    }

    #[test]
    fn records_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("sub/skips.log");
        record(&log, "p", "t::a", "gpu", "no\tadapter\nhere");
        record(&log, "p", "t::b", "go", "no go");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "p\tt::a\tgpu\tno adapter here\np\tt::b\tgo\tno go\n");
    }

    #[test]
    fn this_tests_profile() {
        // The unit tests are built with the dev (test) profile, or with
        // `--release`.
        let p = profile();
        assert!(matches!(p.as_deref(), Some("dev" | "release")), "{p:?}");
    }
}

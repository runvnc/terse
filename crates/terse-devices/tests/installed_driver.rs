//! Installed drivers (`DEVICES.md` 2.2): `terse drivers` puts a driver in
//! the user's drivers folder, and `terse-bus` uses it from then on with no
//! `--driver` option: the same state hashes as `terse run`, its identity
//! in the performance record. A `--driver` comes first; `use <slot>
//! default` and `remove` go back to the shipped driver; an installed
//! driver that is missing, does not start, does not answer in time,
//! answers ERROR, answers for other slots or with another device version
//! gives way to the shipped one with a message; one that dies during the
//! run stops it (status 3), with no fallback. Every directory is a
//! temporary one: the user's own configuration is never read.
//!
//! Unix only: the planted drivers are shell scripts and modes.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use terse_perflog::Component;
use terse_perflog::record::{self, Line};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn bin(name: &str) -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_terse-dev-terminal")).parent().unwrap().join(name)
}

fn program(name: &str) -> String {
    workspace().join(format!("programs/{name}/{name}.terse")).display().to_string()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A temporary home: config, store, data.
struct Home {
    tmp: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Home { tmp: tempfile::tempdir().unwrap() }
    }

    fn path(&self, p: &str) -> String {
        self.tmp.path().join(p).display().to_string()
    }

    fn config(&self) -> String {
        self.path("config")
    }

    /// `terse drivers <args> --config <config>`.
    fn drivers(&self, args: &[&str]) -> Output {
        let config = self.config();
        Command::new(bin("terse")).arg("drivers").args(args).args(["--config", &config]).env_remove("TERSE_CRASH").output().unwrap()
    }

    /// `terse-bus <args>` with this home's directories and no network.
    fn bus(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = Command::new(bin("terse-bus"));
        c.args(args).args(["--config", &self.config(), "--store", &self.path("store"), "--data", &self.path("data"), "--relay", "none"]);
        c.env_remove("TERSE_CRASH").env_remove("TERSE_EXAMPLE_SLOW").env_remove("TERSE_BUS_CPU");
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    /// `terse run <args>` with this home's directories.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(bin("terse")).arg("run").args(args).args(["--config", &self.config(), "--store", &self.path("store"), "--data", &self.path("data")]).output().unwrap()
    }

    /// The example C driver, built here (`None`: no C compiler).
    fn example(&self) -> Option<String> {
        let out = self.tmp.path().join("terminal");
        let ok = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
            .args(["-std=c11", "-O2", "-o"])
            .arg(&out)
            .arg(workspace().join("native/driver-example/terminal.c"))
            .status()
            .is_ok_and(|s| s.success());
        ok.then(|| out.display().to_string())
    }

    fn use_file(&self) -> PathBuf {
        self.tmp.path().join("config/drivers/use")
    }

    fn installed(&self, name: &str) -> PathBuf {
        self.tmp.path().join("config/drivers").join(name)
    }

    /// Writes a driver into the folder by hand: `program` copied (or, if
    /// it starts with `#!`, written as a script) and the manifest's slots.
    fn plant(&self, name: &str, program: &str, slots: &[(u8, u8)]) {
        let dir = self.installed(name);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("prog");
        if program.starts_with("#!") {
            std::fs::write(&p, program).unwrap();
        } else {
            std::fs::copy(program, &p).unwrap();
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut m = String::from("terse-driver 1\nprogram prog\n");
        for (s, v) in slots {
            m.push_str(&format!("slot {s} {v}\n"));
        }
        m.push_str("name planted\nversion -\nbuild -\n");
        std::fs::write(dir.join("manifest"), m).unwrap();
    }
}

/// terse, terse-bus and a C compiler, or a visible line saying what is missing.
fn ready() -> Option<(Home, String)> {
    for b in ["terse", "terse-bus"] {
        if !bin(b).exists() {
            eprintln!("skipped: {b} is not built (scripts/cargo build -p terse-cli -p terse-bus)");
            return None;
        }
    }
    let home = Home::new();
    match home.example() {
        Some(x) => Some((home, x)),
        None => {
            eprintln!("skipped: no C compiler for native/driver-example");
            None
        }
    }
}

/// The driver names `--bench` reports, by lowest slot.
fn driver_names(out: &Output) -> Vec<String> {
    text(&out.stderr).lines().filter(|l| l.starts_with("bus: slots") && l.contains(", transport ")).map(str::to_string).collect()
}

fn terminal_line(out: &Output) -> String {
    driver_names(out).into_iter().find(|l| l.starts_with("bus: slots [2")).unwrap_or_default()
}

#[test]
fn an_installed_driver_is_used_with_no_option_and_named_in_the_record() {
    let Some((home, x)) = ready() else { return };
    let add = home.drivers(&["add", &x]);
    let out = text(&add.stdout);
    assert!(add.status.success(), "{}{}", out, text(&add.stderr));
    for want in ["name     terminal (example)", "version  0.1.0", "build    fnv:", "slots    2 (version 0), 10 (version 0)", "full rights", "Installed terminal-example"] {
        assert!(out.contains(want), "{want}: {out}");
    }
    let build = out.lines().find_map(|l| l.strip_prefix("  build    ")).unwrap().to_string();
    let manifest = std::fs::read_to_string(home.installed("terminal-example").join("manifest")).unwrap();
    assert!(manifest.starts_with("terse-driver 1\nprogram terminal\nslot 2 0\nslot 10 0\nname terminal (example)\n"), "{manifest}");
    let list = text(&home.drivers(&["list"]).stdout);
    assert!(list.contains("terminal-example (installed: terminal (example) 0.1.0 fnv:") && list.contains("terse-dev-vector (shipped)"), "{list}");

    // A program through terse-bus without any --driver: terse run's hashes,
    // the installed driver's identity in the record.
    let (conform, events) = (program("conform-program"), workspace().join("programs/conform-program/conform-program.events").display().to_string());
    for args in [vec![program("hello").as_str(), "--frames", "30", "--hash-every", "1"], vec![&conform, "--events", &events, "--frames", "200", "--hash-every", "10"]] {
        let perf = home.path("perf");
        let _ = std::fs::remove_dir_all(&perf);
        let mut bus_args = args.clone();
        bus_args.extend(["--bench", "--perf-log", &perf]);
        let on_bus = home.bus(&bus_args, &[]);
        assert!(on_bus.status.success(), "{}", text(&on_bus.stderr));
        assert!(terminal_line(&on_bus).contains("terminal (example)") && terminal_line(&on_bus).contains("config/drivers/terminal-example/terminal"), "{:?}", driver_names(&on_bus));
        let reference = home.run(&args);
        assert!(reference.status.success() && !reference.stdout.is_empty());
        assert_eq!(text(&on_bus.stdout), text(&reference.stdout), "{args:?}");
        let mut lines = Vec::new();
        for e in std::fs::read_dir(&perf).unwrap().flatten() {
            lines.extend(record::parse(&std::fs::read_to_string(e.path()).unwrap()));
        }
        assert!(
            lines.iter().any(|l| matches!(l, Line::Device { component: Component::Device(2), identity, .. }
                if identity.name == "terminal (example)" && identity.version == "0.1.0" && identity.build == build)),
            "{lines:?}"
        );
    }
}

#[test]
fn command_line_then_installed_then_shipped() {
    let Some((home, x)) = ready() else { return };
    assert!(home.drivers(&["add", &x]).status.success());
    let hello = program("hello");
    let want = text(&home.run(&[&hello, "--frames", "30", "--hash"]).stdout);
    let run = |extra: &[&str]| {
        let mut a = vec![hello.as_str(), "--frames", "30", "--hash", "--bench"];
        a.extend(extra);
        let out = home.bus(&a, &[]);
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), want);
        terminal_line(&out)
    };
    assert!(run(&[]).contains("terminal (example)"));
    // A --driver comes first.
    let rust = format!("2={}", bin("terse-dev-terminal").display());
    let line = run(&["--driver", &rust]);
    assert!(line.contains("terse-dev-terminal") && !line.contains("example"), "{line}");
    // use <slot> default: the shipped driver, though still installed.
    let out = home.drivers(&["use", "10", "default"]);
    assert!(out.status.success() && text(&out.stdout).contains("the shipped driver"), "{}", text(&out.stderr));
    assert!(std::fs::read_to_string(home.use_file()).unwrap().contains("\n2 default\n"));
    let line = run(&[]);
    assert!(line.contains("terse-dev-terminal") && !line.contains("example"), "{line}");
    let list = text(&home.drivers(&["list"]).stdout);
    assert!(list.contains("chosen with terse drivers use 2 default") && list.contains("not used: terminal-example"), "{list}");
    // ... and back.
    assert!(home.drivers(&["use", "2", "terminal-example"]).status.success());
    assert!(run(&[]).contains("terminal (example)"));
    // A driver is chosen only for its own slots.
    let wrong = home.drivers(&["use", "3", "terminal-example"]);
    assert!(!wrong.status.success() && text(&wrong.stderr).contains("serves slots 2 and 10"), "{}", text(&wrong.stderr));
    // remove: the shipped driver again, nothing left behind.
    let out = home.drivers(&["remove", "terminal-example"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!home.installed("terminal-example").exists());
    assert!(!std::fs::read_to_string(home.use_file()).unwrap().contains("terminal-example"));
    let line = run(&[]);
    assert!(line.contains("terse-dev-terminal") && !line.contains("example"), "{line}");
    assert!(!home.drivers(&["remove", "terminal-example"]).status.success());
}

#[test]
fn add_refuses_what_does_not_answer_as_a_driver() {
    let Some((home, _)) = ready() else { return };
    let script = home.path("not-a-driver");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = home.drivers(&["add", &script]);
    assert!(!out.status.success() && text(&out.stderr).contains("did not answer as a driver"), "{}", text(&out.stderr));
    assert!(!Path::new(&home.config()).join("drivers/use").exists());
    // The vector driver, with --slot and a name of the user's.
    let vector = bin("terse-dev-vector").display().to_string();
    let out = home.drivers(&["add", &vector, "--slot", "9", "--name", "my-vector"]);
    assert!(out.status.success() && text(&out.stdout).contains("slots    3 (version 0), 9 (version 0)"), "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(std::fs::read_to_string(home.use_file()).unwrap().contains("\n3 my-vector\n"));
    // Asked only for slot 2, it answers for nothing.
    assert!(!home.drivers(&["add", &vector, "--slot", "2"]).status.success());
}

/// Each way an installed driver fails before the run: the shipped driver,
/// the same hash, and a message naming the driver and the way back.
#[test]
fn an_installed_driver_that_cannot_be_used_gives_way_to_the_shipped_one() {
    let Some((home, x)) = ready() else { return };
    let hello = program("hello");
    let holo = program("holo");
    let vector = bin("terse-dev-vector").display().to_string();
    let cases: [(&str, &str, Vec<(u8, u8)>, &str, &str); 6] = [
        ("missing", "", vec![(2, 0), (10, 0)], "is missing", &hello),
        ("exits", "#!/bin/sh\nexit 1\n", vec![(2, 0), (10, 0)], "before it connected", &hello),
        ("hangs", "#!/bin/sh\nexec sleep 30\n", vec![(2, 0), (10, 0)], "", &hello),
        ("errs", &x, vec![(3, 0), (9, 0)], "this driver serves slot 2", &holo),
        ("other-slots", &vector, vec![(2, 0), (10, 0)], "answered for slots", &hello),
        ("other-version", &x, vec![(2, 1), (10, 0)], "answered for device 2 version 0, installed as version 1", &hello),
    ];
    for (name, prog, slots, why, image) in cases {
        home.plant(name, if prog.is_empty() { &x } else { prog }, &slots);
        let slot = slots[0].0.to_string();
        assert!(home.drivers(&["use", &slot, name]).status.success(), "{name}");
        if prog.is_empty() {
            std::fs::remove_file(home.installed(name).join("prog")).unwrap();
        }
        let args = [image, "--frames", "20", "--hash", "--bench", "--start-timeout", "1"];
        let out = home.bus(&args, &[]);
        let err = text(&out.stderr);
        assert!(out.status.success(), "{name}: {err}");
        assert!(err.contains(&format!("the installed driver {name} for slot {slot} was not used")) && err.contains(why), "{name}: {err}");
        assert!(err.contains(&format!("(terse drivers use {slot} default)")), "{name}: {err}");
        assert!(!driver_names(&out).iter().any(|l| l.contains("/drivers/")), "{name}: {:?}", driver_names(&out));
        assert_eq!(text(&out.stdout), text(&home.run(&[image, "--frames", "20", "--hash"]).stdout), "{name}");
        assert!(home.drivers(&["use", &slot, "default"]).status.success());
    }
}

/// After the handshake an installed driver is like any other: one that
/// dies during the run stops it (DEVICES.md 10), with no fallback.
#[test]
fn an_installed_driver_that_dies_during_the_run_stops_it() {
    let Some((home, _)) = ready() else { return };
    let crash = bin("terse-dev-crash").display().to_string();
    let add = home.drivers(&["add", &crash]);
    assert!(add.status.success(), "{}{}", text(&add.stdout), text(&add.stderr));
    let hello = program("hello");
    let out = home.bus(&[&hello, "--frames", "30", "--hash"], &[("TERSE_CRASH", "exit"), ("TERSE_CRASH_AT", "5")]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("drivers/terse-dev-crash") && !err.contains("was not used"), "{err}");
}

//! A twin of a test's system on the device bus (DEVICES.md). These test
//! files are also compiled into terse-devices' tests (`sessions.rs`,
//! `sessions_c_socket.rs`), which have the driver programs: there every
//! session runs a second time, with its devices as driver processes, in
//! step with the first, and after every frame the state hash of the shell
//! and of every machine it started must be the one the devices compiled
//! in give. Compiled into terse-cli's own tests there are no drivers, and
//! no twin.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use terse_bus::bus::{BusConfig, split_command};
use terse_bus::host::{self, BusSystem, Start};
use terse_bus::inproc::ContentOptions;
use terse_core::{Image, ProgramId, Script, System};
use terse_net::Consent;

/// What the test's own system was given.
pub struct Setup<'a> {
    pub image: &'a Image,
    /// Run as the shell, with these built-in programs.
    pub shell: Option<Vec<[u8; 32]>>,
    pub identity: Option<ProgramId>,
    /// The test's store and data directory, copied as they are now.
    pub store: Option<&'a Path>,
    pub data: Option<&'a Path>,
    /// The node's subscriptions and identity (a seed), as the test set
    /// them.
    pub subscriptions: Vec<[u8; 32]>,
    pub seed: Option<[u8; 32]>,
    pub consent: Consent,
    /// Relays the node follows.
    pub relays: Vec<String>,
    pub script: &'a str,
    /// Whether the test's system has a content service at all.
    pub content: bool,
    /// The test's node published into a feed it follows before its
    /// content service started: the service starts with that entry's feed
    /// event, which terse-net's headless host announces in frame 1 (a
    /// fresh node has none, so the twin is given it then).
    pub announced: bool,
}

/// The driver programs, when compiled into terse-devices' tests.
fn drivers_dir() -> Option<PathBuf> {
    let t = option_env!("CARGO_BIN_EXE_terse-dev-terminal")?;
    Some(Path::new(t).parent()?.to_path_buf())
}

/// This test binary's variant: the C terminal driver on the socket, or the
/// Rust drivers on shared memory.
fn c_socket() -> bool {
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().starts_with("sessions_c_socket"))).unwrap_or(false)
}

/// The C terminal driver, built once.
fn c_terminal() -> Option<String> {
    static C: OnceLock<Option<String>> = OnceLock::new();
    C.get_or_init(|| {
        // One per copy of this module (each test module has its own).
        let dir = Path::new(option_env!("CARGO_TARGET_TMPDIR").unwrap_or("/tmp")).join(format!("terse-twin-c-{}-{:p}", std::process::id(), &C));
        std::fs::create_dir_all(&dir).ok()?;
        let out = dir.join("terse-dev-terminal-c");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("c/terminal.c");
        let ok = std::process::Command::new("clang")
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-o"])
            .arg(&out)
            .arg(&src)
            .status()
            .is_ok_and(|s| s.success());
        ok.then(|| out.display().to_string())
    })
    .clone()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The test's system run again on the bus.
pub struct Twin {
    // Boxed: the tests move their systems about on small test stacks.
    bs: Box<BusSystem>,
    script: Script,
    announced: bool,
    _dirs: tempfile::TempDir,
}

impl Twin {
    /// The twin, if this build has the drivers.
    pub fn start(s: Setup) -> Option<Twin> {
        Twin::start_with_grants(s, None)
    }

    /// The same with the machine's grants (SPEC §image) given, as the test's
    /// machine has them; `None`: every optional device a driver gives.
    pub fn start_with_grants(s: Setup, grants: Option<terse_core::Grants>) -> Option<Twin> {
        let dir = drivers_dir()?;
        let dirs = tempfile::tempdir().unwrap();
        let (store, config, data) = (dirs.path().join("store"), dirs.path().join("config"), dirs.path().join("data"));
        for d in [&store, &config, &data] {
            std::fs::create_dir_all(d).unwrap();
        }
        if let Some(p) = s.store {
            copy_dir(p, &store);
        }
        if let Some(p) = s.data {
            copy_dir(p, &data);
        }
        let subs: String = s.subscriptions.iter().map(|k| format!("{}\n", hex(k))).collect();
        std::fs::write(config.join("subscriptions"), subs).unwrap();
        if let Some(seed) = s.seed {
            std::fs::write(config.join("identity.key"), hex(&seed)).unwrap();
        }
        let content = ContentOptions {
            store: Some(store),
            config: Some(config),
            data: Some(data),
            consent: Some(s.consent),
            relays: s.relays,
            ..Default::default()
        };
        let mut overrides = Vec::new();
        let socket = c_socket();
        if socket {
            let c = c_terminal().expect("the C terminal driver needs clang");
            overrides.push((vec![2], split_command(&c)));
        }
        let mut slots = host::needed_slots(s.image, s.shell.is_some());
        if !s.content {
            slots.retain(|&x| x != 4);
        }
        let drivers = host::drivers(&dir, &slots, &content.args(), &overrides).unwrap();
        let mut config = BusConfig::new(PathBuf::new());
        config.shm = !socket;
        let start = Start { config, drivers, image: s.image, shell: s.shell, identity: s.identity, snapshot: None, grants, without: Vec::new() };
        let bs = start.start().unwrap_or_else(|e| panic!("the bus twin: {e}"));
        if std::env::var_os("TERSE_TWIN_KEEP").is_some() {
            let kept = dirs.keep();
            eprintln!("the twin's directories: {}", kept.display());
            return Some(Twin { bs: Box::new(bs), script: Script::parse(s.script).unwrap(), announced: s.announced, _dirs: tempfile::tempdir().unwrap() });
        }
        Some(Twin { bs: Box::new(bs), script: Script::parse(s.script).unwrap(), announced: s.announced, _dirs: dirs })
    }

    /// Runs the twin's next frame with the script's input.
    pub fn step(&mut self) {
        let frame = self.bs.system.shell().frame();
        let mut events = self.script.events(frame).to_vec();
        if frame == 1 && self.announced {
            events.push(terse_core::Event::feeds_updated());
        }
        let controllers = self.script.controllers(frame);
        self.bs.frame(&events, controllers).unwrap_or_else(|e| panic!("the bus twin, frame {frame}: {e}"));
    }

    /// Every machine's state hash equals the test's system's.
    pub fn check(&mut self, system: &System) {
        let frame = system.shell().frame();
        let (shell, children) = self.bs.hashes().unwrap_or_else(|e| panic!("the bus twin: {e}"));
        if shell != system.shell().state_hash() {
            let (a, b) = (system.shell().state(), self.bs.system.shell_mut().try_state().unwrap());
            let at = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
            eprintln!(
                "the twin's shell differs after frame {frame}: lengths {} and {}, first at byte {at} (RAM offset {}): {:02x?} vs {:02x?}",
                a.len(),
                b.len(),
                at as i64 - 200,
                &a[at..(at + 16).min(a.len())],
                &b[at..(at + 16).min(b.len())]
            );
        }
        assert_eq!(hex(&shell), hex(&system.shell().state_hash()), "the shell's hash on the bus after frame {frame}");
        let want: Vec<(u8, Option<String>)> = system.children().map(|(h, c)| (h, c.machine.as_ref().map(|m| hex(&m.state_hash())))).collect();
        let got: Vec<(u8, Option<String>)> = children.into_iter().map(|(h, x)| (h, x.map(|x| hex(&x)))).collect();
        assert_eq!(got, want, "the machines' hashes on the bus after frame {frame}");
    }

    /// Runs the next frame and checks it.
    pub fn follow(&mut self, system: &System) {
        self.step();
        self.check(system);
    }

    /// The same for a single machine run without a system.
    pub fn follow_machine(&mut self, m: &terse_core::Machine) {
        self.step();
        let frame = m.frame();
        let got = self.bs.system.shell_mut().try_state_hash().unwrap_or_else(|e| panic!("the bus twin: {e}"));
        assert_eq!(hex(&got), hex(&m.state_hash()), "the machine's hash on the bus after frame {frame}");
    }

    /// Where the eye is (SPEC §vector3d.space), for the pointer over Surfaces.
    pub fn set_eye(&mut self, eye: (f32, f32)) {
        self.bs.system.set_eye(eye);
    }
}

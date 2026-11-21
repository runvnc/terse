//! The user's drivers folder (`DEVICES.md` 2.2): `<config>/drivers`, which
//! `terse drivers` writes and `terse-bus` reads once, when it starts.
//!
//! ```text
//! <config>/drivers/use                which installed driver serves which group
//! <config>/drivers/<name>/manifest    what the driver said it is when installed
//! <config>/drivers/<name>/<program>   a copy of its program file
//! ```
//!
//! This module is the format only: no process is started here.

use std::path::{Path, PathBuf};

/// The folder in the config directory.
pub const DRIVERS_DIR: &str = "drivers";
/// The choices file in the folder.
pub const USE_FILE: &str = "use";
/// The manifest in a driver's directory.
pub const MANIFEST: &str = "manifest";

/// The groups of slots a driver serves (`DEVICES.md` 2.1's table): the
/// terminal with full text, the vector displays, content, sound, the page.
pub const GROUPS: [&[u8]; 5] = [&[2, 10], &[3, 9], &[4], &[6], &[7]];

/// The group `slot` belongs to.
pub fn group_of(slot: u8) -> Option<&'static [u8]> {
    GROUPS.iter().copied().find(|g| g.contains(&slot))
}

/// The shipped driver's program for a group (`DEVICES.md` 2.1).
pub fn shipped_program(group: &[u8]) -> &'static str {
    match group.first() {
        Some(2) => "terse-dev-terminal",
        Some(3) => "terse-dev-vector",
        Some(4) => "terse-dev-content",
        Some(6) => "terse-dev-sound",
        _ => "terse-dev-page",
    }
}

/// The folder for `config`.
pub fn folder(config: &Path) -> PathBuf {
    config.join(DRIVERS_DIR)
}

/// Whether `name` may name an installed driver: 1 to 64 of `a`-`z`,
/// `0`-`9`, `-` and `.`, starting with a letter or digit, not `use`.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name != USE_FILE
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && name.as_bytes()[0].is_ascii_alphanumeric()
}

/// A name made from what a driver calls itself: lowercase, every run of
/// other characters as one `-`, trimmed; `driver` if nothing is left.
pub fn name_from(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_matches(|c| c == '-' || c == '.').chars().take(64).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if valid_name(&out) { out } else { "driver".into() }
}

/// An installed driver's manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The program's file name in the driver's directory.
    pub program: String,
    /// The slots it answered for and the device version of each.
    pub slots: Vec<(u8, u8)>,
    /// Its identity as it gave it (`PERFORMANCE.md` R13); `-` for none.
    pub name: String,
    pub version: String,
    pub build: String,
}

fn field(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

impl Manifest {
    pub fn encode(&self) -> String {
        let mut s = format!("terse-driver 1\nprogram {}\n", self.program);
        for (slot, v) in &self.slots {
            s.push_str(&format!("slot {slot} {v}\n"));
        }
        s.push_str(&format!("name {}\nversion {}\nbuild {}\n", field(&self.name), field(&self.version), field(&self.build)));
        s
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let mut lines = text.lines();
        if lines.next().map(str::trim_end) != Some("terse-driver 1") {
            return Err("not a driver manifest (terse-driver 1)".into());
        }
        let mut m = Manifest { program: String::new(), slots: Vec::new(), name: String::new(), version: String::new(), build: String::new() };
        for line in lines {
            let line = line.trim_end();
            let (word, value) = line.split_once(' ').unwrap_or((line, ""));
            match word {
                "program" => m.program = value.to_string(),
                "slot" => {
                    let (s, v) = value.split_once(' ').ok_or(format!("bad line: {line}"))?;
                    m.slots.push((s.parse().map_err(|_| format!("bad line: {line}"))?, v.parse().map_err(|_| format!("bad line: {line}"))?));
                }
                "name" => m.name = value.to_string(),
                "version" => m.version = value.to_string(),
                "build" => m.build = value.to_string(),
                _ => {}
            }
        }
        if m.program.is_empty() || m.program.contains('/') || m.program == "." || m.program == ".." {
            return Err("no program file named".into());
        }
        let group = m.slots.first().and_then(|s| group_of(s.0)).ok_or("no slot named")?;
        if m.slots.iter().any(|s| !group.contains(&s.0)) {
            return Err("slots of more than one group".into());
        }
        Ok(m)
    }

    /// The group it serves.
    pub fn group(&self) -> &'static [u8] {
        self.slots.first().and_then(|s| group_of(s.0)).unwrap_or(&[])
    }

    /// Its identity in one line: name, version and build.
    pub fn identity(&self) -> String {
        format!("{} {} {}", self.name, field(&self.version), field(&self.build))
    }
}

/// One installed driver: its directory's name and its manifest.
#[derive(Clone, Debug)]
pub struct Installed {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
}

impl Installed {
    /// The program to start, if its manifest is good and the file is there.
    pub fn program(&self) -> Result<PathBuf, String> {
        let m = self.manifest.as_ref().map_err(|e| format!("its manifest: {e}"))?;
        let p = self.dir.join(&m.program);
        if p.is_file() { Ok(p) } else { Err(format!("its program {} is missing", p.display())) }
    }
}

/// Reads one installed driver (it need not exist).
pub fn installed(config: &Path, name: &str) -> Installed {
    let dir = folder(config).join(name);
    let manifest = match std::fs::read_to_string(dir.join(MANIFEST)) {
        Ok(t) => Manifest::parse(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(if dir.is_dir() { "no manifest".into() } else { "not installed".into() }),
        Err(e) => Err(e.to_string()),
    };
    Installed { name: name.to_string(), dir, manifest }
}

/// Every installed driver, by name.
pub fn list(config: &Path) -> Vec<Installed> {
    let Ok(rd) = std::fs::read_dir(folder(config)) else { return Vec::new() };
    let mut names: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| valid_name(n)).collect();
    names.sort();
    names.into_iter().map(|n| installed(config, &n)).collect()
}

/// A line of `use`: a group (its lowest slot) and an installed driver's
/// name, or `None` for the shipped driver.
pub type Choice = (u8, Option<String>);

/// The choices in `use` (none if there is no file), and the lines that
/// could not be read.
pub fn read_use(config: &Path) -> (Vec<Choice>, Vec<String>) {
    let (mut out, mut bad) = (Vec::new(), Vec::new());
    let Ok(text) = std::fs::read_to_string(folder(config).join(USE_FILE)) else { return (out, bad) };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let parsed = line.split_once(' ').and_then(|(s, n)| Some((group_of(s.parse().ok()?)?[0], n.trim())));
        match parsed {
            Some((g, "default")) => set(&mut out, g, None),
            Some((g, n)) if valid_name(n) => set(&mut out, g, Some(n.to_string())),
            _ => bad.push(line.to_string()),
        }
    }
    (out, bad)
}

/// Sets the choice for group `g` (its lowest slot).
pub fn set(choices: &mut Vec<Choice>, g: u8, name: Option<String>) {
    choices.retain(|c| c.0 != g);
    choices.push((g, name));
    choices.sort_by_key(|c| c.0);
}

/// Writes `use`.
pub fn write_use(config: &Path, choices: &[Choice]) -> Result<PathBuf, String> {
    let dir = folder(config);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut s = String::from("# Which installed driver serves which slots (terse drivers use): the lowest slot of the\n# group and a driver's name, or default (the driver shipped with terse-bus). DEVICES.md 2.2.\n");
    for (g, n) in choices {
        s.push_str(&format!("{g} {}\n", n.as_deref().unwrap_or("default")));
    }
    let path = dir.join(USE_FILE);
    let tmp = dir.join(".use.tmp");
    std::fs::write(&tmp, s).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// What serves a group, as a core decides it from the folder (rules 2 and
/// 3 of `DEVICES.md` 2.2; a `--driver` comes before it).
#[derive(Clone, Debug)]
pub enum Source {
    /// The shipped driver; with the installed driver chosen and why it
    /// cannot be used, if one was.
    Shipped { chosen: bool, note: Option<(String, String)> },
    /// An installed driver: its name, program and manifest.
    Installed { name: String, program: PathBuf, manifest: Manifest },
}

/// For each group, what serves it; and notes about `use` lines that could
/// not be read.
pub fn resolve(config: &Path) -> (Vec<(&'static [u8], Source)>, Vec<String>) {
    let (choices, bad) = read_use(config);
    let notes = bad.iter().map(|l| format!("{}: a line that cannot be read: {l}", folder(config).join(USE_FILE).display())).collect();
    let out = GROUPS
        .iter()
        .map(|&g| {
            let source = match choices.iter().find(|c| c.0 == g[0]) {
                None => Source::Shipped { chosen: false, note: None },
                Some((_, None)) => Source::Shipped { chosen: true, note: None },
                Some((_, Some(name))) => {
                    let i = installed(config, name);
                    let ok = i.program().and_then(|p| {
                        let m = i.manifest.clone()?;
                        if m.group() != g { Err(format!("it serves slots {:?}, not {g:?}", m.group())) } else { Ok((p, m)) }
                    });
                    match ok {
                        Ok((program, manifest)) => Source::Installed { name: name.clone(), program, manifest },
                        Err(e) => Source::Shipped { chosen: false, note: Some((name.clone(), e)) },
                    }
                }
            };
            (g, source)
        })
        .collect();
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name_from("terminal (example)"), "terminal-example");
        assert_eq!(name_from("Terse Dev Terminal 2"), "terse-dev-terminal-2");
        assert_eq!(name_from("((("), "driver");
        assert_eq!(name_from("use"), "driver");
        assert!(valid_name("a.b-1") && !valid_name("-a") && !valid_name("A") && !valid_name("use") && !valid_name(""));
    }

    #[test]
    fn manifest_round_trip() {
        let m = Manifest { program: "terminal".into(), slots: vec![(2, 0), (10, 0)], name: "terminal (example)".into(), version: "0.1.0".into(), build: "fnv:0011223344556677".into() };
        let t = m.encode();
        assert_eq!(Manifest::parse(&t).unwrap(), m);
        assert_eq!(m.group(), &[2, 10]);
        assert!(Manifest::parse(&t.replace("slot 10 0", "slot 3 0")).is_err());
        assert!(Manifest::parse(&t.replace("program terminal", "program ../x")).is_err());
        assert!(Manifest::parse("terse-driver 2\n").is_err());
    }

    #[test]
    fn choices_and_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let c = tmp.path();
        assert!(resolve(c).0.iter().all(|(_, s)| matches!(s, Source::Shipped { chosen: false, note: None })));
        let d = folder(c).join("t");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("prog"), b"x").unwrap();
        let m = Manifest { program: "prog".into(), slots: vec![(2, 0)], name: "t".into(), version: String::new(), build: String::new() };
        std::fs::write(d.join(MANIFEST), m.encode()).unwrap();
        let mut ch = Vec::new();
        set(&mut ch, 2, Some("t".into()));
        set(&mut ch, 3, None);
        set(&mut ch, 7, Some("gone".into()));
        write_use(c, &ch).unwrap();
        std::fs::write(folder(c).join(USE_FILE), std::fs::read_to_string(folder(c).join(USE_FILE)).unwrap() + "11 x\n").unwrap();
        let (r, notes) = resolve(c);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(matches!(&r[0].1, Source::Installed { name, .. } if name == "t"));
        assert!(matches!(&r[1].1, Source::Shipped { chosen: true, note: None }));
        assert!(matches!(&r[4].1, Source::Shipped { note: Some((n, e)), .. } if n == "gone" && e == "not installed"));
        std::fs::remove_file(d.join("prog")).unwrap();
        assert!(matches!(&resolve(c).0[0].1, Source::Shipped { note: Some((_, e)), .. } if e.contains("missing")));
        assert_eq!(list(c).len(), 1);
    }
}

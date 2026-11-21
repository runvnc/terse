//! `terse drivers`: the user's drivers folder, `<config>/drivers`
//! (`DEVICES.md` 2.2), which `terse-bus` reads when it starts. `add` asks
//! `terse-bus --probe-driver` (next to this program) for the driver's
//! handshake, so that the core's own code reads what the driver says.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use terse_net::driverlist::{self, Manifest, Source};

use crate::{Args, USAGE};

/// What a group of slots is, for people.
fn what(group: &[u8]) -> &'static str {
    match group.first() {
        Some(2) => "slots 2 and 10 (the terminal, full text)",
        Some(3) => "slots 3 and 9 (the vector displays)",
        Some(4) => "slot 4 (content)",
        Some(6) => "slot 6 (sound)",
        _ => "slot 7 (the page)",
    }
}

const FULL_RIGHTS: &str = "A driver is a native program that runs with your full rights: it can read and change all your files \
and use the network, like any program you install. Install only drivers you trust.";

/// The directory this program (and so terse-bus and the shipped drivers) is in.
fn own_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe.parent().ok_or("no directory")?.to_path_buf())
}

pub fn drivers(args: &[String]) -> Result<ExitCode, String> {
    let (command, rest) = args.split_first().ok_or(USAGE)?;
    let args = Args::parse(rest, &[])?;
    args.check_known(&["--config", "--slot", "--name"])?;
    if command != "add" && (args.has("--slot") || args.has("--name")) {
        return Err(format!("--slot and --name are for terse drivers add\n{USAGE}"));
    }
    let config = args.get("--config").map(PathBuf::from).or_else(terse_net::default_config_dir).ok_or("no config directory (use --config)")?;
    match (command.as_str(), args.positional.as_slice()) {
        ("list", []) => list(&config),
        ("add", [file]) => add(&config, file, args.get("--slot"), args.get("--name")),
        ("remove", [name]) => remove(&config, name),
        ("use", [slot, name]) => choose(&config, slot, name),
        _ => Err(USAGE.into()),
    }
}

fn list(config: &Path) -> Result<ExitCode, String> {
    let dir = own_dir()?;
    let (sources, bad) = driverlist::resolve(config);
    println!("{}", driverlist::folder(config).display());
    let mut used = Vec::new();
    for (group, source) in &sources {
        let shipped = driverlist::shipped_program(group);
        let built = if dir.join(shipped).exists() { "" } else { ", not found next to terse" };
        let (who, why) = match source {
            Source::Installed { name, manifest, .. } => {
                used.push(name.clone());
                (name.clone(), format!("installed: {}", manifest.identity()))
            }
            Source::Shipped { chosen: true, .. } => (shipped.to_string(), format!("shipped{built}; chosen with terse drivers use {} default", group[0])),
            Source::Shipped { note: Some((name, e)), .. } => {
                used.push(name.clone());
                (shipped.to_string(), format!("shipped{built}; the installed driver {name} cannot be used: {e}"))
            }
            Source::Shipped { .. } => (shipped.to_string(), format!("shipped{built}")),
        };
        println!("{:<40} {who} ({why})", what(group));
    }
    for i in driverlist::list(config).iter().filter(|i| !used.contains(&i.name)) {
        match &i.manifest {
            Ok(m) => println!("not used: {} ({}; {}): terse drivers use {} {}", i.name, m.identity(), what(m.group()), m.group().first().unwrap_or(&0), i.name),
            Err(e) => println!("not used: {} ({e})", i.name),
        }
    }
    for b in bad {
        println!("{b}");
    }
    println!("A --driver option of terse-bus comes before all of these (DEVICES.md 2.2).");
    Ok(ExitCode::SUCCESS)
}

/// What the program answered for one group, as `terse-bus --probe-driver`
/// prints it.
fn parse_probe(out: &str) -> Vec<Manifest> {
    let mut found = Vec::new();
    for block in out.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut m = Manifest { program: String::new(), slots: Vec::new(), name: String::new(), version: String::new(), build: String::new() };
        for line in block.lines() {
            let (word, value) = line.split_once(' ').unwrap_or((line, ""));
            match word {
                "name" => m.name = value.to_string(),
                "version" if value != "-" => m.version = value.to_string(),
                "build" if value != "-" => m.build = value.to_string(),
                "slot" => {
                    if let Some((s, v)) = value.split_once(' ').and_then(|(s, v)| Some((s.parse().ok()?, v.parse().ok()?))) {
                        m.slots.push((s, v));
                    }
                }
                _ => {}
            }
        }
        m.slots.sort_unstable();
        if !m.slots.is_empty() {
            found.push(m);
        }
    }
    found
}

fn add(config: &Path, file: &str, slot: Option<&str>, name: Option<&str>) -> Result<ExitCode, String> {
    let path = std::fs::canonicalize(file).map_err(|e| format!("{file}: {e}"))?;
    if !path.is_file() {
        return Err(format!("{}: not a file", path.display()));
    }
    let bus = own_dir()?.join("terse-bus");
    if !bus.exists() {
        return Err(format!("{} is needed to read the driver's handshake (build it: scripts/cargo build -p terse-bus)", bus.display()));
    }
    // The handshake, by the core's own code (DEVICES.md 2.2).
    let mut probe = Command::new(&bus);
    probe.arg("--probe-driver").arg(&path);
    if let Some(s) = slot {
        let g = s.parse().ok().and_then(driverlist::group_of).ok_or(format!("--slot {s}: not a slot a driver serves (2, 3, 4, 6, 7, 9 or 10)"))?;
        probe.args(["--slot", &g[0].to_string()]);
    }
    let out = probe.output().map_err(|e| format!("{}: {e}", bus.display()))?;
    let found = parse_probe(&String::from_utf8_lossy(&out.stdout));
    let refused = String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.starts_with("terse-bus: ")).map(|l| format!("  {}", &l[11..])).collect::<Vec<_>>().join("\n");
    let mut m = match found.as_slice() {
        [] => return Err(format!("{} did not answer as a driver for any slots:\n{refused}", path.display())),
        [m] => m.clone(),
        _ => {
            let groups: Vec<String> = found.iter().map(|m| m.group().first().unwrap_or(&0).to_string()).collect();
            return Err(format!("{} answers for more than one group of slots ({}): say which with --slot", path.display(), groups.join(", ")));
        }
    };
    // Only versions this host implements: no machine could be given another.
    if let Some((s, v)) = m.slots.iter().find(|(s, v)| !terse_core::image::SUPPORTED_DEVICES.iter().any(|d| d.slot == *s && d.version == *v)) {
        return Err(format!("{} implements device {s} version {v}, which this host does not know (SPEC §image)", path.display()));
    }
    let name = match name {
        Some(n) if driverlist::valid_name(n) => n.to_string(),
        Some(n) => return Err(format!("--name {n}: a name is 1 to 64 of a-z, 0-9, - and ., starting with a letter or digit, not `use`")),
        None => driverlist::name_from(&m.name),
    };
    let program = path.file_name().and_then(|f| f.to_str()).filter(|f| *f != driverlist::MANIFEST).unwrap_or("driver").to_string();
    m.program = program.clone();
    let group = m.group();
    println!("{}", path.display());
    println!("  name     {}", m.name);
    println!("  version  {}", if m.version.is_empty() { "-" } else { &m.version });
    println!("  build    {}", if m.build.is_empty() { "-" } else { &m.build });
    println!("  slots    {}", m.slots.iter().map(|(s, v)| format!("{s} (version {v})")).collect::<Vec<_>>().join(", "));
    println!("{FULL_RIGHTS}");

    // Into a new directory first, then in place of any older one.
    let folder = driverlist::folder(config);
    let target = folder.join(&name);
    let tmp = folder.join(format!(".{name}.new"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let copied = tmp.join(&program);
    std::fs::copy(&path, &copied).map_err(|e| format!("{}: {e}", copied.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {e}", copied.display()))?;
    }
    std::fs::write(tmp.join(driverlist::MANIFEST), m.encode()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let replaced = target.exists();
    if replaced {
        std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    std::fs::rename(&tmp, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    let (mut choices, _) = driverlist::read_use(config);
    driverlist::set(&mut choices, group[0], Some(name.clone()));
    driverlist::write_use(config, &choices)?;
    println!(
        "{} {name} in {}: terse-bus uses it for {} from its next start (terse drivers use {} default goes back to the shipped driver).",
        if replaced { "Replaced" } else { "Installed" },
        target.display(),
        what(group),
        group[0]
    );
    Ok(ExitCode::SUCCESS)
}

fn remove(config: &Path, name: &str) -> Result<ExitCode, String> {
    let dir = driverlist::folder(config).join(name);
    if !driverlist::valid_name(name) || !dir.is_dir() {
        return Err(format!("{name} is not installed (terse drivers list)"));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (mut choices, _) = driverlist::read_use(config);
    let freed: Vec<u8> = choices.iter().filter(|c| c.1.as_deref() == Some(name)).map(|c| c.0).collect();
    choices.retain(|c| c.1.as_deref() != Some(name));
    driverlist::write_use(config, &choices)?;
    println!("Removed {name}.");
    for g in freed {
        println!("{}: the shipped driver, from terse-bus's next start.", what(driverlist::group_of(g).unwrap_or(&[])));
    }
    Ok(ExitCode::SUCCESS)
}

fn choose(config: &Path, slot: &str, name: &str) -> Result<ExitCode, String> {
    let group = slot.parse().ok().and_then(driverlist::group_of).ok_or(format!("{slot}: not a slot a driver serves (2, 3, 4, 6, 7, 9 or 10)"))?;
    let (mut choices, _) = driverlist::read_use(config);
    if name == "default" {
        driverlist::set(&mut choices, group[0], None);
        driverlist::write_use(config, &choices)?;
        println!("{}: the shipped driver, from terse-bus's next start.", what(group));
        return Ok(ExitCode::SUCCESS);
    }
    let i = driverlist::installed(config, name);
    let m = i.manifest.as_ref().map_err(|e| format!("{name}: {e} (terse drivers list)"))?;
    i.program().map_err(|e| format!("{name}: {e}"))?;
    if m.group() != group {
        return Err(format!("{name} serves {}, not {}", what(m.group()), what(group)));
    }
    driverlist::set(&mut choices, group[0], Some(name.to_string()));
    driverlist::write_use(config, &choices)?;
    println!("{}: {name} ({}), from terse-bus's next start.", what(group), m.identity());
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_output() {
        let out = "group 2\nname terminal (example)\nversion 0.1.0\nbuild fnv:21dde22df5d9172c\nslot 2 0\nslot 10 0\n\n";
        let m = super::parse_probe(out);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].name.as_str(), m[0].slots.as_slice(), m[0].build.as_str()), ("terminal (example)", &[(2, 0), (10, 0)][..], "fnv:21dde22df5d9172c"));
        assert!(super::parse_probe("").is_empty());
    }
}

//! What terse-host does to a machine the rule names (`PERFORMANCE.md`
//! R15), as terse-bus does: its snapshot at the warning, and at the stop
//! its snapshot and the stop itself (halted, `SPEC.md` §stopping). With a
//! shell the snapshot goes into the machine's automatic slot (15 + its
//! handle, `spec/machines.md` §machines.snapshots), written where the
//! shell's snapshots are and told to the shell; a machine alone's into a
//! file in the record's directory, `stopped-<image>.tsnp`. The shell itself
//! is never snapshotted. The snapshot is written on the frame loop, as the
//! bus's is: once at a warning and once at a stop.

use std::path::Path;

use terse_core::{Snapshot, System};

/// Machine `serial`'s handle (0: the shell or the machine alone), if it
/// is still there.
pub fn handle(system: &System, serial: u32) -> Option<u8> {
    system.machine_by_serial(serial).map(|(h, _)| h)
}

/// Takes machine `h`'s snapshot and keeps it. Returns where it went, as
/// the record's `stop` line says it (`slot<n>`, `file`, `none`), and a
/// description for the user.
pub fn save(system: &mut System, data: &Path, record_dir: Option<&Path>, h: u8) -> (String, String) {
    if h == 0 && system.is_system() {
        return ("none".into(), "the shell is never snapshotted".into());
    }
    let snapshot = match h {
        0 => Snapshot::of(system.shell()),
        h => match system.child(h).and_then(|c| c.machine.as_ref()) {
            Some(m) => Snapshot::of(m),
            None => return ("none".into(), "no machine".into()),
        },
    };
    if h == 0 {
        let Some(dir) = record_dir else { return ("none".into(), "no record directory to keep it in".into()) };
        let name: String = snapshot.image_id()[..8].iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("stopped-{name}.tsnp"));
        let written = std::fs::create_dir_all(dir).and_then(|_| {
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, snapshot.encode())?;
            std::fs::rename(&tmp, &path)
        });
        return match written {
            Ok(()) => ("file".into(), path.display().to_string()),
            Err(e) => ("none".into(), format!("{}: {e}", path.display())),
        };
    }
    let slot = terse_core::snapshot::auto_slot(h);
    let owner = terse_net::Owner::of(system.shell());
    match terse_net::Snapshots::in_data_dir(data).save(&owner, slot, &snapshot.encode()) {
        Ok(()) => {
            system.set_snapshot_info(slot, Some(snapshot.info()));
            (format!("slot{slot}"), format!("snapshot slot {slot}"))
        }
        Err(e) => ("none".into(), format!("snapshot slot {slot} could not be stored: {e}")),
    }
}

/// Stops machine `h` (0: the shell or the machine alone) between frames,
/// for device `slot` (`SPEC.md` §stopping): halted.
pub fn halt(system: &mut System, h: u8, slot: u8) {
    match h {
        0 => system.shell_mut().halt(slot),
        h => {
            if let Some(m) = system.child_mut(h).and_then(|c| c.machine.as_mut()) {
                m.halt(slot);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terse_core::machines::state;
    use terse_core::{Image, Script};
    use terse_net::{BlobStore, Consent, ContentService, HeadlessSystem, Node, Records, Snapshots, builtin};

    /// The built-in shell with hello started (handle 1), as terse-host
    /// runs it, on a store and data directory of its own.
    struct Sys {
        system: System,
        headless: HeadlessSystem,
        _dirs: (tempfile::TempDir, tempfile::TempDir),
    }

    impl Sys {
        fn new(script: &str) -> Sys {
            let (store, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let mut node = Node::new(BlobStore::open(store.path(), None).unwrap(), None);
            node.set_records(Records::in_data_dir(data.path()));
            node.set_snapshots(Snapshots::in_data_dir(data.path()));
            let ids = builtin::install(node.store(), &[]).unwrap();
            let shell = Image::parse(builtin::SHELL).unwrap();
            let mut system = System::new(&shell, true, ids);
            system.set_shell_identity(Some(terse_core::ProgramId { publisher: [0; 32], name: shell.title.clone() }));
            terse_net::snapshots::load_table(&mut system, data.path());
            let content = ContentService::start_with_consent(node, Consent::Deny);
            Sys { system, headless: HeadlessSystem::new(Some(content), Script::parse(script).unwrap()), _dirs: (store, data) }
        }

        fn data(&self) -> &Path {
            self._dirs.1.path()
        }

        fn run_until(&mut self, frame: u32) {
            while self.system.shell().frame() < frame {
                self.headless.frame(&mut self.system);
                assert!(self.system.stopped().is_none());
            }
        }

        fn text(&self) -> String {
            let mut out = self.system.shell().terminal().unwrap().text();
            let page = self.system.shell().page().unwrap();
            if let Some(doc) = page.document() {
                let layout = terse_core::page::reference_layout(doc, page.region().w.max(1));
                for l in &layout.lines {
                    out += &l.spans.iter().map(|s| s.text.as_str()).collect::<String>();
                    out.push('\n');
                }
            }
            out
        }
    }

    /// The warning's and the stop's snapshots go into hello's automatic
    /// slot (16); halted, the shell shows it so (which device, saved), and
    /// Enter on its line restores it from that slot, where it goes on.
    #[test]
    fn a_machine_saved_and_halted_shows_in_the_shell_and_is_restored() {
        // Hello (handle 1); later the home key and Enter on its line.
        let mut s = Sys::new("2 press down\n3 press down\n4 press down\n5 press enter\n35 home\n230 press enter\n");
        s.run_until(20);
        assert_eq!(s.system.state(1), state::RUNNING);
        let serial = s.system.child(1).unwrap().serial;
        assert_eq!(handle(&s.system, serial), Some(1));
        let data = s.data().to_path_buf();
        // The warning.
        assert_eq!(save(&mut s.system, &data, None, 1).0, "slot16");
        s.run_until(40);
        // The stop, for the emulation (device 1).
        let (saved, _) = save(&mut s.system, &data, None, 1);
        assert_eq!(saved, "slot16");
        let frames = s.system.child(1).unwrap().machine.as_ref().unwrap().frame();
        halt(&mut s.system, 1, 1);
        assert_eq!(save(&mut s.system, &data, None, 0).0, "none", "the shell is never snapshotted");
        s.run_until(62);
        assert_eq!(s.system.state(1), state::HALTED);
        let text = s.text();
        for words in ["halted: stopped by Tersenet", "the system device", "kept making frames late", "its state was saved", "Its state was saved: Enter restores it."] {
            assert!(text.replace('\n', " ").contains(words), "{words}: {text}");
        }
        let table = Snapshots::in_data_dir(&data).table(&terse_net::Owner::of(s.system.shell()));
        assert!(table[16].is_some());
        // Enter on its line: restored in its place, going on.
        s.run_until(240);
        assert_eq!(s.system.state(1), state::RUNNING);
        let m = s.system.child(1).unwrap().machine.as_ref().unwrap();
        assert!(m.frame() > frames, "{} {frames}", m.frame());
    }

    /// A machine alone: its snapshot goes into a file in the record's
    /// directory, and it is halted.
    #[test]
    fn a_machine_alone_is_saved_to_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let hello = Image::parse(&std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../programs/hello/hello.terse")).unwrap()).unwrap();
        let mut system = System::new(&hello, false, Vec::new());
        let (saved, path) = save(&mut system, dir.path(), Some(dir.path()), 0);
        assert_eq!(saved, "file");
        assert!(path.ends_with(".tsnp") && Path::new(&path).exists());
        halt(&mut system, 0, 7);
        assert_eq!(system.shell().stopped(), Some(terse_core::Stopped::Halted(7)));
    }
}

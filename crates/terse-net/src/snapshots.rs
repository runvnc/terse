//! Snapshots (SPEC 12.7): the machines a shell asked to keep, private to
//! this host, in slots 0-15 per shell (by the shell's owner, as named
//! records are, SPEC 9.6). They are never sent anywhere.
//!
//! Layout under the snapshots directory (`<data>/snapshots`): the owner's
//! directory as for records (`program/<publisher hex>/<name hex>` or
//! `image/<image ID hex>`), and in it one file per slot, `00` to `15`, in
//! the snapshot file format of STATE.md 4. A file is written to a
//! temporary file, synced and renamed into place, and the directory is
//! synced.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use terse_core::snapshot::{SLOTS, SnapshotInfo};

use crate::records::{Owner, sync_dir};

/// The snapshots directory.
#[derive(Clone, Debug)]
pub struct Snapshots {
    root: PathBuf,
}

impl Snapshots {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Snapshots { root: root.into() }
    }

    /// `<data>/snapshots`.
    pub fn in_data_dir(data: &Path) -> Self {
        Self::open(data.join("snapshots"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file(&self, owner: &Owner, slot: u8) -> PathBuf {
        owner.dir(&self.root).join(format!("{slot:02}"))
    }

    /// Replaces slot `slot`'s snapshot file, durably.
    pub fn save(&self, owner: &Owner, slot: u8, file: &[u8]) -> io::Result<()> {
        if slot as usize >= SLOTS {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no such slot"));
        }
        let dir = owner.dir(&self.root);
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".{slot:02}.tmp"));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(file)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, self.file(owner, slot))?;
        sync_dir(&dir)
    }

    /// Slot `slot`'s snapshot file, if there is one.
    pub fn load(&self, owner: &Owner, slot: u8) -> io::Result<Option<Vec<u8>>> {
        if slot as usize >= SLOTS {
            return Ok(None);
        }
        match fs::read(self.file(owner, slot)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Removes slot `slot`'s snapshot (done whether or not there was one).
    pub fn delete(&self, owner: &Owner, slot: u8) -> io::Result<()> {
        let dir = owner.dir(&self.root);
        match fs::remove_file(self.file(owner, slot)) {
            Ok(()) => sync_dir(&dir),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// What each slot holds, from the head of its file (a file whose head
    /// cannot be read counts as empty; the rest of a file is checked when
    /// it is restored).
    pub fn table(&self, owner: &Owner) -> [Option<SnapshotInfo>; SLOTS] {
        let mut out: [Option<SnapshotInfo>; SLOTS] = Default::default();
        for (slot, info) in out.iter_mut().enumerate() {
            *info = self.load(owner, slot as u8).ok().flatten().and_then(|f| SnapshotInfo::from_file_head(&f));
        }
        out
    }
}

/// Gives the shell of `system` its snapshot slots as they are in the data
/// directory `data` (SPEC 12.7); nothing for a single machine.
pub fn load_table(system: &mut terse_core::System, data: &Path) {
    if system.is_system() {
        let owner = Owner::of(system.shell());
        system.set_snapshot_table(Snapshots::in_data_dir(data).table(&owner));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terse_core::{Image, Machine, Snapshot};

    #[test]
    fn save_load_table_delete() {
        let dir = tempfile::tempdir().unwrap();
        let s = Snapshots::in_data_dir(dir.path());
        let owner = Owner::Program { publisher: [0; 32], name: b"shell".to_vec() };
        let image = Image::parse(&Image::build("t", 1, &[], &[0x6f, 0, 0, 0])).unwrap();
        let snapshot = Snapshot::of(&Machine::new(&image));
        assert!(s.table(&owner).iter().all(Option::is_none));
        s.save(&owner, 3, &snapshot.encode()).unwrap();
        let table = s.table(&owner);
        assert_eq!(table[3], Some(snapshot.info()));
        assert_eq!(table.iter().flatten().count(), 1);
        assert_eq!(Snapshot::decode(&s.load(&owner, 3).unwrap().unwrap()).unwrap(), snapshot);
        // Per owner.
        assert!(s.table(&Owner::Image([1; 32])).iter().all(Option::is_none));
        assert!(s.save(&owner, 16, b"x").is_err());
        s.delete(&owner, 3).unwrap();
        s.delete(&owner, 3).unwrap();
        assert_eq!(s.load(&owner, 3).unwrap(), None);
        let names: Vec<_> = fs::read_dir(owner.dir(s.root())).unwrap().collect();
        assert!(names.is_empty(), "no temporary files left");
    }
}
